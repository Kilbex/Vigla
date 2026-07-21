//! Step-17 acceptance smoke for scenario 3 (sidebar-stop / drawer-stop):
//! prove `supervise_with_adapter` honours the cancel signal end-to-end
//! and that the adapter sees `AdapterExit::Killed` so the worker tile
//! transitions to `failed` instead of stalling in `executing`.
//!
//! No real CLI / no API spend — uses `/bin/sleep` as a long-running
//! child stand-in. The fixed `cancelled` flag in supervise_with_adapter
//! plus the new `classify_exit` helper from audit round 3 are the
//! unit under test.

use adapter_core::{Adapter, AdapterExit};
use event_schema::{Event, EventKind, LogStream, WorkerState};
use orchestrator::{parser::WorkerEventSink, Repository, Supervisor};
use std::path::PathBuf;
use std::process::Stdio;
use std::sync::{Arc, Mutex, Once};
use std::time::Duration;
use tokio::process::Command;
use tokio::sync::oneshot;

static BUILD_ONCE: Once = Once::new();

/// Build `mock-harness` once per `cargo test` process for the tests in
/// this file that go through `spawn_mock` (and therefore need a real
/// child binary on disk). Mirrors the helper in `spawn_mock.rs` —
/// duplicated rather than shared because integration tests don't have
/// a common test-support module.
fn ensure_mock_harness_built() -> PathBuf {
    BUILD_ONCE.call_once(|| {
        let out = std::process::Command::new(env!("CARGO"))
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

#[derive(Default)]
struct CapturingSink {
    events: Mutex<Vec<Event>>,
}

impl WorkerEventSink for CapturingSink {
    fn emit(&self, event: &Event) {
        self.events.lock().unwrap().push(event.clone());
    }
}

/// Adapter that promotes the worker into `executing` on its first
/// finalize call regardless of input — needed because `/bin/sleep`
/// emits no stdout, so the natural ingest_line path never fires. The
/// AdapterExit observed by finalize is recorded for the test to
/// assert against.
#[derive(Debug)]
struct StartedThenFinalizedAdapter {
    worker_id: String,
    seq: u64,
    started: bool,
    observed_exit: Arc<Mutex<Option<AdapterExit>>>,
}

impl Adapter for StartedThenFinalizedAdapter {
    fn ingest_line(&mut self, _line: &str, _stream: LogStream) -> Vec<Event> {
        Vec::new()
    }

    fn finalize(&mut self, exit: AdapterExit) -> Vec<Event> {
        *self.observed_exit.lock().unwrap() = Some(exit);
        if !self.started {
            // Synthesize the executing state so the test can assert
            // the supervise loop ran end-to-end through finalize.
            self.started = true;
            let event = Event {
                schema_version: "1.0".into(),
                worker_id: self.worker_id.clone(),
                task_id: None,
                seq: self.seq,
                ts: "2026-05-09T00:00:00.000Z".into(),
                kind: EventKind::StateChange(event_schema::StateChange {
                    state: WorkerState::Failed,
                    from: Some(WorkerState::Executing),
                    note: match exit {
                        AdapterExit::Killed => Some("worker stopped".into()),
                        _ => None,
                    },
                }),
            };
            self.seq += 1;
            return vec![event];
        }
        Vec::new()
    }
}

#[tokio::test]
async fn cancel_signal_kills_child_and_finalize_sees_killed() {
    let repo = Repository::open_in_memory().await.unwrap();
    let sink: Arc<CapturingSink> = Arc::new(CapturingSink::default());
    let supervisor = Supervisor::new(
        repo.clone(),
        Arc::clone(&sink) as Arc<dyn WorkerEventSink>,
        PathBuf::new(),
    );

    let worker_id = "stop-mid-run-worker".to_string();
    let now = "2026-05-09T00:00:00.000Z".to_string();
    repo.insert_worker(&event_schema::WorkerInfo {
        id: worker_id.clone(),
        name: "stop-mid-run".into(),
        vendor: event_schema::Vendor::Mock,
        cli_binary: "sleep".into(),
        cli_version: None,
        cwd: ".".into(),
        model: None,
        spawned_at: now,
        ended_at: None,
    })
    .await
    .unwrap();

    // Long-lived child — would otherwise block for 30 seconds.
    let mut child = Command::new("/bin/sleep")
        .arg("30")
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true)
        .spawn()
        .expect("spawn /bin/sleep");
    let stdout = child.stdout.take().expect("piped stdout");
    let stderr = child.stderr.take().expect("piped stderr");

    let (cancel_tx, cancel_rx) = oneshot::channel();
    let observed_exit = Arc::new(Mutex::new(None));

    // Fire the cancel after 100 ms — enough for the supervise task to
    // be parked in select! awaiting either stream / child.
    let cancel_handle = tokio::spawn(async move {
        tokio::time::sleep(Duration::from_millis(100)).await;
        let _ = cancel_tx.send(());
    });

    let started = std::time::Instant::now();
    Arc::clone(&supervisor)
        .supervise_with_adapter(
            child,
            stdout,
            stderr,
            Box::new(StartedThenFinalizedAdapter {
                worker_id: worker_id.clone(),
                seq: 0,
                started: false,
                observed_exit: Arc::clone(&observed_exit),
            }),
            worker_id.clone(),
            cancel_rx,
        )
        .await;
    let elapsed = started.elapsed();
    let _ = cancel_handle.await;

    // Cancel must have fired well before the 30-second sleep would
    // have completed — proves the cancel branch in select! actually
    // killed the child instead of waiting on natural EOF.
    assert!(
        elapsed < Duration::from_secs(5),
        "supervise should return promptly after cancel; took {elapsed:?}"
    );

    // Adapter saw the Killed exit (confirms the `cancelled` flag in
    // supervise_with_adapter routed correctly to AdapterExit::Killed).
    let observed = *observed_exit.lock().unwrap();
    assert_eq!(
        observed,
        Some(AdapterExit::Killed),
        "adapter.finalize must be called with AdapterExit::Killed after cancel; got {observed:?}"
    );

    // The synthetic Failure-state event from finalize must have
    // reached the user sink (proves the post-loop finalize plumbing
    // through CoordinatingSink works).
    let events = sink.events.lock().unwrap();
    let last = events.last().expect("at least one event");
    let EventKind::StateChange(sc) = &last.kind else {
        panic!(
            "expected last event to be a state_change, got {:?}",
            last.kind
        );
    };
    assert_eq!(sc.state, WorkerState::Failed);
    assert_eq!(sc.note.as_deref(), Some("worker stopped"));
}

/// Audit r5 — without bounded drains, a child whose grandchild
/// inherits the stdout/stderr fd would wedge supervise_with_adapter
/// forever on the post-loop drain (or after-cancel drain). With the
/// fix, supervise returns promptly even if a backgrounded subprocess
/// keeps the pipes open well past the parent's exit.
#[tokio::test]
async fn supervise_returns_promptly_when_grandchild_holds_pipes_open() {
    let repo = Repository::open_in_memory().await.unwrap();
    let sink: Arc<CapturingSink> = Arc::new(CapturingSink::default());
    let supervisor = Supervisor::new(
        repo.clone(),
        Arc::clone(&sink) as Arc<dyn WorkerEventSink>,
        PathBuf::new(),
    );

    let worker_id = "grandchild-pipe-worker".to_string();
    let now = "2026-05-10T00:00:00.000Z".to_string();
    repo.insert_worker(&event_schema::WorkerInfo {
        id: worker_id.clone(),
        name: "grandchild".into(),
        vendor: event_schema::Vendor::Mock,
        cli_binary: "sh".into(),
        cli_version: None,
        cwd: ".".into(),
        model: None,
        spawned_at: now,
        ended_at: None,
    })
    .await
    .unwrap();

    // The parent shell exits immediately; the backgrounded sleep
    // inherits the pipes and keeps them open for 60 seconds. Without
    // the audit-r5 bounded drain (500 ms per line) + post-cancel
    // skip, supervise would block on the inherited fd until the
    // sleep finished naturally.
    let mut child = Command::new("/bin/sh")
        .arg("-c")
        .arg("/bin/sleep 60 &")
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true)
        .spawn()
        .expect("spawn /bin/sh");
    let stdout = child.stdout.take().expect("piped stdout");
    let stderr = child.stderr.take().expect("piped stderr");

    let (cancel_tx, cancel_rx) = oneshot::channel();
    let observed_exit = Arc::new(Mutex::new(None));

    let cancel_handle = tokio::spawn(async move {
        tokio::time::sleep(Duration::from_millis(50)).await;
        let _ = cancel_tx.send(());
    });

    let started = std::time::Instant::now();
    Arc::clone(&supervisor)
        .supervise_with_adapter(
            child,
            stdout,
            stderr,
            Box::new(StartedThenFinalizedAdapter {
                worker_id: worker_id.clone(),
                seq: 0,
                started: false,
                observed_exit: Arc::clone(&observed_exit),
            }),
            worker_id.clone(),
            cancel_rx,
        )
        .await;
    let elapsed = started.elapsed();
    let _ = cancel_handle.await;

    assert!(
        elapsed < Duration::from_secs(5),
        "supervise must return promptly even when a grandchild keeps the pipes open; took {elapsed:?}"
    );
}

/// Audit r5 — `Supervisor::stop` previously removed the worker from the
/// `workers` map BEFORE awaiting the JoinHandle, so `is_running()`
/// returned false during the cancel-and-drain window even though the
/// supervise task was still running. Callers polling that signal could
/// act prematurely. Verify the entry stays in the map until the
/// supervise task self-removes after cleanup.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn stop_keeps_worker_visible_until_join_completes() {
    use orchestrator::SpawnRequest;

    let mock_harness = ensure_mock_harness_built();
    let repo = Repository::open_in_memory().await.unwrap();
    let sink: Arc<dyn WorkerEventSink> = Arc::new(CapturingSink::default());
    let supervisor = Supervisor::new(repo, sink, mock_harness);

    let id = supervisor
        .spawn_mock(SpawnRequest::realtime("claude_happy"))
        .await
        .expect("spawn");

    assert!(supervisor.is_running(&id).await);

    // Drive stop in a background task and concurrently poll is_running.
    // With the bug, is_running flips false the instant stop() removes
    // the entry, well before the JoinHandle resolves. With the fix, it
    // stays true throughout.
    let sup2 = Arc::clone(&supervisor);
    let id2 = id.clone();
    let stop_task = tokio::spawn(async move { sup2.stop(&id2).await });

    // Poll on a small delay, not yield_now — yield_now tight-loops the
    // scheduler and can land inside the supervise task's post-remove
    // pre-return tail (a microsecond-wide window that's not the bug).
    // The actual bug window — stop() synchronously removing on entry
    // — is hundreds of ms wide (the cancel + drain duration), so 2ms
    // polling catches it reliably without false positives.
    let mut saw_false_during_stop = false;
    loop {
        let running = supervisor.is_running(&id).await;
        if stop_task.is_finished() {
            break;
        }
        if !running {
            saw_false_during_stop = true;
            break;
        }
        tokio::time::sleep(Duration::from_millis(2)).await;
    }

    stop_task.await.expect("join stop task").expect("stop ok");
    assert!(
        !saw_false_during_stop,
        "is_running() returned false while stop() was still in flight (race window open)"
    );
    assert!(
        !supervisor.is_running(&id).await,
        "post-stop is_running must be false"
    );
}
