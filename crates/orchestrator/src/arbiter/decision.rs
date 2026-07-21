//! [`ArbiterDecision`] — what the arbiter tells `mission_loop.rs` to
//! do with a worker's submission. Constructed by [`crate::arbiter::decide`].
//!
//! Variants:
//! - `Accept`: integrate the submission. Carries the audit report
//!   (for `AuditCompleted` event emission) and a summary string.
//! - `Extend`: send the worker back for another pass within the
//!   rework budget. Carries the kind of rework requested.
//! - `Scrub`: drop this worker's work. Useful for unrecoverable
//!   quality failures where rework budget is exhausted.
//! - `Escalate`: halt and surface to the user. Carries which bound
//!   tripped + evidence + a suggested user action.

use crate::arbiter::{bound::AuthorityBound, bound::EscalationEvidence, rework::ReworkKind};
use crate::audit::AuditReport;
use serde::{Deserialize, Serialize};
use specta::Type;
use std::path::PathBuf;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Type)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum ArbiterDecision {
    Accept(AcceptPayload),
    Extend {
        rework_kind: ReworkKind,
        attempts_remaining: u8,
    },
    Scrub {
        reason: ScrubReason,
        retained_artifacts: Vec<PathBuf>,
        partial_audit: Option<AuditReport>,
    },
    Escalate {
        bound: AuthorityBound,
        evidence: EscalationEvidence,
        suggested_user_action: SuggestedUserAction,
    },
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Type)]
pub struct AcceptPayload {
    pub audit: AuditReport,
    pub summary: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Type)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum ScrubReason {
    /// Quality floor unreachable inside the rework budget.
    QualityExhausted,
    /// Audit ran but produced no usable score (infrastructure
    /// failure — test runner missing, lint runner panicked, etc.).
    AuditUnusable,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Type)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum SuggestedUserAction {
    /// User to narrow scope or co-sign the out-of-scope diff.
    ConfirmScope { out_of_scope_paths: Vec<String> },
    /// User to co-sign a risky operation (schema migration, mass
    /// delete, secret-touching change, etc.).
    CoSignRisk { detail: String },
    /// User to acknowledge the irrecoverable state and choose
    /// merge-partial / discard.
    ResolveMission,
    /// QC-3: open `MissionPlanPreview` and approve / regenerate /
    /// reject. Emitted by `plan_envelope_check::check` when the
    /// supervisor's `envelope_fit` trips a bound at decompose time
    /// — before any worker has run. Distinct from `ConfirmScope` /
    /// `CoSignRisk` which arrive post-audit.
    ReviewPlan,
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::arbiter::{bound::AuthorityBound, bound::EscalationEvidence, rework::ReworkKind};

    #[test]
    fn accept_round_trip() {
        let d = ArbiterDecision::Accept(AcceptPayload {
            audit: AuditReport::default(),
            summary: "all green".to_string(),
        });
        let json = serde_json::to_string(&d).unwrap();
        let back: ArbiterDecision = serde_json::from_str(&json).unwrap();
        assert_eq!(d, back);
    }

    #[test]
    fn extend_round_trip_revise() {
        let d = ArbiterDecision::Extend {
            rework_kind: ReworkKind::Revise {
                directive: "address the TODO".into(),
            },
            attempts_remaining: 1,
        };
        let json = serde_json::to_string(&d).unwrap();
        let back: ArbiterDecision = serde_json::from_str(&json).unwrap();
        assert_eq!(d, back);
    }

    #[test]
    fn extend_round_trip_reassign() {
        let d = ArbiterDecision::Extend {
            rework_kind: ReworkKind::Reassign {
                from_worker: "mock-1".into(),
                to_vendor: Some(event_schema::Vendor::Codex),
            },
            attempts_remaining: 2,
        };
        let json = serde_json::to_string(&d).unwrap();
        let back: ArbiterDecision = serde_json::from_str(&json).unwrap();
        assert_eq!(d, back);
    }

    #[test]
    fn extend_round_trip_split() {
        let d = ArbiterDecision::Extend {
            rework_kind: ReworkKind::Split {
                sub_tasks: vec![crate::mission_event::TaskDescriptor {
                    index: 7,
                    title: "parser".into(),
                    ..Default::default()
                }],
            },
            attempts_remaining: 1,
        };
        let json = serde_json::to_string(&d).unwrap();
        let back: ArbiterDecision = serde_json::from_str(&json).unwrap();
        assert_eq!(d, back);
    }

    #[test]
    fn extend_round_trip_narrow() {
        let d = ArbiterDecision::Extend {
            rework_kind: ReworkKind::Narrow {
                reduced_scope: vec![std::path::PathBuf::from("src/lib.rs")],
            },
            attempts_remaining: 1,
        };
        let json = serde_json::to_string(&d).unwrap();
        let back: ArbiterDecision = serde_json::from_str(&json).unwrap();
        assert_eq!(d, back);
    }

    #[test]
    fn extend_round_trip_rebrief() {
        let d = ArbiterDecision::Extend {
            rework_kind: ReworkKind::Rebrief {
                new_brief: "implement only the parser".into(),
            },
            attempts_remaining: 1,
        };
        let json = serde_json::to_string(&d).unwrap();
        let back: ArbiterDecision = serde_json::from_str(&json).unwrap();
        assert_eq!(d, back);
    }

    #[test]
    fn extend_round_trip_mark_unachievable() {
        let d = ArbiterDecision::Extend {
            rework_kind: ReworkKind::MarkUnachievable {
                rationale: "manual review required".into(),
            },
            attempts_remaining: 0,
        };
        let json = serde_json::to_string(&d).unwrap();
        let back: ArbiterDecision = serde_json::from_str(&json).unwrap();
        assert_eq!(d, back);
    }

    #[test]
    fn scrub_round_trip_quality_exhausted() {
        let d = ArbiterDecision::Scrub {
            reason: ScrubReason::QualityExhausted,
            retained_artifacts: vec![PathBuf::from("crash.log")],
            partial_audit: Some(AuditReport::default()),
        };
        let json = serde_json::to_string(&d).unwrap();
        let back: ArbiterDecision = serde_json::from_str(&json).unwrap();
        assert_eq!(d, back);
    }

    #[test]
    fn scrub_round_trip_audit_unusable() {
        let d = ArbiterDecision::Scrub {
            reason: ScrubReason::AuditUnusable,
            retained_artifacts: vec![],
            partial_audit: None,
        };
        let json = serde_json::to_string(&d).unwrap();
        let back: ArbiterDecision = serde_json::from_str(&json).unwrap();
        assert_eq!(d, back);
    }

    #[test]
    fn escalate_round_trip() {
        let d = ArbiterDecision::Escalate {
            bound: AuthorityBound::Scope,
            evidence: EscalationEvidence::default(),
            suggested_user_action: SuggestedUserAction::ConfirmScope {
                out_of_scope_paths: vec!["unrelated/file.rs".into()],
            },
        };
        let json = serde_json::to_string(&d).unwrap();
        let back: ArbiterDecision = serde_json::from_str(&json).unwrap();
        assert_eq!(d, back);
    }
}
