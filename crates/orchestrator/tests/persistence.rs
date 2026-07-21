//! Step 3 — persistence tests.
//!
//! Verifies the [`orchestrator::Repository`] surface against the
//! `docs/event-schema.md` contract: insert, replay axes, idempotence,
//! payload opacity, and survival across an "app restart" (drop + re-
//! open of the same SQLite file).

use event_schema::{
    Artifact, ArtifactKind, Completion, Cost, Dependency, Event, EventKind, Failure,
    FailureCategory, FileActivity, FileOp, Log, LogLevel, LogStream, Progress, StateChange,
    TaskInfo, TestFailure, TestResult, Vendor, WorkerInfo, WorkerState,
};
use orchestrator::{Repository, RetentionGuard};
use sqlx::Row;
use tempfile::tempdir;

fn worker(id: &str, name: &str) -> WorkerInfo {
    WorkerInfo {
        id: id.into(),
        name: name.into(),
        vendor: Vendor::Mock,
        cli_binary: "/usr/local/bin/mock-harness".into(),
        cli_version: Some("0.0.1".into()),
        cwd: "/tmp/work".into(),
        model: None,
        spawned_at: "2026-05-08T19:42:13.000Z".into(),
        ended_at: None,
    }
}

fn task(id: &str, title: &str) -> TaskInfo {
    TaskInfo {
        id: id.into(),
        parent_id: None,
        title: title.into(),
        depends_on: vec![],
        created_at: "2026-05-08T19:42:00.000Z".into(),
    }
}

fn state_change_event(
    worker_id: &str,
    task_id: Option<&str>,
    seq: u64,
    state: WorkerState,
) -> Event {
    Event {
        schema_version: "1.0".into(),
        worker_id: worker_id.into(),
        task_id: task_id.map(Into::into),
        seq,
        ts: format!("2026-05-08T19:43:{:02}.000Z", seq % 60),
        kind: EventKind::StateChange(StateChange {
            state,
            from: None,
            note: None,
        }),
    }
}

#[tokio::test]
async fn in_memory_insert_and_replay_by_worker() {
    let repo = Repository::open_in_memory().await.unwrap();

    repo.insert_worker(&worker("w1", "claude-1")).await.unwrap();
    repo.insert_task(&task("t1", "Add retry to fetcher"))
        .await
        .unwrap();

    for seq in 0..50u64 {
        let state = if seq % 5 == 0 {
            WorkerState::Planning
        } else {
            WorkerState::Executing
        };
        repo.insert_event(&state_change_event("w1", Some("t1"), seq, state))
            .await
            .unwrap();
    }

    let events = repo.replay_for_worker("w1").await.unwrap();
    assert_eq!(events.len(), 50);
    for (i, e) in events.iter().enumerate() {
        assert_eq!(e.seq, i as u64, "events must replay in seq order");
        assert_eq!(e.worker_id, "w1");
    }
}

#[tokio::test]
async fn replay_by_task_orders_by_ts() {
    let repo = Repository::open_in_memory().await.unwrap();

    // Two workers acting on the same task; their seqs are independent
    // but their timestamps interleave. replay_for_task must order by ts.
    repo.insert_worker(&worker("w-a", "claude")).await.unwrap();
    repo.insert_worker(&worker("w-b", "codex")).await.unwrap();
    repo.insert_task(&task("shared", "fix migration"))
        .await
        .unwrap();

    let mut e1 = state_change_event("w-a", Some("shared"), 0, WorkerState::Planning);
    e1.ts = "2026-01-01T00:00:01.000Z".into();
    let mut e2 = state_change_event("w-b", Some("shared"), 0, WorkerState::Executing);
    e2.ts = "2026-01-01T00:00:02.000Z".into();
    let mut e3 = state_change_event("w-a", Some("shared"), 1, WorkerState::Done);
    e3.ts = "2026-01-01T00:00:03.000Z".into();

    repo.insert_event(&e3).await.unwrap();
    repo.insert_event(&e1).await.unwrap();
    repo.insert_event(&e2).await.unwrap();

    let replay = repo.replay_for_task("shared").await.unwrap();
    let timestamps: Vec<&str> = replay.iter().map(|e| e.ts.as_str()).collect();
    assert_eq!(
        timestamps,
        vec![
            "2026-01-01T00:00:01.000Z",
            "2026-01-01T00:00:02.000Z",
            "2026-01-01T00:00:03.000Z",
        ]
    );
}

#[tokio::test]
async fn replay_by_task_keeps_both_events_on_ts_seq_collision() {
    // F-11: seq is per-worker (events PK is (worker_id, seq)), so two
    // workers on the same task can emit events with an identical (ts, seq).
    // A (ts, seq)-only cursor skips one of them at a page boundary; the
    // worker_id tiebreaker prevents that.
    let repo = Repository::open_in_memory().await.unwrap();
    repo.insert_worker(&worker("w-a", "claude")).await.unwrap();
    repo.insert_worker(&worker("w-b", "codex")).await.unwrap();
    repo.insert_task(&task("shared", "collide")).await.unwrap();

    let ts = "2026-01-01T00:00:01.000Z";
    let mut ea = state_change_event("w-a", Some("shared"), 0, WorkerState::Executing);
    ea.ts = ts.into();
    let mut eb = state_change_event("w-b", Some("shared"), 0, WorkerState::Executing);
    eb.ts = ts.into(); // identical (ts, seq) across the two workers
    repo.insert_event(&ea).await.unwrap();
    repo.insert_event(&eb).await.unwrap();

    // Page size 1 forces the cursor to advance across the colliding pair.
    let page1 = repo
        .replay_for_task_page("shared", None, None, None, 1)
        .await
        .unwrap();
    assert_eq!(page1.len(), 1);
    let last = &page1[0];

    let page2 = repo
        .replay_for_task_page(
            "shared",
            Some(last.seq),
            Some(&last.ts),
            Some(&last.worker_id),
            1,
        )
        .await
        .unwrap();
    assert_eq!(
        page2.len(),
        1,
        "the second worker's (ts, seq)-colliding event must not be skipped"
    );
    assert_ne!(
        page2[0].worker_id, last.worker_id,
        "page 2 must surface the OTHER worker's colliding event"
    );

    // The full wrapper returns both, none dropped.
    let all = repo.replay_for_task("shared").await.unwrap();
    assert_eq!(all.len(), 2);
}

#[tokio::test]
async fn payload_round_trips_for_all_event_types() {
    let repo = Repository::open_in_memory().await.unwrap();
    repo.insert_worker(&worker("w1", "mock-1")).await.unwrap();

    let originals: Vec<Event> = vec![
        Event {
            schema_version: "1.0".into(),
            worker_id: "w1".into(),
            task_id: Some("t1".into()),
            seq: 1,
            ts: "2026-01-01T00:00:01.000Z".into(),
            kind: EventKind::StateChange(StateChange {
                state: WorkerState::Executing,
                from: Some(WorkerState::Planning),
                note: Some("note".into()),
            }),
        },
        Event {
            schema_version: "1.0".into(),
            worker_id: "w1".into(),
            task_id: Some("t1".into()),
            seq: 2,
            ts: "2026-01-01T00:00:02.000Z".into(),
            kind: EventKind::Log(Log {
                level: LogLevel::Info,
                stream: LogStream::Stdout,
                line: "[32m✓[0m read 4 files".into(),
                tag: Some("fs".into()),
            }),
        },
        Event {
            schema_version: "1.0".into(),
            worker_id: "w1".into(),
            task_id: Some("t1".into()),
            seq: 3,
            ts: "2026-01-01T00:00:03.000Z".into(),
            kind: EventKind::Progress(Progress {
                percent: 42.5,
                eta_ms: Some(18000),
                note: None,
            }),
        },
        Event {
            schema_version: "1.0".into(),
            worker_id: "w1".into(),
            task_id: Some("t1".into()),
            seq: 4,
            ts: "2026-01-01T00:00:04.000Z".into(),
            kind: EventKind::FileActivity(FileActivity {
                path: "src/x.ts".into(),
                op: FileOp::Modified,
                from_path: None,
                lines_added: Some(12),
                lines_removed: Some(4),
                bytes: None,
            }),
        },
        Event {
            schema_version: "1.0".into(),
            worker_id: "w1".into(),
            task_id: Some("t1".into()),
            seq: 5,
            ts: "2026-01-01T00:00:05.000Z".into(),
            kind: EventKind::TestResult(TestResult {
                suite: "vitest".into(),
                passed: 18,
                failed: 1,
                skipped: 0,
                duration_ms: 1230,
                failures: Some(vec![TestFailure {
                    name: "x > y".into(),
                    message: "got 2".into(),
                    file: None,
                    line: None,
                }]),
            }),
        },
        Event {
            schema_version: "1.0".into(),
            worker_id: "w1".into(),
            task_id: Some("t1".into()),
            seq: 6,
            ts: "2026-01-01T00:00:06.000Z".into(),
            kind: EventKind::Cost(Cost {
                input_tokens: 4210,
                output_tokens: 980,
                usd: 0.0186,
                cache_read_tokens: Some(11200),
                cache_write_tokens: None,
                model: Some("claude-opus-4-7".into()),
            }),
        },
        Event {
            schema_version: "1.0".into(),
            worker_id: "w1".into(),
            task_id: Some("t1".into()),
            seq: 7,
            ts: "2026-01-01T00:00:07.000Z".into(),
            kind: EventKind::Dependency(Dependency {
                waiting_on: vec!["other-task".into()],
                reason: "needs migration".into(),
                since: None,
            }),
        },
        Event {
            schema_version: "1.0".into(),
            worker_id: "w1".into(),
            task_id: Some("t1".into()),
            seq: 8,
            ts: "2026-01-01T00:00:08.000Z".into(),
            kind: EventKind::Completion(Completion {
                summary: "done".into(),
                artifacts: Some(vec![Artifact {
                    kind: ArtifactKind::File,
                    artifact_ref: "src/x.ts".into(),
                    label: Some("patched".into()),
                }]),
                duration_ms: Some(198400),
            }),
        },
        Event {
            schema_version: "1.0".into(),
            worker_id: "w1".into(),
            task_id: Some("t1".into()),
            seq: 9,
            ts: "2026-01-01T00:00:09.000Z".into(),
            kind: EventKind::Failure(Failure {
                error: "exit 1".into(),
                retryable: true,
                suggestion: None,
                exit_code: Some(1),
                category: Some(FailureCategory::TaskLogic),
            }),
        },
    ];

    for e in &originals {
        repo.insert_event(e).await.unwrap();
    }

    let replayed = repo.replay_for_worker("w1").await.unwrap();
    assert_eq!(replayed.len(), originals.len());
    for (orig, got) in originals.iter().zip(replayed.iter()) {
        assert_eq!(orig, got, "round-trip mismatch for seq {}", orig.seq);
    }
}

#[tokio::test]
async fn cost_event_updates_worker_model() {
    let repo = Repository::open_in_memory().await.unwrap();
    repo.insert_worker(&worker("w1", "mock")).await.unwrap();

    repo.insert_event(&Event {
        schema_version: "1.0".into(),
        worker_id: "w1".into(),
        task_id: None,
        seq: 1,
        ts: "2026-01-01T00:00:01.000Z".into(),
        kind: EventKind::Cost(Cost {
            input_tokens: 100,
            output_tokens: 50,
            usd: 0.01,
            cache_read_tokens: None,
            cache_write_tokens: None,
            model: Some("claude-sonnet-4-5".into()),
        }),
    })
    .await
    .unwrap();

    let worker = repo.get_worker_info_by_id("w1").await.unwrap();
    assert_eq!(worker.model.as_deref(), Some("claude-sonnet-4-5"));
}

#[tokio::test]
async fn duplicate_seq_is_idempotent() {
    let repo = Repository::open_in_memory().await.unwrap();
    repo.insert_worker(&worker("w1", "mock")).await.unwrap();

    let evt = state_change_event("w1", None, 42, WorkerState::Idle);
    repo.insert_event(&evt).await.unwrap();
    // Same primary key (worker_id, seq) — must not error and must not
    // create a second row.
    repo.insert_event(&evt).await.unwrap();

    let replay = repo.replay_for_worker("w1").await.unwrap();
    assert_eq!(replay.len(), 1);
}

#[tokio::test]
async fn duplicate_seq_returns_duplicate_skipped_outcome() {
    use orchestrator::InsertOutcome;
    let repo = Repository::open_in_memory().await.unwrap();
    repo.insert_worker(&worker("w1", "mock")).await.unwrap();

    let evt = state_change_event("w1", None, 7, WorkerState::Idle);
    let first = repo.insert_event(&evt).await.unwrap();
    let second = repo.insert_event(&evt).await.unwrap();

    // The previous "INSERT OR IGNORE" silently dropped duplicates;
    // schema §6 requires we surface the regression. Callers (parser,
    // supervisor) use this signal to skip re-emitting to the UI.
    assert_eq!(first, InsertOutcome::Inserted);
    assert_eq!(second, InsertOutcome::DuplicateSkipped);
}

#[tokio::test]
async fn survives_restart() {
    let dir = tempdir().unwrap();
    let path = dir.path().join("vigla.sqlite");

    // Open, insert, drop pool.
    {
        let repo = Repository::open(&path).await.unwrap();
        repo.insert_worker(&worker("w1", "claude")).await.unwrap();
        repo.insert_task(&task("t1", "task")).await.unwrap();
        for seq in 0..20u64 {
            repo.insert_event(&state_change_event(
                "w1",
                Some("t1"),
                seq,
                WorkerState::Executing,
            ))
            .await
            .unwrap();
        }
        // pool closed when `repo` drops at end of scope
    }

    // Re-open the same file. Migrations re-run idempotently.
    let repo2 = Repository::open(&path).await.unwrap();
    let events = repo2.replay_for_worker("w1").await.unwrap();
    assert_eq!(events.len(), 20);
    for (i, e) in events.iter().enumerate() {
        assert_eq!(e.seq, i as u64);
    }
}

#[tokio::test]
async fn replay_tolerates_unknown_event_types_per_schema_section_6() {
    // Schema §6 (docs/event-schema.md) requires consumers to
    // "tolerate unknown event types (persist them; render a generic
    // 'unknown event' placeholder; never crash)". Prior to the fix
    // row_to_event ran serde_json::from_value::<Event>() on every
    // row, so a single unknown type in the persisted log made
    // replay_for_worker error out entirely.
    let repo = Repository::open_in_memory().await.unwrap();
    repo.insert_worker(&worker("w1", "claude")).await.unwrap();

    // Real, known event at seq=0.
    repo.insert_event(&state_change_event("w1", None, 0, WorkerState::Executing))
        .await
        .unwrap();
    // Future / unknown event type at seq=1, persisted via insert_event_raw.
    repo.insert_event_raw(
        "w1",
        None,
        1,
        "2026-05-08T19:43:01.000Z",
        "tool_call",
        r#"{"name":"bash","args":["ls"]}"#,
        "1.1",
    )
    .await
    .unwrap();
    // Another known event at seq=2 to confirm replay continues past
    // the unknown row instead of bailing.
    repo.insert_event(&state_change_event("w1", None, 2, WorkerState::Done))
        .await
        .unwrap();

    let events = repo.replay_for_worker("w1").await.unwrap();
    assert_eq!(events.len(), 3);

    // Unknown row materialises as a placeholder Log event tagged
    // `unknown:{type}` — schema §6's "generic 'unknown event'
    // placeholder".
    let placeholder = &events[1];
    assert_eq!(placeholder.seq, 1);
    let EventKind::Log(log) = &placeholder.kind else {
        panic!(
            "expected unknown row to become EventKind::Log, got {:?}",
            placeholder.kind
        );
    };
    assert_eq!(log.tag.as_deref(), Some("unknown:tool_call"));
    assert!(
        log.line.contains("\"name\":\"bash\""),
        "raw payload should be preserved in the log line: {}",
        log.line
    );

    // Known events on either side survive untouched.
    assert!(matches!(events[0].kind, EventKind::StateChange(_)));
    assert!(matches!(events[2].kind, EventKind::StateChange(_)));
}

/// Audit r5 — supervisor's spawn paths previously inserted the worker
/// row and the task row in two separate sqlx calls. A failure on the
/// task insert (e.g. duplicate PK on a retry mis-fire, disk full mid-
/// transaction) left an orphan worker row visible to
/// `list_recent_workers`. With `insert_worker_and_task` running both
/// inserts in a single transaction, a task-insert failure rolls back
/// the worker insert.
#[tokio::test]
async fn insert_worker_and_task_rolls_back_on_task_failure() {
    let repo = Repository::open_in_memory().await.unwrap();

    // Pre-insert a task so the second insert in the transaction will
    // hit the primary-key constraint.
    let occupant = task("task-occupied", "first owner");
    repo.insert_task(&occupant).await.unwrap();

    let new_worker = worker("worker-doomed", "doomed");
    let dup_task = task("task-occupied", "duplicate");

    let res = repo.insert_worker_and_task(&new_worker, &dup_task).await;
    assert!(
        res.is_err(),
        "expected duplicate-task insert to error, got {res:?}"
    );

    let workers = repo.list_recent_workers(10).await.unwrap();
    assert!(
        !workers.iter().any(|w| w.id == "worker-doomed"),
        "worker insert was not rolled back: {workers:?}"
    );
}

/// Happy-path companion — both rows persisted atomically when neither
/// insert errors.
#[tokio::test]
async fn insert_worker_and_task_persists_both_on_success() {
    let repo = Repository::open_in_memory().await.unwrap();
    let new_worker = worker("worker-ok", "ok");
    let new_task = task("task-ok", "ok");

    repo.insert_worker_and_task(&new_worker, &new_task)
        .await
        .unwrap();

    let workers = repo.list_recent_workers(10).await.unwrap();
    assert!(workers.iter().any(|w| w.id == "worker-ok"));
    // Confirm the task row exists by inserting a duplicate (which
    // should fail) — `Repository` doesn't yet expose a list_tasks API.
    let dup = task("task-ok", "dup");
    let res = repo.insert_task(&dup).await;
    assert!(res.is_err(), "task row should already exist: {res:?}");
}

/// Schema 2.0 removed `Vendor::Aider`, but a user upgrading from 1.x
/// still has historical rows with `vendor = "aider"` in their local
/// SQLite. The history view (`list_recent_workers`) must not fail
/// wholesale on such rows — it must skip them and return the rows it
/// can decode. Regression test for the "row corrupt: unknown vendor
/// aider" error the user reported when clicking history after the 2.0
/// bump.
#[tokio::test]
async fn list_recent_workers_skips_unknown_vendor_rows() {
    let dir = tempdir().unwrap();
    let path = dir.path().join("history.sqlite");
    let repo = Repository::open(&path).await.unwrap();

    // Valid row via the public API.
    let ok = worker("w-ok", "ok-name");
    repo.insert_worker(&ok).await.unwrap();

    // Inject a legacy row directly — the public API would reject
    // `vendor = "aider"` because the enum no longer accepts it.
    let raw_pool = sqlx::SqlitePool::connect(&format!("sqlite://{}", path.display()))
        .await
        .unwrap();
    sqlx::query(
        "INSERT INTO workers \
         (id, name, vendor, cli_binary, cli_version, cwd, model, spawned_at, ended_at) \
         VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?)",
    )
    .bind("w-legacy")
    .bind("legacy-aider")
    .bind("aider")
    .bind("/usr/local/bin/aider")
    .bind(Option::<&str>::None)
    .bind("/tmp/work")
    .bind(Option::<&str>::None)
    .bind("2026-05-01T00:00:00.000Z")
    .bind(Option::<&str>::None)
    .execute(&raw_pool)
    .await
    .unwrap();
    raw_pool.close().await;

    // history view must succeed and surface only the decodable row.
    let workers = repo.list_recent_workers(10).await.unwrap();
    assert_eq!(workers.len(), 1, "expected 1 worker, got {workers:?}");
    assert_eq!(workers[0].id, "w-ok");
}

/// Step 25 regression — `retry_worker` reads `last_prompt` from
/// `workers`. Before this gate the supervisor only wrote it on
/// `continue_worker`, so a fresh worker had no saved prompt and the
/// first retry click failed with `RowCorrupt("no last_prompt saved")`.
/// This test asserts the round-trip the supervisor now performs at
/// spawn time.
#[tokio::test]
async fn set_last_prompt_round_trips_through_get_resume_metadata() {
    let repo = Repository::open_in_memory().await.unwrap();
    let w = WorkerInfo {
        id: "w-retry".into(),
        name: "w-retry".into(),
        vendor: Vendor::Claude,
        cli_binary: "claude".into(),
        cli_version: None,
        cwd: "/tmp/work".into(),
        model: None,
        spawned_at: "2026-05-12T00:00:00.000Z".into(),
        ended_at: None,
    };
    repo.insert_worker(&w).await.unwrap();

    let metadata = repo.get_resume_metadata("w-retry").await.unwrap();
    assert!(metadata.last_prompt.is_none(), "fresh worker has no prompt");

    repo.set_last_prompt("w-retry", "fix the failing test")
        .await
        .unwrap();

    let metadata = repo.get_resume_metadata("w-retry").await.unwrap();
    assert_eq!(
        metadata.last_prompt.as_deref(),
        Some("fix the failing test")
    );
    assert_eq!(metadata.cwd, "/tmp/work");
    assert!(matches!(metadata.vendor, Vendor::Claude));
}

/// Step 25 regression — `spawn_claude_resume` reuses the original
/// worker_id, so the resumed adapter must start its `seq` counter past
/// the original run's max. `max_seq_for_worker` is the query that
/// feeds that seeding. Returns `None` when the worker has no events
/// yet (first spawn) and the high-water mark otherwise.
#[tokio::test]
async fn max_seq_for_worker_returns_none_when_no_events() {
    let repo = Repository::open_in_memory().await.unwrap();
    repo.insert_worker(&worker("w-fresh", "claude-fresh"))
        .await
        .unwrap();
    let max = repo.max_seq_for_worker("w-fresh").await.unwrap();
    assert_eq!(max, None);
}

#[tokio::test]
async fn max_seq_for_worker_returns_highest_seen_seq() {
    let repo = Repository::open_in_memory().await.unwrap();
    repo.insert_worker(&worker("w-seq", "claude-seq"))
        .await
        .unwrap();
    repo.insert_task(&task("t-seq", "first task"))
        .await
        .unwrap();
    // Out-of-order inserts to make sure MAX is taken, not "last inserted".
    repo.insert_event(&state_change_event(
        "w-seq",
        Some("t-seq"),
        5,
        WorkerState::Executing,
    ))
    .await
    .unwrap();
    repo.insert_event(&state_change_event(
        "w-seq",
        Some("t-seq"),
        0,
        WorkerState::Idle,
    ))
    .await
    .unwrap();
    repo.insert_event(&state_change_event(
        "w-seq",
        Some("t-seq"),
        12,
        WorkerState::Done,
    ))
    .await
    .unwrap();
    let max = repo.max_seq_for_worker("w-seq").await.unwrap();
    assert_eq!(max, Some(12));
}

#[tokio::test]
async fn max_seq_for_worker_is_scoped_per_worker() {
    let repo = Repository::open_in_memory().await.unwrap();
    repo.insert_worker(&worker("w-a", "claude-a"))
        .await
        .unwrap();
    repo.insert_worker(&worker("w-b", "claude-b"))
        .await
        .unwrap();
    repo.insert_task(&task("t-a", "a")).await.unwrap();
    repo.insert_task(&task("t-b", "b")).await.unwrap();
    repo.insert_event(&state_change_event(
        "w-a",
        Some("t-a"),
        100,
        WorkerState::Done,
    ))
    .await
    .unwrap();
    repo.insert_event(&state_change_event(
        "w-b",
        Some("t-b"),
        3,
        WorkerState::Executing,
    ))
    .await
    .unwrap();
    assert_eq!(repo.max_seq_for_worker("w-a").await.unwrap(), Some(100));
    assert_eq!(repo.max_seq_for_worker("w-b").await.unwrap(), Some(3));
    assert_eq!(
        repo.max_seq_for_worker("w-nonexistent").await.unwrap(),
        None
    );
}

// ── Archive primitive (Step retention) ─────────────────────────────

fn log_event(worker_id: &str, seq: u64) -> Event {
    Event {
        schema_version: "1.0".into(),
        worker_id: worker_id.into(),
        task_id: None,
        seq,
        ts: format!("2026-05-17T00:{:02}:{:02}.000Z", (seq / 60) % 60, seq % 60),
        kind: EventKind::Log(Log {
            level: LogLevel::Info,
            stream: LogStream::Stdout,
            line: format!("event {seq}"),
            tag: None,
        }),
    }
}

#[tokio::test]
async fn archive_excess_returns_zero_when_under_cap() {
    let repo = Repository::open_in_memory().await.unwrap();
    repo.insert_worker(&worker("w1", "claude")).await.unwrap();
    for seq in 0..5u64 {
        repo.insert_event(&log_event("w1", seq)).await.unwrap();
    }
    let moved = repo.archive_excess_for_worker("w1", 10).await.unwrap();
    assert_eq!(moved, 0, "below-cap workers are not touched");
    let events = repo.replay_for_worker("w1").await.unwrap();
    assert_eq!(events.len(), 5, "events table is unchanged");
}

#[tokio::test]
async fn archive_excess_moves_oldest_keeping_top_seq() {
    let repo = Repository::open_in_memory().await.unwrap();
    repo.insert_worker(&worker("w1", "claude")).await.unwrap();
    for seq in 0..100u64 {
        repo.insert_event(&log_event("w1", seq)).await.unwrap();
    }
    let moved = repo.archive_excess_for_worker("w1", 30).await.unwrap();
    assert_eq!(moved, 70, "70 oldest rows archived");
    let live = repo.replay_for_worker("w1").await.unwrap();
    assert_eq!(live.len(), 30, "live tier holds exactly keep_recent rows");
    let live_seqs: Vec<u64> = live.iter().map(|e| e.seq).collect();
    assert_eq!(
        live_seqs,
        (70..100).collect::<Vec<_>>(),
        "top-seq window kept"
    );
}

#[tokio::test]
async fn archive_excess_is_idempotent() {
    let repo = Repository::open_in_memory().await.unwrap();
    repo.insert_worker(&worker("w1", "claude")).await.unwrap();
    for seq in 0..50u64 {
        repo.insert_event(&log_event("w1", seq)).await.unwrap();
    }

    let first = repo.archive_excess_for_worker("w1", 20).await.unwrap();
    assert_eq!(first, 30);

    // Directly verify the archive table holds exactly the moved rows
    // — not just that the return value says so. Without this, a
    // future regression that swapped INSERT/DELETE order (and so
    // double-inserted into the archive on the second call) would
    // still pass the second-call-returns-zero check below.
    let archived_after_first: i64 =
        sqlx::query_scalar("SELECT COUNT(*) FROM events_archive WHERE worker_id = ?")
            .bind("w1")
            .fetch_one(repo.pool_for_test())
            .await
            .unwrap();
    assert_eq!(archived_after_first, 30, "archive holds 30 unique rows");

    let second = repo.archive_excess_for_worker("w1", 20).await.unwrap();
    assert_eq!(second, 0, "second call is a no-op");

    let archived_after_second: i64 =
        sqlx::query_scalar("SELECT COUNT(*) FROM events_archive WHERE worker_id = ?")
            .bind("w1")
            .fetch_one(repo.pool_for_test())
            .await
            .unwrap();
    assert_eq!(
        archived_after_second, 30,
        "no duplicate rows from the idempotent second call"
    );
}

#[tokio::test]
async fn archive_excess_is_isolated_per_worker() {
    let repo = Repository::open_in_memory().await.unwrap();
    repo.insert_worker(&worker("w-a", "claude")).await.unwrap();
    repo.insert_worker(&worker("w-b", "codex")).await.unwrap();
    for seq in 0..40u64 {
        repo.insert_event(&log_event("w-a", seq)).await.unwrap();
    }
    for seq in 0..5u64 {
        repo.insert_event(&log_event("w-b", seq)).await.unwrap();
    }
    let moved = repo.archive_excess_for_worker("w-a", 10).await.unwrap();
    assert_eq!(moved, 30);
    assert_eq!(repo.replay_for_worker("w-a").await.unwrap().len(), 10);
    assert_eq!(
        repo.replay_for_worker("w-b").await.unwrap().len(),
        5,
        "w-b untouched"
    );
}

#[tokio::test]
async fn archive_excess_for_all_sweeps_only_workers_over_cap() {
    let repo = Repository::open_in_memory().await.unwrap();
    repo.insert_worker(&worker("w-big", "claude"))
        .await
        .unwrap();
    repo.insert_worker(&worker("w-small", "codex"))
        .await
        .unwrap();
    for seq in 0..200u64 {
        repo.insert_event(&log_event("w-big", seq)).await.unwrap();
    }
    for seq in 0..5u64 {
        repo.insert_event(&log_event("w-small", seq)).await.unwrap();
    }
    let summary = repo.archive_excess_for_all(50).await.unwrap();
    assert_eq!(summary.get("w-big").copied(), Some(150));
    assert!(
        !summary.contains_key("w-small"),
        "under-cap workers absent from summary"
    );
    assert_eq!(repo.replay_for_worker("w-big").await.unwrap().len(), 50);
    assert_eq!(repo.replay_for_worker("w-small").await.unwrap().len(), 5);
}

/// Mutex that serialises the two env-var tests below. The integration test
/// binary runs all tests multi-threaded by default; without this guard the
/// set_var / remove_var calls race across threads and produce flaky results.
/// Using a static mutex is the zero-dependency equivalent of `serial_test`.
static ENV_CAP_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

#[tokio::test]
#[allow(clippy::await_holding_lock)] // ENV_CAP_LOCK serializes env-var access across tests; must be held through async repo ops so env-mutating siblings don't race.
async fn mark_worker_ended_archives_excess() {
    // Hold the lock for the full duration of this test so the env-override
    // test cannot set VIGLA_EVENTS_PER_WORKER_CAP while we open our
    // repo with the default cap (50_000).
    let _guard = ENV_CAP_LOCK.lock().unwrap();
    // Remove any stale value left by a previous run.
    unsafe {
        std::env::remove_var("VIGLA_EVENTS_PER_WORKER_CAP");
    }
    let repo = Repository::open_in_memory().await.unwrap();
    repo.insert_worker(&worker("w1", "claude")).await.unwrap();
    for seq in 0..100u64 {
        repo.insert_event(&log_event("w1", seq)).await.unwrap();
    }
    repo.mark_worker_ended("w1", "2026-05-17T00:00:00.000Z")
        .await
        .unwrap();
    // At default cap (EVENTS_PER_WORKER_LIVE_CAP_DEFAULT = 50_000),
    // 100 events stay live — the trigger fires but has nothing to do.
    // The real assertion that mark_worker_ended ACTUALLY trims is in
    // the env-override test below; this test verifies the trigger
    // doesn't fail or corrupt state when there's nothing to trim.
    let live = repo.replay_for_worker("w1").await.unwrap();
    assert_eq!(live.len(), 100, "at default cap, 100 events stay live");
}

#[tokio::test]
#[allow(clippy::await_holding_lock)] // ENV_CAP_LOCK serializes env-var access across tests; must be held through async repo ops so env-mutating siblings don't race.
async fn mark_worker_ended_respects_env_cap_override() {
    // Hold the lock so the env var is exclusively owned for this test.
    let _guard = ENV_CAP_LOCK.lock().unwrap();
    // SAFETY: we hold ENV_CAP_LOCK for the duration; no other test that
    // reads this env var can run concurrently.
    unsafe {
        std::env::set_var("VIGLA_EVENTS_PER_WORKER_CAP", "10");
    }
    let repo = Repository::open_in_memory().await.unwrap();
    repo.insert_worker(&worker("w1", "claude")).await.unwrap();
    for seq in 0..100u64 {
        repo.insert_event(&log_event("w1", seq)).await.unwrap();
    }
    repo.mark_worker_ended("w1", "2026-05-17T00:00:00.000Z")
        .await
        .unwrap();
    let live = repo.replay_for_worker("w1").await.unwrap();
    assert_eq!(
        live.len(),
        10,
        "mark_worker_ended trimmed to env-overridden cap"
    );
    unsafe {
        std::env::remove_var("VIGLA_EVENTS_PER_WORKER_CAP");
    }
}

#[tokio::test]
async fn workers_indexes_include_mission_and_session() {
    let repo = Repository::open_in_memory().await.unwrap();
    let rows = sqlx::query("PRAGMA index_list(workers)")
        .fetch_all(repo.pool_for_test())
        .await
        .unwrap();
    let names: Vec<String> = rows
        .iter()
        .map(|r| r.try_get::<String, _>("name").unwrap())
        .collect();
    assert!(
        names.iter().any(|n| n == "idx_workers_mission_spawned"),
        "missing mission_id index; got {names:?}"
    );
    assert!(
        names.iter().any(|n| n == "idx_workers_session"),
        "missing session_id index; got {names:?}"
    );
}

#[tokio::test]
async fn replay_for_worker_page_returns_empty_for_unknown_worker() {
    let repo = Repository::open_in_memory().await.unwrap();
    let page = repo.replay_for_worker_page("nope", None, 10).await.unwrap();
    assert_eq!(page.len(), 0);
}

#[tokio::test]
async fn replay_for_worker_page_returns_all_when_under_limit() {
    let repo = Repository::open_in_memory().await.unwrap();
    repo.insert_worker(&worker("w1", "claude")).await.unwrap();
    for seq in 0..5u64 {
        repo.insert_event(&log_event("w1", seq)).await.unwrap();
    }
    let page = repo.replay_for_worker_page("w1", None, 10).await.unwrap();
    let seqs: Vec<u64> = page.iter().map(|e| e.seq).collect();
    assert_eq!(seqs, vec![0, 1, 2, 3, 4]);
}

#[tokio::test]
async fn replay_for_worker_page_advances_via_cursor() {
    let repo = Repository::open_in_memory().await.unwrap();
    repo.insert_worker(&worker("w1", "claude")).await.unwrap();
    for seq in 0..25u64 {
        repo.insert_event(&log_event("w1", seq)).await.unwrap();
    }

    let p1 = repo.replay_for_worker_page("w1", None, 10).await.unwrap();
    assert_eq!(p1.len(), 10);
    assert_eq!(p1.first().unwrap().seq, 0);
    assert_eq!(p1.last().unwrap().seq, 9);

    let p2 = repo
        .replay_for_worker_page("w1", Some(9), 10)
        .await
        .unwrap();
    assert_eq!(p2.len(), 10);
    assert_eq!(p2.first().unwrap().seq, 10);
    assert_eq!(p2.last().unwrap().seq, 19);

    let p3 = repo
        .replay_for_worker_page("w1", Some(19), 10)
        .await
        .unwrap();
    assert_eq!(p3.len(), 5, "exhausted page is shorter than limit");
    assert_eq!(p3.first().unwrap().seq, 20);
    assert_eq!(p3.last().unwrap().seq, 24);

    let p4 = repo
        .replay_for_worker_page("w1", Some(24), 10)
        .await
        .unwrap();
    assert!(p4.is_empty(), "past-end cursor returns empty");
}

#[tokio::test]
async fn replay_for_worker_page_clamps_oversized_limit() {
    let repo = Repository::open_in_memory().await.unwrap();
    repo.insert_worker(&worker("w1", "claude")).await.unwrap();
    for seq in 0..50u64 {
        repo.insert_event(&log_event("w1", seq)).await.unwrap();
    }
    let page = repo
        .replay_for_worker_page("w1", None, u32::MAX)
        .await
        .unwrap();
    assert_eq!(page.len(), 50);
}

#[tokio::test]
async fn replay_for_task_page_advances_via_composite_cursor() {
    let repo = Repository::open_in_memory().await.unwrap();
    repo.insert_worker(&worker("w-a", "claude")).await.unwrap();
    repo.insert_worker(&worker("w-b", "codex")).await.unwrap();
    repo.insert_task(&task("t1", "shared")).await.unwrap();

    // Interleave events from two workers across one task. ts is the
    // primary sort axis; seq is the tiebreaker.
    let mut e = log_event("w-a", 0);
    e.task_id = Some("t1".into());
    e.ts = "2026-05-17T00:00:00.000Z".into();
    repo.insert_event(&e).await.unwrap();

    let mut e = log_event("w-b", 0);
    e.task_id = Some("t1".into());
    e.ts = "2026-05-17T00:00:01.000Z".into();
    repo.insert_event(&e).await.unwrap();

    let mut e = log_event("w-a", 1);
    e.task_id = Some("t1".into());
    e.ts = "2026-05-17T00:00:02.000Z".into();
    repo.insert_event(&e).await.unwrap();

    let mut e = log_event("w-b", 1);
    e.task_id = Some("t1".into());
    e.ts = "2026-05-17T00:00:02.000Z".into(); // tie with w-a/1
    repo.insert_event(&e).await.unwrap();

    let p1 = repo
        .replay_for_task_page("t1", None, None, None, 2)
        .await
        .unwrap();
    assert_eq!(p1.len(), 2);
    assert_eq!(p1[0].worker_id, "w-a");
    assert_eq!(p1[1].worker_id, "w-b");

    let last = p1.last().unwrap();
    let p2 = repo
        .replay_for_task_page(
            "t1",
            Some(last.seq),
            Some(&last.ts),
            Some(&last.worker_id),
            10,
        )
        .await
        .unwrap();
    assert_eq!(p2.len(), 2);
    // Within the same ts, seq ordering breaks the tie.
    assert_eq!(p2[0].ts, "2026-05-17T00:00:02.000Z");
    assert_eq!(p2[1].ts, "2026-05-17T00:00:02.000Z");
}

/// End-to-end: insert 200k events, archive down to 50k, reassemble
/// the live tier via the paged API. Pins the design's central
/// guarantee: a worker that emits 200k events never makes the
/// replay path materialise more than `keep_recent`. No timing
/// assertions — release/debug variance and CI noise would make
/// them flaky. The assertion is correctness at scale.
#[cfg_attr(debug_assertions, ignore = "200k-event scale; run with --release")]
#[tokio::test]
async fn large_log_archive_keeps_replay_bounded() {
    let repo = Repository::open_in_memory().await.unwrap();
    repo.insert_worker(&worker("w-fire", "claude"))
        .await
        .unwrap();

    // Insert in batched transactions so the test finishes in a few
    // seconds on release builds. 200_000 rows / 4_000 per batch =
    // 50 commits, each one ~80 ms on a modern laptop.
    const TOTAL: u64 = 200_000;
    const BATCH: u64 = 4_000;
    for batch in 0..(TOTAL / BATCH) {
        let mut tx = repo.pool_for_test().begin().await.unwrap();
        for seq_in_batch in 0..BATCH {
            let seq = batch * BATCH + seq_in_batch;
            sqlx::query(
                "INSERT INTO events \
                 (worker_id, task_id, seq, ts, type, payload_json, schema_version) \
                 VALUES (?, NULL, ?, '2026-05-17T00:00:00.000Z', 'log', '{\"level\":\"info\",\"stream\":\"stdout\",\"line\":\"x\"}', '1.0')",
            )
            .bind("w-fire")
            .bind(seq as i64)
            .execute(&mut *tx)
            .await
            .unwrap();
        }
        tx.commit().await.unwrap();
    }

    let moved = repo
        .archive_excess_for_worker("w-fire", 50_000)
        .await
        .unwrap();
    assert_eq!(moved, 150_000, "moved every row beyond the cap");

    // Reassemble live tier via the paged API.
    let mut cursor: Option<u64> = None;
    let mut total = 0u64;
    let mut last_seq = None;
    loop {
        let page = repo
            .replay_for_worker_page("w-fire", cursor, 512)
            .await
            .unwrap();
        if page.is_empty() {
            break;
        }
        // seq monotonic across pages.
        for e in &page {
            if let Some(prev) = last_seq {
                assert!(e.seq > prev, "non-monotonic seq across pages");
            }
            last_seq = Some(e.seq);
        }
        cursor = page.last().map(|e| e.seq);
        total += page.len() as u64;
    }
    assert_eq!(total, 50_000, "exactly keep_recent events reassembled");

    // Archive count check — direct SQL since we don't expose
    // events_archive on the public Repository surface.
    let archived: i64 =
        sqlx::query_scalar("SELECT COUNT(*) FROM events_archive WHERE worker_id = ?")
            .bind("w-fire")
            .fetch_one(repo.pool_for_test())
            .await
            .unwrap();
    assert_eq!(archived, 150_000);
}

#[tokio::test]
#[allow(clippy::await_holding_lock)] // ENV_CAP_LOCK serializes env-var access across tests; must be held through async repo ops so env-mutating siblings don't race.
async fn retention_guard_trims_on_tick() {
    // SAFETY: tests in this binary run sequentially within this module
    // when serialised by ENV_CAP_LOCK (added in Task 4). Use the same
    // lock so we don't race the mark_worker_ended_* env tests.
    let _guard_env = ENV_CAP_LOCK.lock().unwrap();
    unsafe {
        std::env::set_var("VIGLA_RETENTION_TICK_SECS", "1");
        std::env::set_var("VIGLA_EVENTS_PER_WORKER_CAP", "5");
    }
    let repo = Repository::open_in_memory().await.unwrap();
    repo.insert_worker(&worker("w1", "claude")).await.unwrap();
    for seq in 0..50u64 {
        repo.insert_event(&log_event("w1", seq)).await.unwrap();
    }

    let guard = RetentionGuard::spawn(repo.clone());

    // Poll in REAL time — deliberately NOT `tokio::time::pause()`/`advance()`.
    // Both the guard's sweep and the `replay_for_worker` assertion below
    // open real SQLite connections, which sqlx services on a dedicated
    // background OS thread that tokio's virtual clock cannot see. Under
    // paused time plus the CPU contention of the full test binary running
    // its ~30 other tests in parallel, that background thread gets starved;
    // tokio then considers the runtime idle and auto-advances the paused
    // clock straight past the 5 s pool-acquire timeout, so `pool.acquire()`
    // spuriously returns `PoolTimedOut` (reproduced ~1-in-15 full-binary
    // runs under load). A real-time poll keeps the pool timeout on the real
    // clock. The retention tick is 1 s here, so the trim lands within a
    // second or two; the 200 × 50 ms = 10 s bound fails loudly rather than
    // hanging if a badly starved box never trims.
    let mut live_len = usize::MAX;
    for _ in 0..200 {
        live_len = repo.replay_for_worker("w1").await.unwrap().len();
        if live_len <= 5 {
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;
    }

    assert_eq!(
        live_len, 5,
        "retention guard should trim live events for w1 down to the cap of 5"
    );

    drop(guard);
    unsafe {
        std::env::remove_var("VIGLA_RETENTION_TICK_SECS");
        std::env::remove_var("VIGLA_EVENTS_PER_WORKER_CAP");
    }
}
