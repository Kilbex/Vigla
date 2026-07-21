//! Types describing a mission-level completion judgment.
//!
//! [`CompletionVerdict`] is the typed payload of
//! [`crate::mission_event::MissionEventKind::CompletionVerdictRendered`].
//! Constructed by [`crate::judgment::assemble_verdict`] at the end of every
//! mission run.

use serde::{Deserialize, Serialize};
use specta::Type;

use crate::arbiter::decision::{ArbiterDecision, ScrubReason};
use crate::arbiter::AuthorityBound;
use crate::audit::TestPassScore;

/// Mission-level "is this done?" verdict assembled after the per-task loop.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Type)]
pub struct CompletionVerdict {
    /// True iff every subtask landed in
    /// [`ArbiterDecision::Accept`] rather than any other decision.
    pub all_subtasks_accepted: bool,
    /// The integrated test-pass score. Sourced from the latest
    /// [`crate::mission_event::MissionEventKind::PostIntegrationAuditCompleted`]
    /// when present; falls back to the last per-task
    /// `AuditCompleted` when no post-integration audit ran.
    /// `None` if the mission had no audits at all (degenerate case
    /// for missions with zero tasks).
    pub integrated_test_pass: Option<TestPassScore>,
    /// Residual-risk band derived by [`crate::judgment::risk_band::score_risk`].
    pub residual_risk: RiskBand,
    /// Doc-coverage score in [0.0, 1.0]. v1: ratio of touched
    /// files that contain a top-of-file doc-comment block.
    pub doc_coverage: f64,
    /// Issues that did not resolve cleanly during the mission.
    /// Includes open escalations, recovery attempts, context-budget
    /// truncations, and supervisor-scrubbed subtasks.
    pub unresolved_issues: Vec<UnresolvedIssue>,
    /// Derived recommendation: `Accept` if all_subtasks_accepted
    /// && residual_risk in {Low,Medium} && no open escalations;
    /// `Scrub` otherwise, including residual_risk High.
    pub recommendation: ArbiterDecision,
}

impl Default for CompletionVerdict {
    fn default() -> Self {
        Self {
            all_subtasks_accepted: false,
            integrated_test_pass: None,
            residual_risk: RiskBand::High,
            doc_coverage: 0.0,
            unresolved_issues: Vec::new(),
            recommendation: ArbiterDecision::Scrub {
                reason: ScrubReason::AuditUnusable,
                retained_artifacts: Vec::new(),
                partial_audit: None,
            },
        }
    }
}

/// Closed 3-band residual-risk classification.
///
/// Heuristic boundaries (see
/// [`crate::judgment::risk_band::score_risk`]):
///
///   * `Low` when `audit.overall >= 0.85` AND zero security flags.
///   * `Medium` when `0.7 <= overall < 0.85` OR a single security
///     flag with no audit floor breach.
///   * `High` otherwise (overall below floor or multiple flags).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, Type)]
#[serde(rename_all = "snake_case")]
pub enum RiskBand {
    Low,
    Medium,
    High,
}

/// One unresolved issue surfaced by the mission. Sum type with one
/// variant per producer event class.
///
/// Source events for each variant (documented for the inbox renderer):
///
///   * `OpenEscalation` — produced by
///     [`crate::mission_event::MissionEventKind::ArbiterDecided`]
///     with `bound = Some(_)`.
///   * `RecoveryAttempted` — produced by
///     [`crate::mission_event::MissionEventKind::RecoveryDecided`]
///     events that escalated or retried.
///   * `ContextBudgetTruncated` — produced by
///     [`crate::mission_event::MissionEventKind::ContextBudgetTruncated`]
///     (composer re-render path).
///   * `SubtaskScrubbed` — produced by an `ArbiterDecided` whose decision is
///     `Scrub`, such as quality-budget exhaustion.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Type)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum UnresolvedIssue {
    /// Mission ended with an open arbiter escalation. The bound
    /// identifies which envelope tripped; the summary is the
    /// inbox-card preview text.
    OpenEscalation {
        bound: AuthorityBound,
        summary: String,
    },
    /// Recovery engine ran one or more times during the mission.
    /// `class` is the wire name of the [`crate::recovery::types::FailureClass`]
    /// (e.g. "missing_file", "command_error"); `action_taken` is
    /// the wire name of the chosen [`crate::recovery::types::RecoveryAction`]
    /// (e.g. "retry", "pause", "escalate"); `occurrences` is the
    /// total count for this class across the mission.
    RecoveryAttempted {
        class: String,
        action_taken: String,
        occurrences: u32,
    },
    /// Memory composer truncated the bundle for a worker because
    /// its token budget was exceeded. `dropped_count` is the
    /// number of notes that did not make it into the final
    /// bundle; `worker_id` identifies which worker was affected.
    ContextBudgetTruncated {
        dropped_count: u32,
        worker_id: String,
    },
    /// A subtask was scrubbed after its quality budget was exhausted or its
    /// audit became unusable. `task_index` and `reason` are the closing details.
    SubtaskScrubbed { task_index: u32, reason: String },
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::arbiter::decision::{AcceptPayload, ArbiterDecision};
    use crate::audit::{AuditReport, TestPassScore};

    #[test]
    fn completion_verdict_default_is_unscored() {
        let v = CompletionVerdict::default();
        assert!(!v.all_subtasks_accepted);
        assert!(v.integrated_test_pass.is_none());
        assert_eq!(v.residual_risk, RiskBand::High);
        assert_eq!(v.doc_coverage, 0.0);
        assert!(v.unresolved_issues.is_empty());
        assert!(matches!(v.recommendation, ArbiterDecision::Scrub { .. }));
    }

    #[test]
    fn completion_verdict_serializes_round_trip() {
        let v = CompletionVerdict {
            all_subtasks_accepted: true,
            integrated_test_pass: Some(TestPassScore {
                ran: true,
                passed: 42,
                failed: 0,
                skipped: 1,
                score: 1.0,
            }),
            residual_risk: RiskBand::Low,
            doc_coverage: 0.83,
            unresolved_issues: vec![],
            recommendation: ArbiterDecision::Accept(AcceptPayload {
                audit: AuditReport::default(),
                summary: "3 tasks integrated".into(),
            }),
        };
        let json = serde_json::to_string(&v).unwrap();
        let back: CompletionVerdict = serde_json::from_str(&json).unwrap();
        assert_eq!(v, back);
    }

    #[test]
    fn risk_band_all_variants_round_trip() {
        for band in [RiskBand::Low, RiskBand::Medium, RiskBand::High] {
            let json = serde_json::to_string(&band).unwrap();
            let back: RiskBand = serde_json::from_str(&json).unwrap();
            assert_eq!(band, back);
        }
    }

    #[test]
    fn unresolved_issue_all_variants_round_trip() {
        let cases = [
            UnresolvedIssue::OpenEscalation {
                bound: crate::arbiter::AuthorityBound::Scope,
                summary: "worker touched src/no.rs".into(),
            },
            UnresolvedIssue::RecoveryAttempted {
                class: "missing_file".into(),
                action_taken: "retry".into(),
                occurrences: 2,
            },
            UnresolvedIssue::ContextBudgetTruncated {
                dropped_count: 4,
                worker_id: "mock-1".into(),
            },
            UnresolvedIssue::SubtaskScrubbed {
                task_index: 2,
                reason: "supervisor_marked_unachievable".into(),
            },
        ];
        for u in &cases {
            let json = serde_json::to_string(u).unwrap();
            let back: UnresolvedIssue = serde_json::from_str(&json).unwrap();
            assert_eq!(u, &back);
        }
    }
}
