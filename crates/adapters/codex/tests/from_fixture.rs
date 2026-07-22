//! Step 13 — feed deterministic synthetic Codex CLI `--json` fixtures through
//! [`CodexAdapter`] and assert the resulting canonical-event stream.

use adapter_core::{Adapter, AdapterExit};
use codex_adapter::CodexAdapter;
use event_schema::{Event, EventKind, LogStream, WorkerState};
use std::path::PathBuf;

fn fixture_path(name: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures")
        .join(name)
}

fn run_fixture(name: &str) -> Vec<Event> {
    let body = std::fs::read_to_string(fixture_path(name)).unwrap();
    let mut adapter = CodexAdapter::new(
        "test-worker-codex".to_string(),
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
fn codex_simple_run_produces_canonical_event_stream() {
    let events = run_fixture("codex_simple.jsonl");
    assert!(!events.is_empty());

    // First event MUST be state_change → idle.
    match &events[0].kind {
        EventKind::StateChange(sc) => assert_eq!(sc.state, WorkerState::Idle),
        other => panic!("first event must be idle, got {other:?}"),
    }

    // Adapter promotes to executing on thread.started.
    let executing = events.iter().any(|e| match &e.kind {
        EventKind::StateChange(sc) => sc.state == WorkerState::Executing,
        _ => false,
    });
    assert!(executing, "expected state_change executing");

    // Cost event present (turn.completed → cost).
    let cost = events.iter().find_map(|e| match &e.kind {
        EventKind::Cost(c) => Some(c),
        _ => None,
    });
    let cost = cost.expect("expected cost event");
    assert!(cost.input_tokens > 0 || cost.output_tokens > 0);

    // Tool log for command_execution.
    let bash_log = events.iter().any(|e| match &e.kind {
        EventKind::Log(l) => l.tag.as_deref() == Some("tool:Bash"),
        _ => false,
    });
    assert!(bash_log, "expected a tool:Bash log line");

    // Assistant log present.
    let assistant_log = events.iter().any(|e| match &e.kind {
        EventKind::Log(l) => l.tag.as_deref() == Some("assistant"),
        _ => false,
    });
    assert!(assistant_log, "expected assistant log line");

    // Finalize() should have synthesized completion + done state.
    let completion = events
        .iter()
        .any(|e| matches!(e.kind, EventKind::Completion(_)));
    assert!(completion, "expected synthetic completion on finalize");
    let last_state = events
        .iter()
        .rev()
        .find_map(|e| match &e.kind {
            EventKind::StateChange(sc) => Some(sc.state),
            _ => None,
        })
        .unwrap();
    assert_eq!(last_state, WorkerState::Done);

    // Schema version + monotonic seq.
    for (i, e) in events.iter().enumerate() {
        assert_eq!(e.schema_version, "2.0");
        assert_eq!(e.seq, i as u64);
    }
}

#[test]
fn synthetic_failed_command_emits_warn_log() {
    let mut adapter = CodexAdapter::new("w".to_string(), None);
    adapter.ingest_line(r#"{"type":"thread.started"}"#, LogStream::Stdout);
    let line = r#"{"type":"item.completed","item":{"type":"command_execution","command":"false","exit_code":1,"aggregated_output":"","status":"completed"}}"#;
    let events = adapter.ingest_line(line, LogStream::Stdout);
    let warn = events.iter().any(|e| match &e.kind {
        EventKind::Log(l) => matches!(l.level, event_schema::LogLevel::Warn),
        _ => false,
    });
    assert!(warn, "non-zero exit must produce warn log");
}

#[test]
fn stderr_lines_become_warn_logs() {
    let mut adapter = CodexAdapter::new("w".to_string(), None);
    let events = adapter.ingest_line(
        "2026-05-09T00:00:00.000Z ERROR codex_models_manager: timeout",
        LogStream::Stderr,
    );
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
fn no_terminal_state_without_starting() {
    // If no JSON lines arrived (process never even said
    // thread.started), finalize must NOT synthesize anything (the
    // pipeline can't claim the worker reached `done` if it never
    // started).
    let mut adapter = CodexAdapter::new("w".to_string(), None);
    let events = adapter.finalize(AdapterExit::Clean);
    assert!(events.is_empty());
}

#[test]
fn determinism_across_two_runs() {
    let a = run_fixture("codex_simple.jsonl");
    let b = run_fixture("codex_simple.jsonl");
    assert_eq!(a.len(), b.len());
    for (x, y) in a.iter().zip(b.iter()) {
        assert_eq!(x.seq, y.seq);
        assert_eq!(
            std::mem::discriminant(&x.kind),
            std::mem::discriminant(&y.kind)
        );
    }
}

// -- finalize() exit-status handling -----------------------------------

/// Drive the adapter to `executing` (a `thread.started` line) without
/// ingesting any natural completion, so the only terminal event the
/// test sees must come from finalize().
fn started_codex_adapter() -> CodexAdapter {
    let mut adapter = CodexAdapter::new("w".to_string(), Some("t".to_string()));
    let _ = adapter.ingest_line(
        r#"{"type":"thread.started","thread_id":"x"}"#,
        LogStream::Stdout,
    );
    adapter
}

#[test]
fn finalize_clean_exit_emits_completion_and_done() {
    let mut adapter = started_codex_adapter();
    let events = adapter.finalize(AdapterExit::Clean);
    assert_eq!(events.len(), 2);
    assert!(matches!(events[0].kind, EventKind::Completion(_)));
    let EventKind::StateChange(sc) = &events[1].kind else {
        panic!("expected state_change, got {:?}", events[1].kind);
    };
    assert_eq!(sc.state, WorkerState::Done);
}

#[test]
fn finalize_failed_exit_emits_failure_and_failed() {
    // Regression: the previous finalize ALWAYS emitted Completion +
    // Done regardless of exit status, so a non-zero codex exit
    // surfaced as a clean run.
    let mut adapter = started_codex_adapter();
    let events = adapter.finalize(AdapterExit::Failed { code: Some(2) });
    assert_eq!(events.len(), 2);
    let EventKind::Failure(f) = &events[0].kind else {
        panic!("expected Failure first, got {:?}", events[0].kind);
    };
    assert_eq!(f.exit_code, Some(2));
    assert!(f.error.contains("code 2"));
    let EventKind::StateChange(sc) = &events[1].kind else {
        panic!("expected state_change second");
    };
    assert_eq!(sc.state, WorkerState::Failed);
}

#[test]
fn finalize_killed_exit_emits_failure_tagged_stopped() {
    // Regression: SIGKILL after stop button used to surface as a
    // clean Done state since finalize ignored exit status.
    let mut adapter = started_codex_adapter();
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
fn finalize_idempotent_via_finalized_flag() {
    let mut adapter = started_codex_adapter();
    let first = adapter.finalize(AdapterExit::Clean);
    assert_eq!(first.len(), 2);
    let second = adapter.finalize(AdapterExit::Killed);
    assert!(
        second.is_empty(),
        "second finalize must be a no-op even with a different exit signal"
    );
}

// ── T4 — malformed-schema tolerance ─────────────────────────────────

#[test]
fn t4_turn_completed_without_usage_does_not_panic() {
    let mut a = CodexAdapter::new("w".to_string(), None);
    let line = r#"{"type":"turn.completed","status":"ok"}"#;
    let _ = a.ingest_line(line, LogStream::Stdout);
}

#[test]
fn t4_unknown_type_falls_through_to_trace_log() {
    let mut a = CodexAdapter::new("w".to_string(), None);
    let line = r#"{"type":"future_codex_event","payload":{"x":1}}"#;
    let events = a.ingest_line(line, LogStream::Stdout);
    let has_log = events.iter().any(|e| match &e.kind {
        EventKind::Log(l) => l.tag.as_deref() == Some("codex:future_codex_event"),
        _ => false,
    });
    assert!(
        has_log,
        "unknown chunk types must surface as `codex:<type>` trace logs, got {events:#?}"
    );
}

#[test]
fn t4_item_with_missing_type_does_not_panic() {
    let mut a = CodexAdapter::new("w".to_string(), None);
    // Missing inner "type" on items_added — defaults to "" and falls
    // through. Must not panic.
    let line = r#"{"type":"items_added","items":[{"id":"a"}]}"#;
    let _ = a.ingest_line(line, LogStream::Stdout);
}
