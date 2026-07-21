//! End-to-end Step 5 test: build the `mock-harness` binary, spawn it
//! through `Supervisor`, and verify events flow into the repository
//! and through the sink.

use event_schema::{Event, EventKind, WorkerState};
use orchestrator::{parser::WorkerEventSink, Repository, SpawnRequest, Supervisor};
use std::path::PathBuf;
use std::process::Command;
use std::sync::{Arc, Mutex, Once};
use std::time::{Duration, Instant};

#[derive(Default)]
struct CapturingSink {
    events: Mutex<Vec<Event>>,
}

impl WorkerEventSink for CapturingSink {
    fn emit(&self, event: &Event) {
        self.events.lock().unwrap().push(event.clone());
    }
}

static BUILD_ONCE: Once = Once::new();

/// Build `mock-harness` once per `cargo test` process. Cargo's
/// incremental build makes subsequent invocations cheap.
fn ensure_mock_harness_built() -> PathBuf {
    BUILD_ONCE.call_once(|| {
        let out = Command::new(env!("CARGO"))
            .args(["build", "-p", "vigla-mock-harness", "--bin", "mock-harness"])
            .output()
            .expect("invoke cargo build");
        if !out.status.success() {
            panic!(
                "cargo build -p vigla-mock-harness failed:\nstdout: {}\nstderr: {}",
                String::from_utf8_lossy(&out.stdout),
                String::from_utf8_lossy(&out.stderr),
            );
        }
    });

    // Workspace root is two levels up from CARGO_MANIFEST_DIR (crates/orchestrator/).
    let manifest_dir = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let workspace_root = manifest_dir
        .parent()
        .and_then(|p| p.parent())
        .expect("crates/orchestrator/ has a workspace root two levels up")
        .to_path_buf();
    let bin = workspace_root.join("target/debug/mock-harness");
    assert!(bin.exists(), "expected mock-harness at {bin:?}");
    bin
}

async fn drain_until_ended(supervisor: &Arc<Supervisor>, worker_id: &str, deadline: Duration) {
    let start = Instant::now();
    while start.elapsed() < deadline {
        // The worker is removed from the supervisor map once its
        // supervise task completes (see Supervisor::supervise tail).
        if !supervisor.is_running(worker_id).await {
            return;
        }
        tokio::time::sleep(Duration::from_millis(25)).await;
    }
    panic!("worker {worker_id} did not finish within {deadline:?}");
}

#[tokio::test]
async fn claude_happy_runs_end_to_end_through_supervisor() {
    let mock_harness = ensure_mock_harness_built();
    let repo = Repository::open_in_memory().await.unwrap();
    let sink: Arc<CapturingSink> = Arc::new(CapturingSink::default());

    let supervisor = Supervisor::new(repo.clone(), Arc::clone(&sink) as _, mock_harness);

    let worker_id = supervisor
        .spawn_mock(SpawnRequest {
            script: "claude_happy".into(),
            speed: 0.0,
            task_title: "test task".into(),
        })
        .await
        .expect("spawn_mock");

    drain_until_ended(&supervisor, &worker_id, Duration::from_secs(5)).await;

    // Persisted events should match the captured emissions in seq order.
    let persisted = repo.replay_for_worker(&worker_id).await.unwrap();
    assert!(
        !persisted.is_empty(),
        "expected at least one event persisted"
    );

    // Persisted events vs captured emissions:
    // - Non-progress events must be identical in order (the sink
    //   forwards every state_change / log / cost / etc. unchanged).
    // - Progress events are coalesced by CoalescingSink to
    //   "latest per (worker, task)" flushed every 100ms, so the
    //   sink may have fewer progress events than the repo.
    let emitted = sink.events.lock().unwrap().clone();
    let strip_progress = |evs: &[event_schema::Event]| -> Vec<event_schema::Event> {
        evs.iter()
            .filter(|e| !matches!(e.kind, EventKind::Progress(_)))
            .cloned()
            .collect()
    };
    assert_eq!(
        strip_progress(&persisted),
        strip_progress(&emitted),
        "non-progress events must match between repo and sink"
    );
    // Sanity: the sink saw at least one progress event (coalesced),
    // and never more than the repo persisted.
    let persisted_progress = persisted
        .iter()
        .filter(|e| matches!(e.kind, EventKind::Progress(_)))
        .count();
    let emitted_progress = emitted
        .iter()
        .filter(|e| matches!(e.kind, EventKind::Progress(_)))
        .count();
    assert!(emitted_progress <= persisted_progress,
        "sink should not see more progress events ({emitted_progress}) than repo ({persisted_progress})");

    // First event is worker-level idle (per event-schema.md §5).
    match &persisted[0].kind {
        EventKind::StateChange(sc) => assert_eq!(sc.state, WorkerState::Idle),
        other => panic!("expected first event = state_change idle, got {other:?}"),
    }
    assert!(persisted[0].task_id.is_none());

    // Final event = completion.
    let last = persisted.last().unwrap();
    assert!(matches!(last.kind, EventKind::Completion(_)));

    // Schema version stamped on every event.
    for e in &persisted {
        assert_eq!(e.schema_version, "2.0");
    }
}

#[tokio::test]
async fn unknown_script_returns_error_without_spawning() {
    let mock_harness = ensure_mock_harness_built();
    let repo = Repository::open_in_memory().await.unwrap();
    let sink: Arc<CapturingSink> = Arc::new(CapturingSink::default());

    let supervisor = Supervisor::new(repo, Arc::clone(&sink) as _, mock_harness);

    let result = supervisor
        .spawn_mock(SpawnRequest {
            script: "doesnt_exist".into(),
            speed: 0.0,
            task_title: "x".into(),
        })
        .await;

    assert!(result.is_err());
    assert_eq!(sink.events.lock().unwrap().len(), 0);
}
