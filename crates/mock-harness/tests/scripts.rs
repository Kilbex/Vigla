//! Direct-API tests for the `build_script` function. Verifies
//! canonical-shape invariants (seq monotonic, ts monotonic, schema
//! version stamped, terminal events present) without spawning the
//! binary.

use event_schema::{Event, EventKind, WorkerState};
use mock_harness::{build_script, EmitOpts, Script};

fn opts() -> EmitOpts {
    EmitOpts {
        worker_id: "test-worker".into(),
        task_id: "test-task".into(),
        // 2026-01-01T00:00:00.000Z — chosen so the resulting ts strings
        // are easy to read while debugging if a test fails.
        start_unix_ms: 1_767_225_600_000,
    }
}

fn assert_round_trips(timed: &[mock_harness::TimedEvent]) {
    for te in timed {
        let json = serde_json::to_string(&te.event).expect("event must serialize");
        let parsed: Event = serde_json::from_str(&json)
            .unwrap_or_else(|e| panic!("event re-parse failed for {json}: {e}"));
        assert_eq!(parsed, te.event, "round-trip changed the event");
    }
}

#[test]
fn claude_happy_emits_canonical_events() {
    let timed = build_script(Script::ClaudeHappy, &opts());
    assert!(!timed.is_empty(), "claude_happy must emit events");
    assert_round_trips(&timed);

    // First event is worker-level idle (per event-schema.md §5).
    match &timed[0].event.kind {
        EventKind::StateChange(sc) => assert_eq!(sc.state, WorkerState::Idle),
        other => panic!("expected first event = state_change idle, got {other:?}"),
    }
    assert!(
        timed[0].event.task_id.is_none(),
        "initial idle is worker-scoped (task_id null)"
    );
    assert_eq!(timed[0].delay_ms_before, 0);

    // Final event is completion; previous-or-equal state_change is `done`.
    let last = timed.last().unwrap();
    assert!(
        matches!(last.event.kind, EventKind::Completion(_)),
        "claude_happy must end with completion, got {:?}",
        last.event.kind
    );

    let last_state = timed
        .iter()
        .rev()
        .find_map(|te| match &te.event.kind {
            EventKind::StateChange(sc) => Some(sc.state),
            _ => None,
        })
        .expect("expected at least one state_change");
    assert_eq!(last_state, WorkerState::Done);
}

#[test]
fn codex_blocked_includes_blocked_state_and_recovers() {
    let timed = build_script(Script::CodexBlocked, &opts());
    assert_round_trips(&timed);

    let states: Vec<WorkerState> = timed
        .iter()
        .filter_map(|te| match &te.event.kind {
            EventKind::StateChange(sc) => Some(sc.state),
            _ => None,
        })
        .collect();

    assert!(states.contains(&WorkerState::Blocked));
    assert!(states.contains(&WorkerState::Done));
    // Recovery: a blocked state must precede a later executing state.
    let blocked_idx = states
        .iter()
        .position(|s| *s == WorkerState::Blocked)
        .unwrap();
    let executing_after_block = states[blocked_idx..]
        .iter()
        .skip(1)
        .any(|s| *s == WorkerState::Executing);
    assert!(
        executing_after_block,
        "codex_blocked must transition back to executing after blocked"
    );

    // A `dependency` event should accompany the blocked transition.
    let has_dependency = timed
        .iter()
        .any(|te| matches!(te.event.kind, EventKind::Dependency(_)));
    assert!(
        has_dependency,
        "codex_blocked must include a dependency event"
    );
}

#[test]
fn seq_is_strictly_monotonic_per_script() {
    for script in [Script::ClaudeHappy, Script::CodexBlocked] {
        let timed = build_script(script, &opts());
        for (i, te) in timed.iter().enumerate() {
            assert_eq!(
                te.event.seq,
                i as u64,
                "{}: seq must equal index",
                script.name()
            );
        }
    }
}

#[test]
fn timestamps_are_monotonic_and_rfc3339() {
    for script in [Script::ClaudeHappy, Script::CodexBlocked] {
        let timed = build_script(script, &opts());
        let mut prev: Option<&str> = None;
        for te in &timed {
            // RFC 3339 with fixed-width fields compares lexicographically
            // in chronological order.
            assert!(
                te.event.ts.ends_with('Z'),
                "{}: ts {:?} must end in Z",
                script.name(),
                te.event.ts
            );
            assert!(
                te.event.ts.contains('T'),
                "{}: ts {:?} must contain T separator",
                script.name(),
                te.event.ts
            );
            if let Some(p) = prev {
                assert!(
                    te.event.ts.as_str() >= p,
                    "{}: ts must be monotonic ({} < {})",
                    script.name(),
                    te.event.ts,
                    p
                );
            }
            prev = Some(&te.event.ts);
        }
    }
}

#[test]
fn schema_version_is_stamped_on_every_event() {
    for script in [Script::ClaudeHappy, Script::CodexBlocked] {
        let timed = build_script(script, &opts());
        for te in &timed {
            assert_eq!(te.event.schema_version, "2.0");
        }
    }
}

#[test]
fn build_script_is_deterministic() {
    let a = build_script(Script::ClaudeHappy, &opts());
    let b = build_script(Script::ClaudeHappy, &opts());
    assert_eq!(a.len(), b.len());
    for (x, y) in a.iter().zip(b.iter()) {
        assert_eq!(x.event, y.event);
        assert_eq!(x.delay_ms_before, y.delay_ms_before);
    }
}

#[test]
fn worker_and_task_ids_propagate_into_events() {
    let custom = EmitOpts {
        worker_id: "WID".into(),
        task_id: "TID".into(),
        start_unix_ms: 1_767_225_600_000,
    };
    let timed = build_script(Script::ClaudeHappy, &custom);
    for te in &timed {
        assert_eq!(te.event.worker_id, "WID");
    }
    // First event is worker-level (task_id null). Every subsequent
    // event in claude_happy is task-attached.
    assert!(timed[0].event.task_id.is_none());
    for te in &timed[1..] {
        assert_eq!(te.event.task_id.as_deref(), Some("TID"));
    }
}

#[test]
fn gemini_happy_emits_full_lifecycle() {
    let timed = build_script(Script::GeminiHappy, &opts());
    assert!(!timed.is_empty());
    assert_round_trips(&timed);

    let states: Vec<_> = timed
        .iter()
        .filter_map(|te| match &te.event.kind {
            EventKind::StateChange(sc) => Some(sc.state),
            _ => None,
        })
        .collect();
    assert_eq!(
        states,
        vec![
            WorkerState::Idle,
            WorkerState::Planning,
            WorkerState::Executing,
            WorkerState::Reviewing,
            WorkerState::Done,
        ]
    );
}

#[test]
fn gemini_blocked_passes_through_blocked_state() {
    let timed = build_script(Script::GeminiBlocked, &opts());
    assert_round_trips(&timed);
    let saw_blocked = timed.iter().any(|te| {
        matches!(
            &te.event.kind,
            EventKind::StateChange(sc) if sc.state == WorkerState::Blocked,
        )
    });
    assert!(saw_blocked, "gemini_blocked must include a Blocked state");
}

#[test]
fn gemini_failed_emits_retryable_failure() {
    let timed = build_script(Script::GeminiFailed, &opts());
    assert_round_trips(&timed);
    let failure = timed
        .iter()
        .find_map(|te| match &te.event.kind {
            EventKind::Failure(f) => Some(f.clone()),
            _ => None,
        })
        .expect("gemini_failed must emit a Failure event");
    assert!(failure.retryable, "gemini_failed must be retryable=true");
}

#[test]
fn gemini_terminal_emits_non_retryable_failure() {
    let timed = build_script(Script::GeminiTerminal, &opts());
    assert_round_trips(&timed);
    let failure = timed
        .iter()
        .find_map(|te| match &te.event.kind {
            EventKind::Failure(f) => Some(f.clone()),
            _ => None,
        })
        .expect("gemini_terminal must emit a Failure event");
    assert!(
        !failure.retryable,
        "gemini_terminal must be retryable=false"
    );
}
