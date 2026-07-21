//! Pure helper translating a [`ReworkKind`] into a typed
//! [`ReworkPlan`] the mission loop applies between worker passes.
//! Mission loop owns all state mutation; this module just produces
//! the plan.

use crate::arbiter::ReworkKind;
use crate::mission_event::TaskDescriptor;
use event_schema::Vendor;
use std::path::PathBuf;

/// What the mission loop does *after* applying a [`ReworkPlan`].
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub enum NextLoopAction {
    /// Re-run another worker pass with overlays applied.
    #[default]
    Continue,
    /// Drop the current task and advance — Split case.
    Skip,
    /// Mission state → Attention with rationale (MarkUnachievable).
    Escalate { rationale: String },
}

/// Overlays the mission loop applies before the next worker pass.
/// `None` on any field means "no change" on that dimension.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct ReworkPlan {
    pub directive: Option<String>,
    pub fresh_worker_id: Option<String>,
    pub vendor_swap: Option<Vendor>,
    pub scope_overlay: Option<Vec<PathBuf>>,
    pub rebrief_overlay: Option<String>,
    pub append_sub_tasks: Option<Vec<TaskDescriptor>>,
    pub next_action: NextLoopAction,
}

/// Translate a [`ReworkKind`] into a [`ReworkPlan`].
///
/// Pure function: no I/O, no allocation beyond cloning the input
/// payloads. Deterministic given the same `(kind, current_worker_id,
/// current_task_index, attempts_used_for_task)` tuple.
pub fn plan_for_kind(
    kind: &ReworkKind,
    current_worker_id: &str,
    current_task_index: u32,
    attempts_used_for_task: u8,
) -> ReworkPlan {
    match kind {
        ReworkKind::Revise { directive } => ReworkPlan {
            directive: Some(if directive.trim().is_empty() {
                "audit score below floor; address findings and resubmit".to_string()
            } else {
                directive.clone()
            }),
            next_action: NextLoopAction::Continue,
            ..Default::default()
        },

        ReworkKind::Reassign {
            from_worker: _,
            to_vendor,
        } => {
            let next_attempt = attempts_used_for_task.saturating_add(1);
            let fresh = format!(
                "mock-{}-r{}",
                current_task_index.saturating_add(1),
                next_attempt
            );
            ReworkPlan {
                directive: Some(format!(
                    "Reassigning from {current_worker_id}. Previous worker's \
                     session ended without an acceptable submission; please \
                     attempt the task fresh."
                )),
                fresh_worker_id: Some(fresh),
                vendor_swap: *to_vendor,
                next_action: NextLoopAction::Continue,
                ..Default::default()
            }
        }

        ReworkKind::Split { sub_tasks } => {
            if sub_tasks.is_empty() {
                return ReworkPlan {
                    directive: Some(
                        "supervisor emitted empty split; treating as revise".to_string(),
                    ),
                    next_action: NextLoopAction::Continue,
                    ..Default::default()
                };
            }
            ReworkPlan {
                append_sub_tasks: Some(sub_tasks.clone()),
                next_action: NextLoopAction::Skip,
                ..Default::default()
            }
        }

        ReworkKind::Narrow { reduced_scope } => ReworkPlan {
            directive: Some(format!(
                "Scope narrowed by supervisor to {} path(s). Stay within these \
                 paths; do not modify files outside this set.",
                reduced_scope.len(),
            )),
            scope_overlay: Some(reduced_scope.clone()),
            next_action: NextLoopAction::Continue,
            ..Default::default()
        },

        ReworkKind::Rebrief { new_brief } => ReworkPlan {
            rebrief_overlay: Some(new_brief.clone()),
            directive: Some("Task brief replaced by supervisor; see new task title.".to_string()),
            next_action: NextLoopAction::Continue,
            ..Default::default()
        },

        ReworkKind::MarkUnachievable { rationale } => ReworkPlan {
            next_action: NextLoopAction::Escalate {
                rationale: rationale.clone(),
            },
            ..Default::default()
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn revise_with_directive_passes_through() {
        let plan = plan_for_kind(
            &ReworkKind::Revise {
                directive: "fix the parser bug".into(),
            },
            "mock-1",
            0,
            0,
        );
        assert_eq!(plan.directive.as_deref(), Some("fix the parser bug"));
        assert_eq!(plan.next_action, NextLoopAction::Continue);
        assert!(plan.fresh_worker_id.is_none());
        assert!(plan.vendor_swap.is_none());
        assert!(plan.scope_overlay.is_none());
        assert!(plan.rebrief_overlay.is_none());
        assert!(plan.append_sub_tasks.is_none());
    }

    #[test]
    fn revise_with_empty_directive_falls_back_to_default() {
        let plan = plan_for_kind(
            &ReworkKind::Revise {
                directive: String::new(),
            },
            "mock-1",
            0,
            0,
        );
        assert!(plan
            .directive
            .as_deref()
            .unwrap()
            .contains("address findings"));
    }

    #[test]
    fn reassign_allocates_fresh_id_with_attempt_suffix() {
        let plan = plan_for_kind(
            &ReworkKind::Reassign {
                from_worker: "mock-1".into(),
                to_vendor: None,
            },
            "mock-1",
            0,
            0,
        );
        assert_eq!(plan.fresh_worker_id.as_deref(), Some("mock-1-r1"));
        assert!(plan.vendor_swap.is_none());
        assert_eq!(plan.next_action, NextLoopAction::Continue);
    }

    #[test]
    fn split_appends_sub_tasks_and_signals_skip() {
        let subs = vec![
            TaskDescriptor {
                index: 0,
                title: "a".into(),
                ..Default::default()
            },
            TaskDescriptor {
                index: 1,
                title: "b".into(),
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
        assert_eq!(plan.append_sub_tasks.as_ref().unwrap(), &subs);
        assert_eq!(plan.next_action, NextLoopAction::Skip);
    }

    #[test]
    fn split_with_empty_sub_tasks_falls_back_to_revise() {
        let plan = plan_for_kind(&ReworkKind::Split { sub_tasks: vec![] }, "mock-1", 0, 0);
        assert_eq!(plan.next_action, NextLoopAction::Continue);
        assert!(plan.append_sub_tasks.is_none());
        assert!(plan.directive.as_deref().unwrap().contains("empty split"));
    }

    #[test]
    fn narrow_applies_scope_overlay() {
        let plan = plan_for_kind(
            &ReworkKind::Narrow {
                reduced_scope: vec![PathBuf::from("src/lib.rs")],
            },
            "mock-1",
            0,
            0,
        );
        assert_eq!(
            plan.scope_overlay.as_ref().unwrap(),
            &vec![PathBuf::from("src/lib.rs")]
        );
        assert!(plan
            .directive
            .as_deref()
            .unwrap()
            .contains("Scope narrowed"));
        assert_eq!(plan.next_action, NextLoopAction::Continue);
    }

    #[test]
    fn rebrief_applies_title_overlay() {
        let plan = plan_for_kind(
            &ReworkKind::Rebrief {
                new_brief: "Implement only the parser.".into(),
            },
            "mock-1",
            0,
            0,
        );
        assert_eq!(
            plan.rebrief_overlay.as_deref(),
            Some("Implement only the parser.")
        );
        assert_eq!(plan.next_action, NextLoopAction::Continue);
    }

    #[test]
    fn mark_unachievable_escalates_with_rationale() {
        let plan = plan_for_kind(
            &ReworkKind::MarkUnachievable {
                rationale: "needs manual review".into(),
            },
            "mock-1",
            0,
            0,
        );
        match plan.next_action {
            NextLoopAction::Escalate { rationale } => {
                assert_eq!(rationale, "needs manual review");
            }
            other => panic!("expected Escalate, got {other:?}"),
        }
        assert!(plan.directive.is_none());
        assert!(plan.scope_overlay.is_none());
    }
}
