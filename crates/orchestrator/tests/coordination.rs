//! Step 9 — coordination tests.
//!
//! Verifies that:
//! 1. A 3-worker DAG with a fan-in dependency runs end-to-end:
//!    upstream tasks complete first, downstream task spawns only
//!    after both upstreams reach `done`.
//! 2. Retry-on-failure policy: a `gemini_failed` task with
//!    `max_attempts: 2` produces two attempts in sequence.
//! 3. Audit-r5 retry gate: a `gemini_terminal` task (Failure with
//!    `retryable: false`) must NOT spawn a retry even when
//!    RetryPolicy::OnFailure is set.

use event_schema::Event;
use orchestrator::{parser::WorkerEventSink, DispatchRequest, Repository, RetryPolicy, Supervisor};
use std::path::PathBuf;
use std::process::Command;
use std::sync::{Arc, Mutex, Once};
use std::time::{Duration, Instant};

static BUILD_ONCE: Once = Once::new();

fn ensure_mock_harness_built() -> PathBuf {
    BUILD_ONCE.call_once(|| {
        let out = Command::new(env!("CARGO"))
            .args(["build", "-p", "vigla-mock-harness", "--bin", "mock-harness"])
            .output()
            .expect("invoke cargo build");
        assert!(
            out.status.success(),
            "mock-harness build: {}",
            String::from_utf8_lossy(&out.stderr)
        );
    });
    let manifest_dir = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    manifest_dir
        .parent()
        .and_then(|p| p.parent())
        .unwrap()
        .join("target/debug/mock-harness")
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

async fn drain_quiescent(supervisor: &Arc<Supervisor>, deadline: Duration) {
    let start = Instant::now();
    while start.elapsed() < deadline {
        if supervisor.is_quiescent().await {
            return;
        }
        tokio::time::sleep(Duration::from_millis(25)).await;
    }
    panic!("supervisor did not reach quiescence within {deadline:?}");
}

#[tokio::test]
async fn dag_fan_in_runs_in_dependency_order() {
    let bin = ensure_mock_harness_built();
    let repo = Repository::open_in_memory().await.unwrap();
    let sink: Arc<CapturingSink> = Arc::new(CapturingSink::default());
    let supervisor = Supervisor::new(repo.clone(), Arc::clone(&sink) as _, bin);

    // Pre-allocate task IDs so we can express the DAG.
    let task_a = "task-a-claude".to_string();
    let task_b = "task-b-codex".to_string();
    let task_c = "task-c-codex".to_string();

    // Worker A: claude_happy, no deps.
    supervisor
        .dispatch(DispatchRequest {
            script: "claude_happy".into(),
            speed: 0.0,
            task_title: "step-9 A".into(),
            task_id: task_a.clone(),
            depends_on: vec![],
            retry: RetryPolicy::Never,
        })
        .await
        .unwrap();
    // Worker B: claude_happy as well, no deps. (We use claude_happy
    // for both upstreams to avoid the codex_blocked self-block making
    // assertions noisier — for this test we want clean uppstream
    // completion semantics, not vendor variety.)
    supervisor
        .dispatch(DispatchRequest {
            script: "claude_happy".into(),
            speed: 0.0,
            task_title: "step-9 B".into(),
            task_id: task_b.clone(),
            depends_on: vec![],
            retry: RetryPolicy::Never,
        })
        .await
        .unwrap();
    // Worker C: claude_happy, depends on A AND B.
    supervisor
        .dispatch(DispatchRequest {
            script: "claude_happy".into(),
            speed: 0.0,
            task_title: "step-9 C (fan-in)".into(),
            task_id: task_c.clone(),
            depends_on: vec![task_a.clone(), task_b.clone()],
            retry: RetryPolicy::Never,
        })
        .await
        .unwrap();

    drain_quiescent(&supervisor, Duration::from_secs(15)).await;

    // Identify worker IDs by replaying for each task.
    let events_a = repo.replay_for_task(&task_a).await.unwrap();
    let events_b = repo.replay_for_task(&task_b).await.unwrap();
    let events_c = repo.replay_for_task(&task_c).await.unwrap();
    assert!(!events_a.is_empty(), "task A produced no events");
    assert!(!events_b.is_empty(), "task B produced no events");
    assert!(!events_c.is_empty(), "task C produced no events");

    // Compare worker spawn order via the workers table — this is the
    // wall-clock authoritative axis. (The mock-harness's `ts` field
    // reflects scripted time which is FUTURE-dated relative to actual
    // emit time, so lexical ts comparison is unreliable for ordering.)
    let worker_a_id = events_a[0].worker_id.clone();
    let worker_b_id = events_b[0].worker_id.clone();
    let worker_c_id = events_c[0].worker_id.clone();

    // Use sqlx directly via repo? Simpler: assert C's worker has a
    // later "ended_at" than A's & B's (since A & B must have
    // completed before C spawned).
    use sqlx::Row;
    let pool = repo.pool_for_test();
    let a_ended: String = sqlx::query("SELECT ended_at FROM workers WHERE id = ?")
        .bind(&worker_a_id)
        .fetch_one(pool)
        .await
        .unwrap()
        .get(0);
    let b_ended: String = sqlx::query("SELECT ended_at FROM workers WHERE id = ?")
        .bind(&worker_b_id)
        .fetch_one(pool)
        .await
        .unwrap()
        .get(0);
    let c_spawned: String = sqlx::query("SELECT spawned_at FROM workers WHERE id = ?")
        .bind(&worker_c_id)
        .fetch_one(pool)
        .await
        .unwrap()
        .get(0);

    assert!(
        c_spawned >= a_ended,
        "C spawned at {c_spawned} but A ended at {a_ended} — fan-in violated"
    );
    assert!(
        c_spawned >= b_ended,
        "C spawned at {c_spawned} but B ended at {b_ended} — fan-in violated"
    );

    // Final state of each: completion event present.
    let last_a = events_a.last().unwrap();
    let last_b = events_b.last().unwrap();
    let last_c = events_c.last().unwrap();
    use event_schema::EventKind;
    assert!(matches!(last_a.kind, EventKind::Completion(_)));
    assert!(matches!(last_b.kind, EventKind::Completion(_)));
    assert!(matches!(last_c.kind, EventKind::Completion(_)));
}

#[tokio::test]
async fn retryable_failure_makes_a_second_attempt() {
    let bin = ensure_mock_harness_built();
    let repo = Repository::open_in_memory().await.unwrap();
    let sink: Arc<CapturingSink> = Arc::new(CapturingSink::default());
    let supervisor = Supervisor::new(repo.clone(), Arc::clone(&sink) as _, bin);

    let task_id = "retry-task".to_string();
    supervisor
        .dispatch(DispatchRequest {
            script: "gemini_failed".into(),
            speed: 0.0,
            task_title: "step-9 retry".into(),
            task_id: task_id.clone(),
            depends_on: vec![],
            retry: RetryPolicy::OnFailure {
                max_attempts: 2,
                base_ms: 50,
            },
        })
        .await
        .unwrap();

    drain_quiescent(&supervisor, Duration::from_secs(15)).await;

    // Both attempts share the same task_id — replay_for_task should
    // surface events from both worker_ids.
    let events = repo.replay_for_task(&task_id).await.unwrap();
    let unique_workers: std::collections::HashSet<&str> =
        events.iter().map(|e| e.worker_id.as_str()).collect();
    assert_eq!(
        unique_workers.len(),
        2,
        "expected 2 attempts (2 distinct worker_ids), got {} for task {task_id}",
        unique_workers.len()
    );

    use event_schema::EventKind;
    let failure_count = events
        .iter()
        .filter(|e| matches!(e.kind, EventKind::Failure(_)))
        .count();
    assert_eq!(
        failure_count, 2,
        "expected 2 failure events across attempts"
    );
}

/// Audit r5 polish — non-retryable failures must NOT spawn a second
/// attempt, even when the dispatch's RetryPolicy::OnFailure would
/// otherwise allow more attempts. The worker-emitted Failure event's
/// `retryable: false` flag is authoritative.
#[tokio::test]
async fn non_retryable_failure_does_not_retry_despite_retry_policy() {
    let bin = ensure_mock_harness_built();
    let repo = Repository::open_in_memory().await.unwrap();
    let sink: Arc<CapturingSink> = Arc::new(CapturingSink::default());
    let supervisor = Supervisor::new(repo.clone(), Arc::clone(&sink) as _, bin);

    let task_id = "non-retryable-task".to_string();
    supervisor
        .dispatch(DispatchRequest {
            // gemini_terminal emits Failure { retryable: false }.
            script: "gemini_terminal".into(),
            speed: 0.0,
            task_title: "audit-r5 retry gate".into(),
            task_id: task_id.clone(),
            depends_on: vec![],
            retry: RetryPolicy::OnFailure {
                max_attempts: 3,
                base_ms: 50,
            },
        })
        .await
        .unwrap();

    drain_quiescent(&supervisor, Duration::from_secs(15)).await;

    let events = repo.replay_for_task(&task_id).await.unwrap();
    let unique_workers: std::collections::HashSet<&str> =
        events.iter().map(|e| e.worker_id.as_str()).collect();
    assert_eq!(
        unique_workers.len(),
        1,
        "non-retryable failure must produce EXACTLY ONE attempt — got {} workers for task {task_id}",
        unique_workers.len()
    );

    use event_schema::EventKind;
    let failures: Vec<_> = events
        .iter()
        .filter_map(|e| {
            if let EventKind::Failure(f) = &e.kind {
                Some(f)
            } else {
                None
            }
        })
        .collect();
    assert_eq!(
        failures.len(),
        1,
        "expected exactly one Failure event, got {}",
        failures.len()
    );
    assert!(
        !failures[0].retryable,
        "the failure should be non-retryable; got {:?}",
        failures[0]
    );
}

#[tokio::test]
async fn terminal_failure_cancels_transitive_downstream_tasks() {
    let bin = ensure_mock_harness_built();
    let repo = Repository::open_in_memory().await.unwrap();
    let sink: Arc<CapturingSink> = Arc::new(CapturingSink::default());
    let supervisor = Supervisor::new(repo.clone(), Arc::clone(&sink) as _, bin);

    let upstream = "terminal-upstream".to_string();
    let middle = "blocked-middle".to_string();
    let leaf = "blocked-leaf".to_string();

    supervisor
        .dispatch(DispatchRequest {
            script: "gemini_terminal".into(),
            speed: 1.0,
            task_title: "terminal upstream".into(),
            task_id: upstream.clone(),
            depends_on: vec![],
            retry: RetryPolicy::Never,
        })
        .await
        .unwrap();
    supervisor
        .dispatch(DispatchRequest {
            script: "claude_happy".into(),
            speed: 0.0,
            task_title: "blocked middle".into(),
            task_id: middle.clone(),
            depends_on: vec![upstream],
            retry: RetryPolicy::Never,
        })
        .await
        .unwrap();
    supervisor
        .dispatch(DispatchRequest {
            script: "claude_happy".into(),
            speed: 0.0,
            task_title: "blocked leaf".into(),
            task_id: leaf.clone(),
            depends_on: vec![middle.clone()],
            retry: RetryPolicy::Never,
        })
        .await
        .unwrap();

    drain_quiescent(&supervisor, Duration::from_secs(3)).await;

    assert!(supervisor.task_failed(&middle).await);
    assert!(supervisor.task_failed(&leaf).await);
    assert!(
        repo.replay_for_task(&middle).await.unwrap().is_empty(),
        "a task with a failed dependency must never spawn"
    );
    assert!(
        repo.replay_for_task(&leaf).await.unwrap().is_empty(),
        "cancellation must propagate through the pending DAG"
    );

    let late = "late-dependent".to_string();
    supervisor
        .dispatch(DispatchRequest {
            script: "claude_happy".into(),
            speed: 0.0,
            task_title: "late dependent".into(),
            task_id: late.clone(),
            depends_on: vec!["terminal-upstream".into()],
            retry: RetryPolicy::Never,
        })
        .await
        .unwrap();
    assert!(supervisor.is_quiescent().await);
    assert!(supervisor.task_failed(&late).await);
    assert!(repo.replay_for_task(&late).await.unwrap().is_empty());
}
