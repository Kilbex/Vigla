//! [`DecisionContext`] — everything `decide()` needs that is not
//! the audit report itself. Mission-loop fills this in just before
//! calling `decide()`. Kept separate from the policy so that
//! per-mission state stays out of the long-lived policy struct.

use std::path::PathBuf;

use crate::arbiter::rework::ReworkKind;

#[derive(Debug, Clone, PartialEq)]
pub struct DecisionContext {
    /// Number of rework attempts the current worker has already
    /// consumed for this task.
    pub attempts_used_for_task: u8,
    /// Number of rework attempts consumed across the mission so far.
    pub attempts_used_for_mission: u8,
    /// What the worker reported as the human summary of its
    /// submission. Becomes the `Accept.summary` if accepted.
    pub submission_summary: String,
    /// Set of files the worker says it touched (relative to
    /// worktree root). Used by the scope check together with the
    /// audit's `ScopeScore`.
    pub touched_files: Vec<String>,
    /// Allow-list from the mission spec. Empty means "no constraint".
    pub scope_paths: Vec<PathBuf>,
    /// The supervisor's explicit semantic-rework request for this review turn.
    /// `None` means it accepted the submission or no semantic review was
    /// required. Automated scope/risk gates remain authoritative, but a
    /// `Some(_)` request is never overridden merely because tests and lint are
    /// green. The selected kind is preserved in the `ArbiterDecided` payload.
    pub preferred_rework_kind: Option<ReworkKind>,
}

impl DecisionContext {
    /// Convenience constructor used in tests.
    #[cfg(test)]
    pub fn for_test(submission_summary: &str) -> Self {
        Self {
            attempts_used_for_task: 0,
            attempts_used_for_mission: 0,
            submission_summary: submission_summary.to_string(),
            touched_files: vec![],
            scope_paths: vec![],
            preferred_rework_kind: None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::arbiter::rework::ReworkKind;

    #[test]
    fn for_test_zeroes_attempts() {
        let ctx = DecisionContext::for_test("hello");
        assert_eq!(ctx.attempts_used_for_task, 0);
        assert_eq!(ctx.attempts_used_for_mission, 0);
        assert_eq!(ctx.submission_summary, "hello");
    }

    #[test]
    fn for_test_defaults_preferred_rework_to_none() {
        let c = DecisionContext::for_test("x");
        assert!(c.preferred_rework_kind.is_none());
    }

    #[test]
    fn preferred_rework_kind_round_trips_through_clone() {
        let mut c = DecisionContext::for_test("y");
        c.preferred_rework_kind = Some(ReworkKind::Revise {
            directive: "d".into(),
        });
        let c2 = c.clone();
        assert_eq!(
            c2.preferred_rework_kind,
            Some(ReworkKind::Revise {
                directive: "d".into()
            })
        );
    }
}
