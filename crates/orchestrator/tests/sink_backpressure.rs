//! Tests for CoordinatingSink + CoalescingSink backpressure behaviour.

use event_schema::{
    Event, EventKind, Log, LogLevel, LogStream, StateChange, Vendor, WorkerInfo, WorkerState,
};
use orchestrator::{parser::WorkerEventSink, CoalescingSink, Repository, Supervisor};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

/// Single serialisation point for all tests that mutate
/// `VIGLA_SINK_LOG_RATE`. env vars are process-global; multiple
/// tests mutating them in parallel produce non-deterministic reads.
/// Each env-mutating test must acquire this lock for its full duration.
static ENV_LOCK: Mutex<()> = Mutex::new(());

// ── helpers ──────────────────────────────────────────────────────────

fn worker(id: &str, name: &str) -> WorkerInfo {
    WorkerInfo {
        id: id.into(),
        name: name.into(),
        vendor: Vendor::Mock,
        cli_binary: "/usr/local/bin/mock-harness".into(),
        cli_version: None,
        cwd: "/tmp/work".into(),
        model: None,
        spawned_at: "2026-05-17T00:00:00.000Z".into(),
        ended_at: None,
    }
}

fn log_event(worker_id: &str, seq: u64) -> Event {
    Event {
        schema_version: "1.0".into(),
        worker_id: worker_id.into(),
        task_id: None,
        seq,
        ts: format!("2026-05-17T00:00:{:02}.000Z", seq % 60),
        kind: EventKind::Log(Log {
            level: LogLevel::Info,
            stream: LogStream::Stdout,
            line: format!("evt {seq}"),
            tag: None,
        }),
    }
}

fn state_change(worker_id: &str, seq: u64, state: WorkerState) -> Event {
    Event {
        schema_version: "1.0".into(),
        worker_id: worker_id.into(),
        task_id: None,
        seq,
        ts: format!("2026-05-17T00:00:{:02}.000Z", seq % 60),
        kind: EventKind::StateChange(StateChange {
            state,
            from: None,
            note: None,
        }),
    }
}

#[derive(Default)]
struct CountingSink {
    total: AtomicU64,
    logs: AtomicU64,
    state_changes: AtomicU64,
    captured: Mutex<Vec<Event>>,
}

impl WorkerEventSink for CountingSink {
    fn emit(&self, event: &Event) {
        self.total.fetch_add(1, Ordering::Relaxed);
        match &event.kind {
            EventKind::Log(_) => {
                self.logs.fetch_add(1, Ordering::Relaxed);
            }
            EventKind::StateChange(_) => {
                self.state_changes.fetch_add(1, Ordering::Relaxed);
            }
            _ => {}
        }
        self.captured.lock().unwrap().push(event.clone());
    }
}

fn mock_harness_for_test() -> std::path::PathBuf {
    std::path::PathBuf::from("/tmp/vigla-tests/mock-harness-does-not-exist")
}

// ── CoordinatingSink tests ───────────────────────────────────────────

#[tokio::test]
async fn coordinating_sink_forwards_critical_events_to_inner() {
    let repo = Repository::open_in_memory().await.unwrap();
    let counting: Arc<CountingSink> = Arc::new(CountingSink::default());
    let supervisor = Supervisor::new(
        repo,
        counting.clone() as Arc<dyn WorkerEventSink>,
        mock_harness_for_test(),
    );
    let sink = supervisor.coordinating_sink_for_test();

    sink.emit(&log_event("w1", 0));
    sink.emit(&state_change("w1", 1, WorkerState::Executing));
    sink.emit(&state_change("w1", 2, WorkerState::Done));

    assert_eq!(counting.total.load(Ordering::Relaxed), 3);
    assert_eq!(counting.logs.load(Ordering::Relaxed), 1);
    assert_eq!(counting.state_changes.load(Ordering::Relaxed), 2);
}

#[tokio::test]
async fn coordinating_sink_filters_noisy_events_from_coord_queue() {
    let repo = Repository::open_in_memory().await.unwrap();
    let counting: Arc<CountingSink> = Arc::new(CountingSink::default());
    let supervisor = Supervisor::new(
        repo,
        counting.clone() as Arc<dyn WorkerEventSink>,
        mock_harness_for_test(),
    );
    let sink = supervisor.coordinating_sink_for_test();

    for seq in 0..1000u64 {
        sink.emit(&log_event("w1", seq));
    }

    tokio::time::sleep(Duration::from_millis(50)).await;

    assert_eq!(sink.dropped_coordination_events_for_test(), 0);
    assert_eq!(counting.logs.load(Ordering::Relaxed), 1000);
}

#[tokio::test]
async fn coordinating_sink_bounded_queue_under_fast_consumer() {
    // Happy-path test: under realistic state_change load (tens to
    // low hundreds of events per worker lifetime — production rate),
    // the bounded(256) queue accommodates the burst without dropping
    // and the consumer drains everything to the inner sink. The
    // adverse case where the queue actually fills and the drop path
    // fires is covered separately by
    // `coordinating_sink_drops_under_sustained_slow_consumer` in
    // Task 2 — that test deliberately overwhelms the consumer with
    // an unrealistic 50_000-event burst to prove the bound holds and
    // drops are observable.
    let repo = Repository::open_in_memory().await.unwrap();
    let counting: Arc<CountingSink> = Arc::new(CountingSink::default());
    let supervisor = Supervisor::new(
        repo,
        counting.clone() as Arc<dyn WorkerEventSink>,
        mock_harness_for_test(),
    );
    let sink = supervisor.coordinating_sink_for_test();

    supervisor
        .repo_for_test()
        .insert_worker(&worker("w1", "test"))
        .await
        .unwrap();

    for seq in 0..200u64 {
        sink.emit(&state_change("w1", seq, WorkerState::Executing));
    }

    tokio::time::sleep(Duration::from_millis(500)).await;

    assert_eq!(
        sink.dropped_coordination_events_for_test(),
        0,
        "production-rate burst should not trigger drops"
    );
    assert_eq!(counting.state_changes.load(Ordering::Relaxed), 200);
}

/// Drop path under sustained storm. Proves the bounded-queue
/// contract: drops occur (drop_count > 0) under a 50k-event burst
/// because each StateChange triggers a SQLite UPDATE in the
/// consumer task, so the consumer drains at hundreds-per-second
/// while the synchronous emit loop pushes thousands in microseconds.
/// The 10ms blocking-send timeout fires, events are dropped, drop
/// counter increments. Companion to
/// `coordinating_sink_bounded_queue_under_fast_consumer` which
/// proves the no-drop happy path at realistic load.
///
/// The inner sink is still called for EVERY event regardless of
/// coordination drops — `emit()` forwards to inner before any
/// filter/queue logic, so the user-visible event flow is never
/// affected by coordination-queue pressure.
#[tokio::test]
async fn coordinating_sink_drops_under_sustained_slow_consumer() {
    let repo = Repository::open_in_memory().await.unwrap();
    let counting: Arc<CountingSink> = Arc::new(CountingSink::default());
    let supervisor = Supervisor::new(
        repo,
        counting.clone() as Arc<dyn WorkerEventSink>,
        mock_harness_for_test(),
    );
    let sink = supervisor.coordinating_sink_for_test();

    supervisor
        .repo_for_test()
        .insert_worker(&worker("w1", "test"))
        .await
        .unwrap();

    for seq in 0..50_000u64 {
        sink.emit(&state_change("w1", seq, WorkerState::Executing));
    }

    // Let the runtime drain whatever did fit. The remaining events
    // either landed in spawned blocking-send tasks (which may still
    // be racing for slots) or hit the timeout and were dropped.
    tokio::time::sleep(Duration::from_millis(500)).await;

    let drops = sink.dropped_coordination_events_for_test();
    assert!(
        drops > 0,
        "drop path should fire under sustained storm; got drops={drops}"
    );
    assert_eq!(
        counting.state_changes.load(Ordering::Relaxed),
        50_000,
        "inner sink should see every event regardless of coordination drops"
    );
}

/// Terminal coordination is structural: losing it can strand a DAG forever.
/// Even while the bounded nonterminal lane is saturated, completion must reach
/// the supervisor through the dedicated lossless lane.
#[tokio::test]
async fn coordinating_sink_never_drops_completion_under_saturation() {
    let repo = Repository::open_in_memory().await.unwrap();
    let counting: Arc<CountingSink> = Arc::new(CountingSink::default());
    let supervisor = Supervisor::new(
        repo.clone(),
        counting.clone() as Arc<dyn WorkerEventSink>,
        mock_harness_for_test(),
    );
    let sink = supervisor.coordinating_sink_for_test();

    repo.insert_worker(&worker("w1", "test")).await.unwrap();

    for seq in 0..50_000u64 {
        sink.emit(&state_change("w1", seq, WorkerState::Executing));
    }
    sink.emit(&completion_event("w1", 50_001));

    tokio::time::timeout(Duration::from_secs(2), async {
        while !supervisor.task_completed("t1").await {
            tokio::task::yield_now().await;
        }
    })
    .await
    .expect("completion must survive coordination queue saturation");

    assert!(
        sink.dropped_coordination_events_for_test() > 0,
        "the test must actually saturate the bounded nonterminal lane"
    );
    assert_eq!(
        counting.total.load(Ordering::Relaxed),
        50_001,
        "the user-visible sink still receives every event"
    );
}

// ── CoalescingSink tests ─────────────────────────────────────────────

fn cost_event(worker_id: &str, seq: u64, usd: f64) -> Event {
    Event {
        schema_version: "1.0".into(),
        worker_id: worker_id.into(),
        task_id: None,
        seq,
        ts: format!("2026-05-17T00:00:{:02}.000Z", seq % 60),
        kind: EventKind::Cost(event_schema::Cost {
            usd,
            input_tokens: 0,
            output_tokens: 0,
            cache_read_tokens: None,
            cache_write_tokens: None,
            model: None,
        }),
    }
}

fn completion_event(worker_id: &str, seq: u64) -> Event {
    Event {
        schema_version: "1.0".into(),
        worker_id: worker_id.into(),
        task_id: Some("t1".into()),
        seq,
        ts: format!("2026-05-17T00:00:{:02}.000Z", seq % 60),
        kind: EventKind::Completion(event_schema::Completion {
            summary: "done".into(),
            artifacts: None,
            duration_ms: None,
        }),
    }
}

#[tokio::test]
async fn coalescing_sink_passes_critical_events_immediately() {
    let counting: Arc<CountingSink> = Arc::new(CountingSink::default());
    let sink = CoalescingSink::new(counting.clone() as Arc<dyn WorkerEventSink>);

    sink.emit(&state_change("w1", 0, WorkerState::Executing));
    sink.emit(&cost_event("w1", 1, 0.0042));
    sink.emit(&completion_event("w1", 2));

    // All three forwarded WITHOUT waiting for any flush tick.
    assert_eq!(counting.total.load(Ordering::Relaxed), 3);
}

#[tokio::test]
async fn coalescing_sink_rate_limits_log_events() {
    // SAFETY: env vars are process-global; serialized via the
    // module-level ENV_LOCK shared by all env-mutating tests.
    let _guard = ENV_LOCK.lock().unwrap();
    unsafe {
        std::env::set_var("VIGLA_SINK_LOG_RATE", "20");
    }

    let counting: Arc<CountingSink> = Arc::new(CountingSink::default());
    let sink = CoalescingSink::new(counting.clone() as Arc<dyn WorkerEventSink>);

    // Burst 200 log events for one worker.
    for seq in 0..200u64 {
        sink.emit(&log_event("w1", seq));
    }

    let forwarded = counting.logs.load(Ordering::Relaxed);
    // We should have forwarded EXACTLY 20 (the initial token bucket).
    assert_eq!(
        forwarded, 20,
        "expected exactly 20 forwarded logs, got {forwarded}"
    );

    unsafe {
        std::env::remove_var("VIGLA_SINK_LOG_RATE");
    }
}

fn progress_event(worker_id: &str, task_id: Option<&str>, seq: u64, percent: f32) -> Event {
    Event {
        schema_version: "1.0".into(),
        worker_id: worker_id.into(),
        task_id: task_id.map(Into::into),
        seq,
        ts: format!("2026-05-17T00:00:{:02}.000Z", seq % 60),
        kind: EventKind::Progress(event_schema::Progress {
            percent: percent as f64,
            eta_ms: None,
            note: None,
        }),
    }
}

#[tokio::test]
async fn coalescing_sink_keeps_latest_progress_per_worker_task() {
    let counting: Arc<CountingSink> = Arc::new(CountingSink::default());
    let sink = CoalescingSink::new(counting.clone() as Arc<dyn WorkerEventSink>);

    // 100 progress updates with monotonically-increasing percent.
    for seq in 0..100u64 {
        sink.emit(&progress_event("w1", Some("t1"), seq, seq as f32));
    }

    // Wait for one flush tick (100ms interval + buffer).
    tokio::time::sleep(Duration::from_millis(150)).await;

    // Exactly one progress event delivered, percent = 99.
    let captured = counting.captured.lock().unwrap();
    let progresses: Vec<&Event> = captured
        .iter()
        .filter(|e| matches!(&e.kind, EventKind::Progress(_)))
        .collect();
    assert_eq!(
        progresses.len(),
        1,
        "expected 1 coalesced progress, got {}",
        progresses.len()
    );
    if let EventKind::Progress(p) = &progresses[0].kind {
        assert!(
            (p.percent - 99.0).abs() < 0.01,
            "expected percent=99, got {}",
            p.percent
        );
    } else {
        panic!("not a progress event");
    }
}

#[tokio::test]
async fn coalescing_sink_separates_progress_by_task() {
    let counting: Arc<CountingSink> = Arc::new(CountingSink::default());
    let sink = CoalescingSink::new(counting.clone() as Arc<dyn WorkerEventSink>);

    for seq in 0..10u64 {
        sink.emit(&progress_event("w1", Some("t1"), seq, seq as f32));
        sink.emit(&progress_event("w1", Some("t2"), seq + 100, seq as f32));
    }

    tokio::time::sleep(Duration::from_millis(150)).await;

    let captured = counting.captured.lock().unwrap();
    let progresses: Vec<&Event> = captured
        .iter()
        .filter(|e| matches!(&e.kind, EventKind::Progress(_)))
        .collect();
    assert_eq!(progresses.len(), 2, "expected 2 (one per task)");
}

#[tokio::test]
#[allow(clippy::await_holding_lock)] // ENV_LOCK serializes env-var access across tests; holding through the sleep is required to keep other env-mutating tests from racing during the 1s flush window.
async fn coalescing_sink_emits_drop_summary_after_rate_limit() {
    let _guard = ENV_LOCK.lock().unwrap();
    unsafe {
        std::env::set_var("VIGLA_SINK_LOG_RATE", "20");
    }
    let counting: Arc<CountingSink> = Arc::new(CountingSink::default());
    let sink = CoalescingSink::new(counting.clone() as Arc<dyn WorkerEventSink>);

    for seq in 0..200u64 {
        sink.emit(&log_event("w1", seq));
    }

    // Wait for the 1s summary flush.
    tokio::time::sleep(Duration::from_millis(1_100)).await;

    let captured = counting.captured.lock().unwrap();
    let summaries: Vec<&Event> = captured
        .iter()
        .filter(|e| {
            matches!(
                &e.kind,
                EventKind::Log(log) if log.tag.as_deref() == Some("vigla:rate-limit")
            )
        })
        .collect();
    assert_eq!(summaries.len(), 1, "expected exactly one drop summary");
    if let EventKind::Log(log) = &summaries[0].kind {
        assert!(
            log.line.contains("180 log events dropped"),
            "summary line was: {}",
            log.line
        );
    }

    unsafe {
        std::env::remove_var("VIGLA_SINK_LOG_RATE");
    }
}

#[tokio::test]
async fn coalescing_sink_drop_aborts_flush_task() {
    // The flush_handle is aborted in Drop. Stash a progress event but
    // DON'T wait for the 100ms flush tick — instead drop the sink
    // immediately. After waiting longer than a flush tick would have
    // taken, assert the inner sink received zero progress events.
    let counting: Arc<CountingSink> = Arc::new(CountingSink::default());
    let sink = CoalescingSink::new(counting.clone() as Arc<dyn WorkerEventSink>);

    sink.emit(&progress_event("w1", Some("t1"), 0, 42.0));
    drop(sink);

    // Wait long enough that the flush would have fired if not aborted.
    tokio::time::sleep(Duration::from_millis(200)).await;

    let captured = counting.captured.lock().unwrap();
    let progresses: Vec<&Event> = captured
        .iter()
        .filter(|e| matches!(&e.kind, EventKind::Progress(_)))
        .collect();
    assert_eq!(
        progresses.len(),
        0,
        "no progress events should reach inner after sink is dropped"
    );
}

#[tokio::test]
async fn coalescing_sink_env_override_changes_log_rate() {
    let _guard = ENV_LOCK.lock().unwrap();
    unsafe {
        std::env::set_var("VIGLA_SINK_LOG_RATE", "5");
    }
    let counting: Arc<CountingSink> = Arc::new(CountingSink::default());
    let sink = CoalescingSink::new(counting.clone() as Arc<dyn WorkerEventSink>);

    for seq in 0..50u64 {
        sink.emit(&log_event("w1", seq));
    }

    let forwarded = counting.logs.load(Ordering::Relaxed);
    assert_eq!(
        forwarded, 5,
        "expected exactly 5 with env=5, got {forwarded}"
    );

    unsafe {
        std::env::remove_var("VIGLA_SINK_LOG_RATE");
    }
}

#[tokio::test]
async fn storm_does_not_blow_memory() {
    // End-to-end: route 100k log events for one worker through
    // CoordinatingSink → CoalescingSink → CountingSink. With the
    // default 20 log/s cap, the inner sink should see roughly
    // 20 + a few summary events, not 100k.
    let repo = Repository::open_in_memory().await.unwrap();
    let counting: Arc<CountingSink> = Arc::new(CountingSink::default());
    let supervisor = Supervisor::new(
        repo,
        counting.clone() as Arc<dyn WorkerEventSink>,
        mock_harness_for_test(),
    );
    let sink = supervisor.coordinating_sink_for_test_full();

    for seq in 0..100_000u64 {
        sink.emit(&log_event("w1", seq));
    }

    // Allow at most one summary flush.
    tokio::time::sleep(Duration::from_millis(1_100)).await;

    let forwarded = counting.total.load(Ordering::Relaxed);
    assert!(
        forwarded < 100,
        "expected \u{ab} 100 forwarded events after rate-limit; got {forwarded}"
    );
    assert!(forwarded > 0, "rate limit allowed nothing through");
}
