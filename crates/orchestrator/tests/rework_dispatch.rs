//! S6 — per-kind ReworkKind dispatch (integration / public-API).
//!
//! Mirrors `arbiter::rework_dispatch::tests` but at the crate
//! boundary. Each variant exercised once via the public re-exports.

use orchestrator::arbiter::{plan_for_kind, NextLoopAction, ReworkKind};
use orchestrator::mission_event::TaskDescriptor;
use std::path::PathBuf;

#[test]
fn revise_public_re_export() {
    let plan = plan_for_kind(
        &ReworkKind::Revise {
            directive: "fix the parser".into(),
        },
        "mock-1",
        0,
        0,
    );
    assert_eq!(plan.directive.as_deref(), Some("fix the parser"));
    assert_eq!(plan.next_action, NextLoopAction::Continue);
}

#[test]
fn reassign_public_re_export_with_vendor() {
    let plan = plan_for_kind(
        &ReworkKind::Reassign {
            from_worker: "mock-1".into(),
            to_vendor: Some(event_schema::Vendor::Gemini),
        },
        "mock-1",
        0,
        1,
    );
    assert_eq!(plan.vendor_swap, Some(event_schema::Vendor::Gemini));
    assert_eq!(plan.fresh_worker_id.as_deref(), Some("mock-1-r2"));
}

#[test]
fn split_public_re_export() {
    let subs = vec![
        TaskDescriptor {
            index: 0,
            title: "Parser".into(),
            ..Default::default()
        },
        TaskDescriptor {
            index: 1,
            title: "Tests".into(),
            ..Default::default()
        },
    ];
    let plan = plan_for_kind(
        &ReworkKind::Split {
            sub_tasks: subs.clone(),
        },
        "mock-1",
        0,
        0,
    );
    assert_eq!(plan.next_action, NextLoopAction::Skip);
    assert_eq!(plan.append_sub_tasks.as_ref().unwrap(), &subs);
}

#[test]
fn narrow_public_re_export() {
    let plan = plan_for_kind(
        &ReworkKind::Narrow {
            reduced_scope: vec![PathBuf::from("src/parser.rs")],
        },
        "mock-1",
        0,
        0,
    );
    assert_eq!(plan.scope_overlay.as_ref().unwrap().len(), 1);
}

#[test]
fn rebrief_public_re_export() {
    let plan = plan_for_kind(
        &ReworkKind::Rebrief {
            new_brief: "Implement only the parser combinator.".into(),
        },
        "mock-1",
        0,
        0,
    );
    assert_eq!(
        plan.rebrief_overlay.as_deref(),
        Some("Implement only the parser combinator.")
    );
}

#[test]
fn mark_unachievable_public_re_export() {
    let plan = plan_for_kind(
        &ReworkKind::MarkUnachievable {
            rationale: "manual review needed".into(),
        },
        "mock-1",
        0,
        0,
    );
    match plan.next_action {
        NextLoopAction::Escalate { rationale } => {
            assert_eq!(rationale, "manual review needed");
        }
        other => panic!("expected Escalate, got {other:?}"),
    }
}
