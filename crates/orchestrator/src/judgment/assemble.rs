//! Verdict assembler. Pure function — easy to property-test.
//!
//! Pulls the mission's signals into one input struct
//! [`AssembleInputs`], delegates to the three sub-scorers
//! (risk, unresolved, doc coverage), then derives the
//! `recommendation: ArbiterDecision` from a simple
//! majority-of-signals rule:
//!
//!   * `Accept` if `all_subtasks_accepted` AND
//!     `residual_risk in {Low, Medium}` AND no `OpenEscalation`
//!     issues remain.
//!   * `Scrub` if residual risk is high, a subtask failed, or an escalation
//!     remains open. Post-verdict continuation is not a supported runtime
//!     action, so the recommendation must fail closed.

use std::path::PathBuf;

use crate::arbiter::decision::{AcceptPayload, ArbiterDecision, ScrubReason};
use crate::audit::{AuditReport, TestPassScore};
use crate::judgment::doc_coverage::score_doc_coverage;
use crate::judgment::risk_band::score_risk;
use crate::judgment::unresolved::{collect_unresolved, ScrubRecord};
use crate::judgment::verdict::{CompletionVerdict, RiskBand, UnresolvedIssue};
use crate::judgment::RecoveryHistorySummary;
use crate::mission_event::MissionEventKind;

/// Borrowed snapshot of mission-level inputs feeding the assembler.
/// The driver constructs this immediately after the per-task loop
/// completes (or at any abort path).
#[derive(Debug)]
pub struct AssembleInputs<'a> {
    /// Worktree root used by the doc-coverage scorer to read
    /// touched files.
    pub worktree_root: PathBuf,
    /// Files touched across the whole mission (mission-level
    /// diff). Same shape as `WorkerSubmission.files`.
    pub touched_files: &'a [String],
    /// True iff every task in the decomposition landed in
    /// `ArbiterDecision::Accept`. Derived by the driver from its
    /// per-task outcome ledger.
    pub all_subtasks_accepted: bool,
    /// Best mission-level audit: post-integration audit when
    /// present, otherwise the last per-task audit report. `None`
    /// if no audits ran (degenerate case).
    pub mission_audit: Option<&'a AuditReport>,
    /// Test-pass score lifted from `mission_audit` if present;
    /// kept separate so the assembler doesn't reach into the
    /// `Option<&AuditReport>` twice.
    pub integrated_test_pass: Option<&'a TestPassScore>,
    /// Mission-level recovery aggregate.
    pub recovery_history: &'a RecoveryHistorySummary,
    /// Telemetry events relevant to unresolved-issue collection.
    /// The driver filters the full event stream down to:
    ///   * `ArbiterDecided` with `bound = Some` (escalations)
    ///   * `ContextBudgetTruncated` (S8 carryover)
    ///
    /// Other events are not needed.
    pub events: &'a [MissionEventKind],
    /// Per-task scrubs the driver observed.
    pub scrubs: &'a [ScrubRecord],
}

/// Assemble a [`CompletionVerdict`] from the mission's collected
/// signals.
///
/// Pure function — no IO besides the doc-coverage scorer's
/// touched-file reads. Easy to property-test.
///
/// Recommendation rule (deliberately simple):
///
/// | all_accepted | residual_risk | open escalations | →      |
/// |--------------|---------------|------------------|--------|
/// | true         | Low / Medium  | none             | Accept |
/// | true         | High          | none             | Scrub  |
/// | true         | any           | ≥ 1              | Scrub  |
/// | false        | any           | any              | Scrub  |
pub fn assemble_verdict(inputs: &AssembleInputs<'_>) -> CompletionVerdict {
    // 1. Risk band (uses the mission audit + history).
    let placeholder_audit = AuditReport::default();
    let report_ref = inputs.mission_audit.unwrap_or(&placeholder_audit);
    let residual_risk = score_risk(report_ref, inputs.recovery_history);

    // 2. Doc coverage.
    let doc_coverage = score_doc_coverage(&inputs.worktree_root, inputs.touched_files);

    // 3. Unresolved issues.
    let unresolved_issues =
        collect_unresolved(inputs.events, inputs.recovery_history, inputs.scrubs);

    let has_open_escalation = unresolved_issues
        .iter()
        .any(|i| matches!(i, UnresolvedIssue::OpenEscalation { .. }));

    // 4. Recommendation derivation.
    let recommendation = derive_recommendation(
        inputs.all_subtasks_accepted,
        residual_risk,
        has_open_escalation,
        report_ref,
    );

    // 5. Stash the integrated test-pass score for the inbox card.
    let integrated_test_pass = inputs.integrated_test_pass.cloned();

    CompletionVerdict {
        all_subtasks_accepted: inputs.all_subtasks_accepted,
        integrated_test_pass,
        residual_risk,
        doc_coverage,
        unresolved_issues,
        recommendation,
    }
}

fn derive_recommendation(
    all_subtasks_accepted: bool,
    residual_risk: RiskBand,
    has_open_escalation: bool,
    audit: &AuditReport,
) -> ArbiterDecision {
    if !all_subtasks_accepted || has_open_escalation {
        return ArbiterDecision::Scrub {
            reason: if has_open_escalation {
                ScrubReason::AuditUnusable
            } else {
                ScrubReason::QualityExhausted
            },
            retained_artifacts: Vec::new(),
            partial_audit: Some(audit.clone()),
        };
    }
    match residual_risk {
        RiskBand::Low | RiskBand::Medium => ArbiterDecision::Accept(AcceptPayload {
            audit: audit.clone(),
            summary: format!("all subtasks accepted; residual risk {residual_risk:?}",),
        }),
        RiskBand::High => ArbiterDecision::Scrub {
            reason: ScrubReason::QualityExhausted,
            retained_artifacts: Vec::new(),
            partial_audit: Some(audit.clone()),
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::audit::report::{SecurityFlag, SecurityFlagKind};
    use std::fs;
    use tempfile::TempDir;

    fn audit(overall: f64, with_tests: bool) -> AuditReport {
        AuditReport {
            overall,
            test_pass: if with_tests {
                Some(TestPassScore {
                    ran: true,
                    passed: 12,
                    failed: 0,
                    skipped: 0,
                    score: 1.0,
                })
            } else {
                None
            },
            ..AuditReport::default()
        }
    }

    fn worktree_with_doc() -> TempDir {
        let dir = TempDir::new().unwrap();
        fs::create_dir_all(dir.path().join("src")).unwrap();
        fs::write(
            dir.path().join("src/lib.rs"),
            "//! Documented module.\npub fn x() {}\n",
        )
        .unwrap();
        dir
    }

    /// Build an `AssembleInputs` from explicit values; helper keeps
    /// per-test setup short.
    fn run(
        dir: &TempDir,
        all_accepted: bool,
        mission_audit: Option<&AuditReport>,
        events: &[MissionEventKind],
        scrubs: &[ScrubRecord],
        history: &RecoveryHistorySummary,
    ) -> CompletionVerdict {
        let touched = vec!["src/lib.rs".to_string()];
        let integrated = mission_audit.and_then(|r| r.test_pass.as_ref());
        let inputs = AssembleInputs {
            worktree_root: dir.path().to_path_buf(),
            touched_files: &touched,
            all_subtasks_accepted: all_accepted,
            mission_audit,
            integrated_test_pass: integrated,
            recovery_history: history,
            events,
            scrubs,
        };
        assemble_verdict(&inputs)
    }

    fn flags(n: usize) -> Vec<SecurityFlag> {
        (0..n)
            .map(|i| SecurityFlag {
                kind: SecurityFlagKind::SchemaMigration,
                path: format!("migrations/{i}.sql"),
                detail: "schema".into(),
            })
            .collect()
    }

    #[test]
    fn happy_path_yields_accept_recommendation() {
        let dir = worktree_with_doc();
        let a = audit(0.92, true);
        let v = run(
            &dir,
            true,
            Some(&a),
            &[],
            &[],
            &RecoveryHistorySummary::default(),
        );
        assert!(v.all_subtasks_accepted);
        assert_eq!(v.residual_risk, RiskBand::Low);
        assert!(v.unresolved_issues.is_empty());
        assert!(matches!(v.recommendation, ArbiterDecision::Accept(_)));
        assert!(v.doc_coverage > 0.99);
    }

    #[test]
    fn high_risk_with_clean_subtasks_fails_closed() {
        let dir = worktree_with_doc();
        let mut a = audit(0.92, true);
        a.security_flags = flags(2);
        let v = run(
            &dir,
            true,
            Some(&a),
            &[],
            &[],
            &RecoveryHistorySummary::default(),
        );
        assert_eq!(v.residual_risk, RiskBand::High);
        assert!(matches!(v.recommendation, ArbiterDecision::Scrub { .. }));
    }

    #[test]
    fn failed_subtasks_yield_scrub() {
        let dir = worktree_with_doc();
        let a = audit(0.45, false);
        let scrubs = vec![ScrubRecord {
            task_index: 0,
            reason: "quality_exhausted".into(),
        }];
        let v = run(
            &dir,
            false,
            Some(&a),
            &[],
            &scrubs,
            &RecoveryHistorySummary::default(),
        );
        assert!(!v.all_subtasks_accepted);
        assert!(matches!(v.recommendation, ArbiterDecision::Scrub { .. }));
        assert_eq!(v.unresolved_issues.len(), 1);
    }

    #[test]
    fn open_escalation_blocks_accept() {
        let dir = worktree_with_doc();
        let a = audit(0.92, true);
        let decision = ArbiterDecision::Escalate {
            bound: crate::arbiter::AuthorityBound::Scope,
            evidence: crate::arbiter::EscalationEvidence {
                summary: "out of scope".into(),
                payload_json: None,
            },
            suggested_user_action: crate::arbiter::decision::SuggestedUserAction::ConfirmScope {
                out_of_scope_paths: vec!["src/no.rs".into()],
            },
        };
        let event = MissionEventKind::ArbiterDecided {
            worker_id: "worker-1".into(),
            decision_json: serde_json::to_string(&decision).unwrap(),
            audit_overall: 0.4,
            bound: Some(crate::arbiter::AuthorityBound::Scope),
        };
        // Driver thinks all subtasks accepted, but the open escalation forces Scrub.
        let v = run(
            &dir,
            true,
            Some(&a),
            std::slice::from_ref(&event),
            &[],
            &RecoveryHistorySummary::default(),
        );
        assert!(matches!(v.recommendation, ArbiterDecision::Scrub { .. }));
        assert!(v
            .unresolved_issues
            .iter()
            .any(|i| matches!(i, UnresolvedIssue::OpenEscalation { .. })));
    }

    #[test]
    fn no_audit_yields_default_scrub() {
        let dir = worktree_with_doc();
        let v = run(
            &dir,
            false,
            None,
            &[],
            &[],
            &RecoveryHistorySummary::default(),
        );
        assert_eq!(v.residual_risk, RiskBand::High);
        assert!(matches!(v.recommendation, ArbiterDecision::Scrub { .. }));
        assert!(v.integrated_test_pass.is_none());
    }

    #[test]
    fn medium_risk_with_clean_subtasks_yields_accept() {
        let dir = worktree_with_doc();
        let a = audit(0.75, true);
        // 0.75 → Medium; no flags → Medium not High; Medium + clean = Accept.
        let v = run(
            &dir,
            true,
            Some(&a),
            &[],
            &[],
            &RecoveryHistorySummary::default(),
        );
        assert_eq!(v.residual_risk, RiskBand::Medium);
        assert!(matches!(v.recommendation, ArbiterDecision::Accept(_)));
    }

    #[test]
    fn truncation_event_does_not_block_accept() {
        let dir = worktree_with_doc();
        let a = audit(0.92, true);
        let trunc = MissionEventKind::ContextBudgetTruncated {
            worker_id: "mock-1".into(),
            original_bytes: 12_000,
            rendered_bytes: 8_000,
            dropped_note_ids: vec!["note-a".into(), "note-b".into()],
        };
        let v = run(
            &dir,
            true,
            Some(&a),
            std::slice::from_ref(&trunc),
            &[],
            &RecoveryHistorySummary::default(),
        );
        assert!(matches!(v.recommendation, ArbiterDecision::Accept(_)));
        assert!(v
            .unresolved_issues
            .iter()
            .any(|i| matches!(i, UnresolvedIssue::ContextBudgetTruncated { .. })));
    }

    #[test]
    fn accept_summary_is_non_empty() {
        let dir = worktree_with_doc();
        let a = audit(0.92, true);
        let v = run(
            &dir,
            true,
            Some(&a),
            &[],
            &[],
            &RecoveryHistorySummary::default(),
        );
        if let ArbiterDecision::Accept(AcceptPayload { summary, .. }) = &v.recommendation {
            assert!(!summary.is_empty());
        } else {
            panic!("expected Accept recommendation");
        }
    }
}
