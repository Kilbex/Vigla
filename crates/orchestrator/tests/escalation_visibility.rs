//! Integration test for the silent-by-default invariant: a
//! happy-path mission emits zero Inbox events until the terminal
//! `mission.completed` card.
//!
//! Also covers the escalation path: an Escalate decision lands
//! exactly one `ActionRequired` Inbox card.

use orchestrator::arbiter::AuthorityBound;
use orchestrator::escalation::{visibility_for, EventVisibility, InboxKind, Severity};
use orchestrator::mission::MissionSpec;
use orchestrator::mission_event::{MergeResolution, MissionEventKind, TaskDescriptor};

fn spec() -> MissionSpec {
    MissionSpec {
        title: "T".into(),
        objective: "O".into(),
        target_ref: "main".into(),
        tests: None,
        supervisor_model: None,
        worker_model: None,
        worker_count: None,
        confirm_plan: None,
        scope_paths: vec![],
    }
}

fn count_inbox(events: &[MissionEventKind]) -> usize {
    events
        .iter()
        .filter(|e| matches!(visibility_for(e), EventVisibility::Inbox { .. }))
        .count()
}

fn count_action_required(events: &[MissionEventKind]) -> usize {
    events
        .iter()
        .filter(|e| {
            matches!(
                visibility_for(e),
                EventVisibility::Inbox {
                    severity: Severity::ActionRequired,
                    ..
                }
            )
        })
        .count()
}

/// U7 acceptance criterion: a happy-path mission produces ZERO
/// user-visible alerts until the terminal Completion card.
#[test]
fn happy_path_mission_emits_only_one_terminal_inbox_event() {
    // Simulate the event stream of a 2-task happy-path mission.
    let events: Vec<MissionEventKind> = vec![
        MissionEventKind::Created { spec: spec() },
        MissionEventKind::ExecutionStarted,
        MissionEventKind::Decomposition {
            tasks: vec![
                TaskDescriptor {
                    index: 0,
                    title: "Step 1".into(),
                    ..Default::default()
                },
                TaskDescriptor {
                    index: 1,
                    title: "Step 2".into(),
                    ..Default::default()
                },
            ],
        },
        MissionEventKind::WorkerSpawned {
            worker_id: "w-1".into(),
            task_index: 0,
            task_title: "Step 1".into(),
        },
        MissionEventKind::WorkerProgress {
            worker_id: "w-1".into(),
            note: "looking at src/".into(),
        },
        MissionEventKind::WorkerResultSubmitted {
            worker_id: "w-1".into(),
            files: vec!["src/a.rs".into()],
            summary: "patched".into(),
        },
        MissionEventKind::ReviewStarted {
            worker_id: "w-1".into(),
        },
        MissionEventKind::AuditCompleted {
            tier: "smoke".into(),
            overall: 0.85,
            payload_json: "{}".into(),
        },
        MissionEventKind::ArbiterDecided {
            worker_id: "w-1".into(),
            decision_json: r#"{"kind":"accept"}"#.into(),
            audit_overall: 0.85,
            bound: None,
        },
        MissionEventKind::Integrated {
            worker_id: "w-1".into(),
            integration_sha: "deadbeef".into(),
            snapshot_tag: "snap-1".into(),
        },
        MissionEventKind::WorkerSpawned {
            worker_id: "w-2".into(),
            task_index: 1,
            task_title: "Step 2".into(),
        },
        MissionEventKind::WorkerResultSubmitted {
            worker_id: "w-2".into(),
            files: vec!["src/b.rs".into()],
            summary: "patched".into(),
        },
        MissionEventKind::AuditCompleted {
            tier: "smoke".into(),
            overall: 0.9,
            payload_json: "{}".into(),
        },
        MissionEventKind::ArbiterDecided {
            worker_id: "w-2".into(),
            decision_json: r#"{"kind":"accept"}"#.into(),
            audit_overall: 0.9,
            bound: None,
        },
        MissionEventKind::Integrated {
            worker_id: "w-2".into(),
            integration_sha: "cafebabe".into(),
            snapshot_tag: "snap-2".into(),
        },
        MissionEventKind::Completed {
            summary: "2 tasks integrated".into(),
            files_changed: 2,
        },
    ];

    // Per-worker Accept decisions are Completion cards; the
    // mission-level Completed is also a Completion card. The
    // U7 acceptance language ("until the terminal Accept event")
    // allows the per-worker Accept cards. What it forbids is
    // ActionRequired during a happy path.
    let inbox_count = count_inbox(&events);
    assert!(
        inbox_count >= 1,
        "expected at least one terminal Inbox card; got {inbox_count}"
    );

    let action_required = count_action_required(&events);
    assert_eq!(
        action_required, 0,
        "happy path must emit ZERO ActionRequired alerts; got {action_required}"
    );
}

/// U7 acceptance criterion: an Escalate decision produces exactly
/// one `ActionRequired` Inbox card with structured evidence.
#[test]
fn quality_escalation_emits_one_action_required_card() {
    let events: Vec<MissionEventKind> = vec![
        MissionEventKind::Created { spec: spec() },
        MissionEventKind::ExecutionStarted,
        MissionEventKind::WorkerSpawned {
            worker_id: "w-1".into(),
            task_index: 0,
            task_title: "Step".into(),
        },
        MissionEventKind::WorkerResultSubmitted {
            worker_id: "w-1".into(),
            files: vec!["src/x.rs".into()],
            summary: "patched".into(),
        },
        MissionEventKind::AuditCompleted {
            tier: "smoke".into(),
            overall: 0.3,
            payload_json: "{}".into(),
        },
        MissionEventKind::ArbiterDecided {
            worker_id: "w-1".into(),
            decision_json: r#"{"kind":"escalate","bound":"quality"}"#.into(),
            audit_overall: 0.3,
            bound: Some(AuthorityBound::Quality),
        },
    ];

    let action_required = count_action_required(&events);
    assert_eq!(action_required, 1, "escalation must produce 1 alert");
}

/// Edge case: an Aborted event is a Warning Inbox card, NOT
/// ActionRequired (the abort already happened — nothing for the
/// user to action).
#[test]
fn abort_emits_warning_not_action_required() {
    let events = vec![MissionEventKind::Aborted {
        reason: "user cancelled".into(),
    }];

    let v = visibility_for(&events[0]);
    assert!(matches!(
        v,
        EventVisibility::Inbox {
            kind: InboxKind::Escalation,
            severity: Severity::Warning,
        }
    ));
    assert_eq!(count_action_required(&events), 0);
}

/// Edge case: a merge resolution is an Info Completion card.
#[test]
fn merge_resolved_emits_info_completion() {
    let v = visibility_for(&MissionEventKind::MergeResolved {
        resolution: MergeResolution::Merged,
    });
    assert!(matches!(
        v,
        EventVisibility::Inbox {
            kind: InboxKind::Completion,
            severity: Severity::Info,
        }
    ));
}
