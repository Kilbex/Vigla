//! Fixture-driven tests for `GeminiAdapter`.
//!
//! `tests/fixtures/happy_path.jsonl` is a deterministic synthetic model of
//! `gemini --output-format stream-json` output (see fixtures/README.md). These
//! tests verify the adapter translates that wire format into the canonical
//! event shape correctly.

use adapter_core::{Adapter, AdapterExit};
use event_schema::{EventKind, FileOp, LogStream, WorkerState};
use gemini_adapter::GeminiAdapter;

fn ingest_all(adapter: &mut GeminiAdapter, raw: &str) -> Vec<event_schema::Event> {
    let mut out = Vec::new();
    for line in raw.lines() {
        out.extend(adapter.ingest_line(line, LogStream::Stdout));
    }
    out
}

#[test]
fn happy_path_fixture_produces_idle_executing_done() {
    let raw = include_str!("fixtures/happy_path.jsonl");
    let mut adapter = GeminiAdapter::new("w-fix", Some("t-fix".into()));
    let events = ingest_all(&mut adapter, raw);

    let states: Vec<WorkerState> = events
        .iter()
        .filter_map(|e| match &e.kind {
            EventKind::StateChange(sc) => Some(sc.state),
            _ => None,
        })
        .collect();

    assert!(
        states.contains(&WorkerState::Idle),
        "must emit initial idle: {states:?}"
    );
    assert!(
        states.contains(&WorkerState::Executing),
        "must promote to executing: {states:?}"
    );
    assert_eq!(
        states.last(),
        Some(&WorkerState::Done),
        "must end in Done: {states:?}"
    );
}

#[test]
fn happy_path_fixture_emits_assistant_log() {
    let raw = include_str!("fixtures/happy_path.jsonl");
    let mut adapter = GeminiAdapter::new("w-fix", Some("t-fix".into()));
    let events = ingest_all(&mut adapter, raw);

    let assistant_log = events.iter().find_map(|e| match &e.kind {
        EventKind::Log(l) if l.tag.as_deref() == Some("assistant") => Some(l),
        _ => None,
    });
    assert!(
        assistant_log.is_some(),
        "must emit at least one assistant log"
    );
}

#[test]
fn happy_path_fixture_emits_file_activity_for_read_file_tool() {
    // The fixture's prompt ("Read README.md ... summarize") triggers
    // a `read_file` tool_use. The adapter maps that into a
    // FileActivity event with op=Read.
    let raw = include_str!("fixtures/happy_path.jsonl");
    let mut adapter = GeminiAdapter::new("w-fix", Some("t-fix".into()));
    let events = ingest_all(&mut adapter, raw);

    let read_activity = events.iter().find_map(|e| match &e.kind {
        EventKind::FileActivity(f) if matches!(f.op, FileOp::Read) => Some(f),
        _ => None,
    });
    assert!(
        read_activity.is_some(),
        "fixture's read_file tool_use must surface as FileActivity::Read"
    );
}

#[test]
fn happy_path_fixture_emits_cost_with_token_counts() {
    let raw = include_str!("fixtures/happy_path.jsonl");
    let mut adapter = GeminiAdapter::new("w-fix", Some("t-fix".into()));
    let events = ingest_all(&mut adapter, raw);

    let cost = events
        .iter()
        .find_map(|e| match &e.kind {
            EventKind::Cost(c) => Some(c),
            _ => None,
        })
        .expect("result line must produce a Cost event");
    assert!(cost.input_tokens > 0, "input_tokens must be parsed");
    assert!(cost.output_tokens > 0, "output_tokens must be parsed");
    // Gemini stream-json does not include a USD price.
    assert_eq!(cost.usd, 0.0);
    assert!(cost.model.is_some(), "headline model name must be parsed");
}

#[test]
fn empty_input_clean_finalize_emits_idle_then_done() {
    let mut adapter = GeminiAdapter::new("w", None);
    let events = adapter.finalize(AdapterExit::Clean);
    let states: Vec<WorkerState> = events
        .iter()
        .filter_map(|e| match &e.kind {
            EventKind::StateChange(sc) => Some(sc.state),
            _ => None,
        })
        .collect();
    assert_eq!(states, vec![WorkerState::Idle, WorkerState::Done]);
}

#[test]
fn killed_finalize_emits_failed_state() {
    let mut adapter = GeminiAdapter::new("w", None);
    // Promote to executing first so the synthesized terminal is
    // executing → failed (matches stop-mid-run shape).
    let _ = adapter.ingest_line(
        r#"{"type":"init","session_id":"s","model":"gemini-3"}"#,
        LogStream::Stdout,
    );
    let events = adapter.finalize(AdapterExit::Killed);
    assert!(events.iter().any(|e| matches!(
        &e.kind,
        EventKind::StateChange(sc) if sc.state == WorkerState::Failed
    )));
    assert!(events
        .iter()
        .any(|e| matches!(&e.kind, EventKind::Failure(_))));
}

#[test]
fn double_finalize_does_not_re_emit() {
    let raw = include_str!("fixtures/happy_path.jsonl");
    let mut adapter = GeminiAdapter::new("w", None);
    let _ = ingest_all(&mut adapter, raw);
    let again = adapter.finalize(AdapterExit::Clean);
    assert!(
        again.is_empty(),
        "finalize after a successful result line must be a no-op, got {} events",
        again.len()
    );
}

#[test]
fn stderr_lines_become_warn_logs() {
    let mut adapter = GeminiAdapter::new("w", None);
    let events = adapter.ingest_line("YOLO mode is enabled.", LogStream::Stderr);
    assert!(events.iter().any(|e| matches!(
        &e.kind,
        EventKind::Log(l) if l.line.contains("YOLO mode")
    )));
}

#[test]
fn duplicate_result_line_does_not_re_emit_terminal() {
    // Regression: a malformed vendor stream that emits a second
    // `result` line after the first was emitting a second
    // Completion + StateChange pair with a desynchronised seq,
    // leaving the worker tile in an inconsistent UI state.
    let result_line = r#"{"type":"result","status":"success","stats":{"duration_ms":42}}"#;
    let mut adapter = GeminiAdapter::new("w", None);

    let first = adapter.ingest_line(result_line, LogStream::Stdout);
    let first_terminal: Vec<_> = first
        .iter()
        .filter(|e| {
            matches!(
                &e.kind,
                EventKind::StateChange(_) | EventKind::Completion(_)
            )
        })
        .collect();
    assert!(
        !first_terminal.is_empty(),
        "first result line must emit a terminal event"
    );

    let second = adapter.ingest_line(result_line, LogStream::Stdout);
    let second_terminal: Vec<_> = second
        .iter()
        .filter(|e| {
            matches!(
                &e.kind,
                EventKind::StateChange(_) | EventKind::Completion(_) | EventKind::Failure(_)
            )
        })
        .collect();
    assert!(
        second_terminal.is_empty(),
        "second result line must NOT re-emit terminal events; got {} events",
        second_terminal.len()
    );
}

// ── T4 — malformed-schema tolerance ─────────────────────────────────

#[test]
fn t4_result_without_stats_does_not_panic() {
    let mut a = GeminiAdapter::new("w".to_string(), None);
    let line = r#"{"type":"result","status":"ok"}"#;
    let _ = a.ingest_line(line, LogStream::Stdout);
}

#[test]
fn t4_unknown_type_falls_through_to_trace_log() {
    let mut a = GeminiAdapter::new("w".to_string(), None);
    let line = r#"{"type":"future_gemini_event","payload":{"x":1}}"#;
    let events = a.ingest_line(line, LogStream::Stdout);
    let has_log = events.iter().any(|e| match &e.kind {
        EventKind::Log(l) => l.tag.as_deref() == Some("gemini:future_gemini_event"),
        _ => false,
    });
    assert!(
        has_log,
        "unknown chunk types must surface as `gemini:<type>` trace logs, got {events:#?}"
    );
}

#[test]
fn t4_tool_use_missing_arguments_does_not_panic() {
    let mut a = GeminiAdapter::new("w".to_string(), None);
    // `tool_use` with no `arguments` field — must not crash extraction.
    let line = r#"{"type":"tool_use","tool":"read_file"}"#;
    let _ = a.ingest_line(line, LogStream::Stdout);
}
