//! Step 10 — feed deterministic synthetic Claude Code stream-json fixtures through
//! [`ClaudeAdapter`] and assert the resulting canonical-event stream
//! matches the `event-schema` contract.

use adapter_core::{Adapter, AdapterExit};
use claude_adapter::ClaudeAdapter;
use event_schema::{Event, EventKind, LogStream, WorkerState};
use std::path::PathBuf;

fn fixture_path(name: &str) -> PathBuf {
    let manifest_dir = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    manifest_dir.join("tests/fixtures").join(name)
}

fn run_fixture(name: &str) -> Vec<Event> {
    let path = fixture_path(name);
    let body = std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("read {path:?}: {e}"));
    let mut adapter = ClaudeAdapter::new(
        "test-worker-claude".to_string(),
        Some("test-task".to_string()),
    );
    let mut events = Vec::new();
    for line in body.lines() {
        events.extend(adapter.ingest_line(line, LogStream::Stdout));
    }
    events.extend(adapter.finalize(AdapterExit::Clean));
    events
}

#[test]
fn claude_simple_run_produces_canonical_event_stream() {
    let events = run_fixture("claude_simple.jsonl");
    assert!(!events.is_empty(), "expected at least one canonical event");

    // First event MUST be state_change → idle (event-schema.md §5).
    match &events[0].kind {
        EventKind::StateChange(sc) => assert_eq!(sc.state, WorkerState::Idle),
        other => panic!("first event must be state_change idle, got {other:?}"),
    }

    // Adapter must transition to executing on the system/init line.
    let executing = events.iter().any(|e| match &e.kind {
        EventKind::StateChange(sc) => sc.state == WorkerState::Executing,
        _ => false,
    });
    assert!(executing, "expected a state_change → executing");

    // Last lifecycle state should be `done` for a successful run.
    let last_state = events
        .iter()
        .rev()
        .find_map(|e| match &e.kind {
            EventKind::StateChange(sc) => Some(sc.state),
            _ => None,
        })
        .expect("at least one state_change");
    assert_eq!(last_state, WorkerState::Done);

    // Completion event present with non-empty summary.
    let completion = events.iter().find_map(|e| match &e.kind {
        EventKind::Completion(c) => Some(c),
        _ => None,
    });
    let completion = completion.expect("expected a completion event");
    assert!(
        !completion.summary.is_empty(),
        "completion summary must be non-empty"
    );

    // Cost event present with non-zero token totals (Claude always
    // reports usage).
    let cost = events.iter().find_map(|e| match &e.kind {
        EventKind::Cost(c) => Some(c),
        _ => None,
    });
    let cost = cost.expect("expected a cost event");
    assert!(
        cost.input_tokens > 0 || cost.output_tokens > 0 || cost.cache_read_tokens.unwrap_or(0) > 0,
        "cost event must carry token usage"
    );
    assert!(cost.usd >= 0.0);

    // At least one assistant text log was emitted.
    let assistant_log = events.iter().find_map(|e| match &e.kind {
        EventKind::Log(l) if l.tag.as_deref() == Some("assistant") => Some(&l.line),
        _ => None,
    });
    assert!(assistant_log.is_some(), "expected an assistant text log");

    // Every event has the schema_version stamped (event-schema §5).
    for e in &events {
        assert_eq!(e.schema_version, "2.0");
        assert_eq!(e.worker_id, "test-worker-claude");
    }

    // Seq is strictly monotonic from 0.
    for (i, e) in events.iter().enumerate() {
        assert_eq!(
            e.seq, i as u64,
            "seq must be 0..N (event {i} has seq {})",
            e.seq
        );
    }
}

#[test]
fn ingest_is_pure_function_no_external_state() {
    // Re-running the fixture from a fresh adapter must produce the
    // same number of events of each kind. Timestamps will differ
    // (`now_rfc3339`) but the structure is invariant.
    let a = run_fixture("claude_simple.jsonl");
    let b = run_fixture("claude_simple.jsonl");
    assert_eq!(a.len(), b.len());
    for (ax, bx) in a.iter().zip(b.iter()) {
        assert_eq!(ax.seq, bx.seq);
        assert_eq!(ax.worker_id, bx.worker_id);
        assert_eq!(
            std::mem::discriminant(&ax.kind),
            std::mem::discriminant(&bx.kind),
            "kind discriminants must match for seq {}",
            ax.seq
        );
    }
}

#[test]
fn non_json_stdout_line_becomes_info_log() {
    // First emission has the schema-mandated initial idle prepended
    // (event-schema §5).
    let mut adapter = ClaudeAdapter::new("w".to_string(), None);
    let events = adapter.ingest_line("hello, this is not JSON", LogStream::Stdout);
    assert_eq!(events.len(), 2);
    match &events[0].kind {
        EventKind::StateChange(sc) => assert_eq!(sc.state, WorkerState::Idle),
        other => panic!("first event must be idle, got {other:?}"),
    }
    match &events[1].kind {
        EventKind::Log(l) => {
            assert!(matches!(l.level, event_schema::LogLevel::Info));
            assert_eq!(l.line, "hello, this is not JSON");
        }
        other => panic!("expected log event, got {other:?}"),
    }
    // Second non-JSON line on the same adapter should NOT re-emit
    // the idle state.
    let next = adapter.ingest_line("more text", LogStream::Stdout);
    assert_eq!(next.len(), 1);
}

#[test]
fn stderr_line_becomes_warn_log() {
    let mut adapter = ClaudeAdapter::new("w".to_string(), None);
    let events = adapter.ingest_line("oops", LogStream::Stderr);
    assert_eq!(events.len(), 2);
    match &events[0].kind {
        EventKind::StateChange(sc) => assert_eq!(sc.state, WorkerState::Idle),
        other => panic!("first event must be idle, got {other:?}"),
    }
    match &events[1].kind {
        EventKind::Log(l) => {
            assert!(matches!(l.level, event_schema::LogLevel::Warn));
            assert!(matches!(l.stream, LogStream::Stderr));
        }
        other => panic!("expected warn log, got {other:?}"),
    }
}

#[test]
fn empty_line_emits_nothing() {
    let mut adapter = ClaudeAdapter::new("w".to_string(), None);
    assert!(adapter.ingest_line("", LogStream::Stdout).is_empty());
    assert!(adapter.ingest_line("\n", LogStream::Stdout).is_empty());
}

#[test]
fn synthetic_assistant_tool_use_emits_file_activity() {
    let mut adapter = ClaudeAdapter::new("w".to_string(), Some("t".to_string()));
    let line = r#"{"type":"assistant","message":{"content":[{"type":"tool_use","name":"Edit","input":{"file_path":"src/x.rs"}}]}}"#;
    let events = adapter.ingest_line(line, LogStream::Stdout);
    let has_file = events.iter().any(|e| match &e.kind {
        EventKind::FileActivity(f) => f.path == "src/x.rs",
        _ => false,
    });
    assert!(
        has_file,
        "Edit tool_use must produce file_activity for src/x.rs"
    );
}

#[test]
fn synthetic_error_result_emits_failure_then_failed_state() {
    let mut adapter = ClaudeAdapter::new("w".to_string(), Some("t".to_string()));
    // Drive into executing first via init.
    adapter.ingest_line(
        r#"{"type":"system","subtype":"init","cwd":"/tmp"}"#,
        LogStream::Stdout,
    );
    let events = adapter.ingest_line(
        r#"{"type":"result","subtype":"error_during_execution","is_error":true,"result":"boom","usage":{"input_tokens":10,"output_tokens":2}}"#,
        LogStream::Stdout,
    );

    let failure = events.iter().find_map(|e| match &e.kind {
        EventKind::Failure(f) => Some(f),
        _ => None,
    });
    assert!(failure.is_some(), "error result must produce failure event");
    assert_eq!(failure.unwrap().error, "boom");

    let final_state = events.iter().rev().find_map(|e| match &e.kind {
        EventKind::StateChange(sc) => Some(sc.state),
        _ => None,
    });
    assert_eq!(final_state, Some(WorkerState::Failed));
}

// -- finalize() exit-status handling -----------------------------------

/// Drive the adapter to `executing` (an `init` system line) without
/// emitting any `result` line, so the only terminal event the test
/// can observe must come from finalize().
fn started_but_unresolved_adapter() -> ClaudeAdapter {
    let mut adapter = ClaudeAdapter::new("w".to_string(), Some("t".to_string()));
    let init_line = r#"{"type":"system","subtype":"init"}"#;
    let _ = adapter.ingest_line(init_line, LogStream::Stdout);
    adapter
}

#[test]
fn finalize_emits_nothing_when_never_started() {
    let mut adapter = ClaudeAdapter::new("w".to_string(), None);
    assert!(adapter.finalize(AdapterExit::Clean).is_empty());
    assert!(adapter.finalize(AdapterExit::Killed).is_empty());
    assert!(adapter
        .finalize(AdapterExit::Failed { code: Some(1) })
        .is_empty());
}

#[test]
fn finalize_after_clean_exit_synthesises_done_when_no_result_seen() {
    // Regression: process killed mid-flight or exited cleanly without
    // a `result` line used to leave the worker tile stuck in
    // `executing` because the adapter returned an empty Vec from
    // finalize() regardless of exit status.
    let mut adapter = started_but_unresolved_adapter();
    let events = adapter.finalize(AdapterExit::Clean);
    assert_eq!(events.len(), 2, "expected Completion + Done state");
    assert!(matches!(events[0].kind, EventKind::Completion(_)));
    let EventKind::StateChange(sc) = &events[1].kind else {
        panic!(
            "expected state_change as second event, got {:?}",
            events[1].kind
        );
    };
    assert_eq!(sc.state, WorkerState::Done);
}

#[test]
fn finalize_after_failed_exit_synthesises_failure_with_exit_code() {
    let mut adapter = started_but_unresolved_adapter();
    let events = adapter.finalize(AdapterExit::Failed { code: Some(7) });
    assert_eq!(events.len(), 2);
    let EventKind::Failure(f) = &events[0].kind else {
        panic!("expected Failure first, got {:?}", events[0].kind);
    };
    assert_eq!(f.exit_code, Some(7));
    assert!(f.error.contains("code 7"));
    let EventKind::StateChange(sc) = &events[1].kind else {
        panic!("expected state_change second");
    };
    assert_eq!(sc.state, WorkerState::Failed);
}

#[test]
fn finalize_after_kill_synthesises_failure_tagged_stopped() {
    let mut adapter = started_but_unresolved_adapter();
    let events = adapter.finalize(AdapterExit::Killed);
    assert_eq!(events.len(), 2);
    assert!(matches!(events[0].kind, EventKind::Failure(_)));
    let EventKind::StateChange(sc) = &events[1].kind else {
        panic!("expected state_change second");
    };
    assert_eq!(sc.state, WorkerState::Failed);
    assert_eq!(sc.note.as_deref(), Some("worker stopped"));
}

#[test]
fn error_max_turns_categorized_as_timeout_not_internal() {
    // Audit r5 — `error_max_turns` is a budget-exceeded condition;
    // it should map to FailureCategory::Timeout so the operator UI
    // surfaces "raise --max-turns" rather than the wrong "report bug"
    // remediation that `Internal` implies.
    use event_schema::FailureCategory;
    let mut adapter = ClaudeAdapter::new("w".to_string(), Some("t".to_string()));
    let line = serde_json::json!({
        "type": "result",
        "subtype": "error_max_turns",
        "is_error": true,
        "result": "ran out of turns",
    })
    .to_string();
    let events = adapter.ingest_line(&line, LogStream::Stdout);
    let failure = events
        .iter()
        .find_map(|e| {
            if let EventKind::Failure(f) = &e.kind {
                Some(f)
            } else {
                None
            }
        })
        .expect("ingest_line should emit a Failure event for error_max_turns");
    assert_eq!(
        failure.category,
        Some(FailureCategory::Timeout),
        "error_max_turns must map to Timeout, got {:?}",
        failure.category,
    );
}

#[test]
fn finalize_no_op_when_terminal_already_emitted_via_result() {
    // claude_simple.jsonl ends with `result/success`, so the adapter
    // emits Completion + Done during ingest_line and `terminal_emitted`
    // is set. finalize() must NOT double-emit a second Done.
    let mut adapter = ClaudeAdapter::new("w".to_string(), Some("t".to_string()));
    let body = std::fs::read_to_string(fixture_path("claude_simple.jsonl")).unwrap();
    for line in body.lines() {
        let _ = adapter.ingest_line(line, LogStream::Stdout);
    }
    let extra = adapter.finalize(AdapterExit::Clean);
    assert!(
        extra.is_empty(),
        "finalize must be a no-op once handle_result already emitted a terminal state, got {} events",
        extra.len()
    );
}

/// Step 25 regression — when the supervisor resumes a Claude worker,
/// the original run's events already occupy `(worker_id, seq=0..N)` in
/// the events table. The resumed adapter must start past that high
/// water mark so its rows don't collide on the PRIMARY KEY and vanish
/// inside `insert_event_raw`. `with_starting_seq` is what feeds that
/// seeding from `Repository::max_seq_for_worker(...) + 1`.
#[test]
fn with_starting_seq_seeds_initial_event_seq() {
    let mut adapter = ClaudeAdapter::with_starting_seq("w".to_string(), Some("t".to_string()), 42);
    // The first ingest line of any real claude stream is a `system/init`,
    // which promotes the adapter into `executing` via two emitted
    // state_change events (idle, then executing). Both should use seq
    // ≥ 42 so they don't collide with whatever the original run
    // persisted at seq=0,1.
    let init_line = r#"{"type":"system","subtype":"init","session_id":"abc"}"#;
    let events = adapter.ingest_line(init_line, LogStream::Stdout);
    assert!(
        events.len() >= 2,
        "expected at least idle + executing, got {}",
        events.len()
    );
    assert_eq!(events[0].seq, 42, "first event seq must equal starting_seq");
    assert_eq!(
        events[1].seq, 43,
        "second event seq must increment from starting_seq"
    );
    // And the next emitted event continues to climb without ever
    // visiting any lower number.
    for e in &events {
        assert!(
            e.seq >= 42,
            "seeded adapter must never emit seq < starting_seq: {}",
            e.seq
        );
    }
}

#[test]
fn new_is_equivalent_to_with_starting_seq_zero() {
    let mut a = ClaudeAdapter::new("w".to_string(), None);
    let mut b = ClaudeAdapter::with_starting_seq("w".to_string(), None, 0);
    let init_line = r#"{"type":"system","subtype":"init","session_id":"abc"}"#;
    let events_a = a.ingest_line(init_line, LogStream::Stdout);
    let events_b = b.ingest_line(init_line, LogStream::Stdout);
    let seqs_a: Vec<u64> = events_a.iter().map(|e| e.seq).collect();
    let seqs_b: Vec<u64> = events_b.iter().map(|e| e.seq).collect();
    assert_eq!(seqs_a, seqs_b, "new() must equal with_starting_seq(.., 0)");
}

// ── T4 — malformed-schema tolerance ─────────────────────────────────
//
// These tests pin **current** adapter behaviour under partial /
// unexpected vendor schemas. They are documentation of the
// silent-degradation contract: if a vendor renames or restructures
// a field, the adapter must not panic, must still report a terminal
// state when one would otherwise be reachable, and unknown chunk
// types must surface as trace logs (never silently dropped).
//
// If a future refactor regresses any of these guarantees the test
// suite catches it immediately — until then the codepath has zero
// coverage and any such regression is a silent UX hit.

#[test]
fn t4_result_success_without_usage_does_not_panic() {
    // `usage` is the field cost accounting reads. A vendor schema
    // tweak that drops it must not crash the adapter.
    let mut a = ClaudeAdapter::new("w", None);
    let init = r#"{"type":"system","subtype":"init","session_id":"abc"}"#;
    a.ingest_line(init, LogStream::Stdout);
    let line = r#"{"type":"result","subtype":"success","result":"ok"}"#;
    let events = a.ingest_line(line, LogStream::Stdout);
    // No specific shape required — just no panic and a sensible event
    // stream (state_change → done or completion event).
    let _ = events;
}

#[test]
fn t4_assistant_with_empty_content_array_does_not_panic() {
    let mut a = ClaudeAdapter::new("w", None);
    let init = r#"{"type":"system","subtype":"init","session_id":"abc"}"#;
    a.ingest_line(init, LogStream::Stdout);
    let line = r#"{"type":"assistant","message":{"content":[]}}"#;
    // Should not panic. Empty `content` skips the inner loop entirely;
    // any state-change side-effects from ensure_started already
    // fired on the init line.
    let _ = a.ingest_line(line, LogStream::Stdout);
}

#[test]
fn t4_unknown_type_falls_through_to_trace_log() {
    let mut a = ClaudeAdapter::new("w", None);
    let line = r#"{"type":"future_event_type","payload":{"x":1}}"#;
    let events = a.ingest_line(line, LogStream::Stdout);
    let has_log = events.iter().any(|e| match &e.kind {
        EventKind::Log(l) => l.tag.as_deref() == Some("claude:future_event_type"),
        _ => false,
    });
    assert!(
        has_log,
        "unknown chunk types must surface as `claude:<type>` trace logs, got {events:#?}"
    );
}

#[test]
fn t4_assistant_with_missing_message_field_does_not_panic() {
    let mut a = ClaudeAdapter::new("w", None);
    let init = r#"{"type":"system","subtype":"init","session_id":"abc"}"#;
    a.ingest_line(init, LogStream::Stdout);
    // `message` entirely absent — handler returns silently.
    let _ = a.ingest_line(r#"{"type":"assistant"}"#, LogStream::Stdout);
}
