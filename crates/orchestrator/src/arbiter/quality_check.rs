//! Quality-bound check. If audit.overall >= policy.quality_min, no
//! violation. Otherwise:
//! - rework budget remaining → request Extend(Revise)
//! - budget exhausted → request Scrub(QualityExhausted)
//! - audit returned no usable score (overall == 0.0 *and* every
//!   sub-score is None) → Scrub(AuditUnusable)
//!
//! Returned variant is the *decision*, since quality can recover
//! within budget without escalating.

use crate::arbiter::{
    context::DecisionContext,
    decision::{ArbiterDecision, ScrubReason},
    policy::ArbiterPolicy,
    rework::ReworkKind,
};
use crate::audit::AuditReport;

pub fn check_quality(
    audit: &AuditReport,
    ctx: &DecisionContext,
    policy: &ArbiterPolicy,
) -> Option<ArbiterDecision> {
    if audit_unusable(audit) {
        return Some(ArbiterDecision::Scrub {
            reason: ScrubReason::AuditUnusable,
            retained_artifacts: vec![],
            partial_audit: Some(audit.clone()),
        });
    }

    // Any executed test failure is a hard non-Accept gate. The composite
    // remains useful for ranking quality, but cannot average a regression
    // away with green scope and lint scores.
    if audit
        .test_pass
        .as_ref()
        .is_some_and(|tests| tests.ran && (tests.failed > 0 || tests.score < 1.0))
    {
        return Some(rework_or_scrub(audit, ctx, policy));
    }

    if audit.overall >= policy.quality_min {
        return None;
    }

    Some(rework_or_scrub(audit, ctx, policy))
}

/// Apply the shared task/mission rework budgets to an explicit supervisor
/// rework request or an automated quality-floor failure.
pub(crate) fn rework_or_scrub(
    audit: &AuditReport,
    ctx: &DecisionContext,
    policy: &ArbiterPolicy,
) -> ArbiterDecision {
    // MarkUnachievable is a terminal supervisor declaration, not another
    // worker attempt. Preserve its rationale even when the retry budget is
    // already exhausted; the dispatcher moves the mission to Attention
    // without reserving a rework slot.
    if let Some(rework_kind @ ReworkKind::MarkUnachievable { .. }) =
        ctx.preferred_rework_kind.clone()
    {
        return ArbiterDecision::Extend {
            rework_kind,
            attempts_remaining: 0,
        };
    }

    let task_left = policy
        .rework_budget_per_task
        .saturating_sub(ctx.attempts_used_for_task);
    let mission_left = policy
        .rework_budget_per_mission
        .saturating_sub(ctx.attempts_used_for_mission);
    let attempts_remaining = task_left.min(mission_left);

    if attempts_remaining > 0 {
        let rework_kind = ctx
            .preferred_rework_kind
            .clone()
            .unwrap_or_else(|| ReworkKind::Revise {
                directive: String::new(),
            });
        ArbiterDecision::Extend {
            rework_kind,
            attempts_remaining,
        }
    } else {
        ArbiterDecision::Scrub {
            reason: ScrubReason::QualityExhausted,
            retained_artifacts: vec![],
            partial_audit: Some(audit.clone()),
        }
    }
}

fn audit_unusable(audit: &AuditReport) -> bool {
    audit.overall == 0.0
        && audit.test_pass.is_none()
        && audit.scope.is_none()
        && audit.regression.is_none()
        && audit.lint.is_none()
        && audit.security_flags.is_empty()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::audit::{AuditReport, ScopeScore, TestPassScore};

    fn audit_overall(overall: f64) -> AuditReport {
        AuditReport {
            overall,
            // Populate one sub-score so unusable detector doesn't fire.
            scope: Some(ScopeScore {
                in_scope: 1,
                out_of_scope: 0,
                score: 1.0,
            }),
            ..AuditReport::default()
        }
    }

    fn policy_with_budget(per_task: u8, per_mission: u8) -> ArbiterPolicy {
        ArbiterPolicy {
            rework_budget_per_task: per_task,
            rework_budget_per_mission: per_mission,
            ..ArbiterPolicy::default()
        }
    }

    fn ctx(task: u8, mission: u8) -> DecisionContext {
        let mut c = DecisionContext::for_test("");
        c.attempts_used_for_task = task;
        c.attempts_used_for_mission = mission;
        c
    }

    #[test]
    fn passing_score_returns_none() {
        let r = check_quality(&audit_overall(0.8), &ctx(0, 0), &policy_with_budget(2, 3));
        assert!(r.is_none());
    }

    #[test]
    fn failing_test_suite_cannot_be_averaged_into_acceptance() {
        let mut audit = audit_overall(0.99);
        audit.test_pass = Some(TestPassScore {
            ran: true,
            passed: 99,
            failed: 1,
            skipped: 0,
            score: 0.99,
        });
        let decision = check_quality(&audit, &ctx(0, 0), &policy_with_budget(2, 3))
            .expect("one failed test must block Accept");
        assert!(matches!(decision, ArbiterDecision::Extend { .. }));
    }

    #[test]
    fn failing_score_with_budget_extends() {
        let r = check_quality(&audit_overall(0.5), &ctx(0, 0), &policy_with_budget(2, 3))
            .expect("violation");
        match r {
            ArbiterDecision::Extend {
                rework_kind: ReworkKind::Revise { .. },
                attempts_remaining,
            } => {
                assert_eq!(attempts_remaining, 2);
            }
            _ => panic!("expected Extend(Revise)"),
        }
    }

    #[test]
    fn failing_score_exhausted_per_task_scrubs() {
        let r = check_quality(&audit_overall(0.5), &ctx(2, 2), &policy_with_budget(2, 3))
            .expect("violation");
        match r {
            ArbiterDecision::Scrub {
                reason: ScrubReason::QualityExhausted,
                ..
            } => {}
            _ => panic!("expected Scrub(QualityExhausted)"),
        }
    }

    #[test]
    fn failing_score_exhausted_per_mission_scrubs() {
        let r = check_quality(&audit_overall(0.5), &ctx(0, 3), &policy_with_budget(2, 3))
            .expect("violation");
        match r {
            ArbiterDecision::Scrub { .. } => {}
            _ => panic!("expected Scrub"),
        }
    }

    #[test]
    fn empty_default_audit_is_unusable() {
        let audit = AuditReport::default();
        let r = check_quality(&audit, &ctx(0, 0), &policy_with_budget(2, 3)).expect("violation");
        match r {
            ArbiterDecision::Scrub {
                reason: ScrubReason::AuditUnusable,
                ..
            } => {}
            _ => panic!("expected Scrub(AuditUnusable)"),
        }
    }
}
