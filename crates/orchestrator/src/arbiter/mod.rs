//! Authority Model. Consumes a [`crate::audit::AuditReport`]
//! and emits a typed [`ArbiterDecision`].
//!
//! The arbiter is a *pure* policy function. It does not perform
//! integration, IO, or vendor calls. `mission_loop.rs` is
//! responsible for executing the decision (integrate / rework /
//! scrub / escalate).
//!
//! Public entry point: [`decide`]. Sub-modules implement
//! individual bound checks (scope, risk, quality) and the
//! [`ArbiterPolicy`] type that parameterises them.

pub mod bound;
pub mod context;
pub mod decision;
pub mod plan_envelope_check;
pub mod policy;
pub mod quality_check;
pub mod rework;
pub mod rework_dispatch;
pub mod risk_check;
pub mod scope_check;

pub use bound::{AuthorityBound, EscalationEvidence};
pub use context::DecisionContext;
pub use decision::{AcceptPayload, ArbiterDecision, ScrubReason, SuggestedUserAction};
pub use plan_envelope_check::{check as check_plan_envelope, EnvelopeTrip};
pub use policy::ArbiterPolicy;
pub use rework::ReworkKind;
pub use rework_dispatch::{plan_for_kind, NextLoopAction, ReworkPlan};

use crate::audit::AuditReport;

/// Run the arbiter over an audit report. Pure function; deterministic
/// given the same `(audit, ctx, policy)` triple.
///
/// Priority order:
/// 1. Scope check — out-of-scope diff always escalates (user must
///    co-sign or narrow scope).
/// 2. Risk check — a tripped risk detector always escalates.
/// 3. Supervisor-requested rework or quality check — recoverable within the
///    shared rework budget; exhausted quality fails closed with Scrub.
/// 4. Accept — everything in bounds.
///
/// Reversibility requires Git I/O, so it is enforced downstream by
/// `mission_workspace::integrate_worker`; integration failures are emitted as
/// `Escalate(Reversibility)` rather than guessed inside this pure function.
pub fn decide(
    audit: &AuditReport,
    ctx: &DecisionContext,
    policy: &ArbiterPolicy,
) -> ArbiterDecision {
    if let Some(v) = scope_check::check_scope(audit, ctx) {
        return ArbiterDecision::Escalate {
            bound: v.bound,
            evidence: v.evidence,
            suggested_user_action: v.suggested_user_action,
        };
    }

    if let Some(v) = risk_check::check_risk(audit, &policy.risk_detectors_enabled) {
        return ArbiterDecision::Escalate {
            bound: v.bound,
            evidence: v.evidence,
            suggested_user_action: v.suggested_user_action,
        };
    }

    // Automated gates establish a floor; they do not replace the
    // supervisor's semantic review. If that review found an on-task defect,
    // honor its requested rework even when tests and lint are green.
    if ctx.preferred_rework_kind.is_some() {
        return quality_check::rework_or_scrub(audit, ctx, policy);
    }

    if let Some(d) = quality_check::check_quality(audit, ctx, policy) {
        return d;
    }

    ArbiterDecision::Accept(decision::AcceptPayload {
        audit: audit.clone(),
        summary: ctx.submission_summary.clone(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::audit::{AuditReport, ScopeScore, SecurityFlag, SecurityFlagKind};

    fn ctx(summary: &str) -> DecisionContext {
        DecisionContext {
            attempts_used_for_task: 0,
            attempts_used_for_mission: 0,
            submission_summary: summary.to_string(),
            touched_files: vec!["src/lib.rs".to_string()],
            scope_paths: vec!["src".into()],
            preferred_rework_kind: None,
        }
    }

    fn ok_audit() -> AuditReport {
        AuditReport {
            overall: 0.85,
            scope: Some(ScopeScore {
                in_scope: 1,
                out_of_scope: 0,
                score: 1.0,
            }),
            ..AuditReport::default()
        }
    }

    #[test]
    fn happy_path_accepts() {
        let d = decide(&ok_audit(), &ctx("looks good"), &ArbiterPolicy::default());
        match d {
            ArbiterDecision::Accept(p) => assert_eq!(p.summary, "looks good"),
            _ => panic!("expected Accept"),
        }
    }

    #[test]
    fn passing_automated_audit_does_not_override_supervisor_rework() {
        let mut context = ctx("contains a semantic defect");
        context.preferred_rework_kind = Some(ReworkKind::Revise {
            directive: "replace the draft with complete behavior".into(),
        });

        let decision = decide(&ok_audit(), &context, &ArbiterPolicy::default());

        assert!(matches!(decision, ArbiterDecision::Extend { .. }));
    }

    #[test]
    fn mark_unachievable_is_not_blocked_by_an_exhausted_retry_budget() {
        let mut context = ctx("cannot be completed automatically");
        context.attempts_used_for_task = ArbiterPolicy::default().rework_budget_per_task;
        context.attempts_used_for_mission = ArbiterPolicy::default().rework_budget_per_mission;
        context.preferred_rework_kind = Some(ReworkKind::MarkUnachievable {
            rationale: "requires a human-owned signing key".into(),
        });

        let decision = decide(&ok_audit(), &context, &ArbiterPolicy::default());
        assert!(matches!(
            decision,
            ArbiterDecision::Extend {
                rework_kind: ReworkKind::MarkUnachievable { .. },
                attempts_remaining: 0,
            }
        ));
    }

    #[test]
    fn scope_violation_escalates() {
        let mut c = ctx("");
        c.touched_files = vec!["src/lib.rs".into(), "wild/oops.rs".into()];
        let mut a = ok_audit();
        a.scope = Some(ScopeScore {
            in_scope: 1,
            out_of_scope: 1,
            score: 0.5,
        });
        let d = decide(&a, &c, &ArbiterPolicy::default());
        assert!(matches!(
            d,
            ArbiterDecision::Escalate {
                bound: AuthorityBound::Scope,
                ..
            }
        ));
    }

    #[test]
    fn risk_flag_escalates() {
        let mut a = ok_audit();
        a.security_flags.push(SecurityFlag {
            kind: SecurityFlagKind::SecretFile,
            path: ".env".into(),
            detail: String::new(),
        });
        let d = decide(&a, &ctx(""), &ArbiterPolicy::default());
        assert!(matches!(
            d,
            ArbiterDecision::Escalate {
                bound: AuthorityBound::Risk,
                ..
            }
        ));
    }

    #[test]
    fn low_quality_with_budget_extends() {
        let mut a = ok_audit();
        a.overall = 0.5;
        let d = decide(&a, &ctx(""), &ArbiterPolicy::default());
        assert!(matches!(d, ArbiterDecision::Extend { .. }));
    }

    #[test]
    fn low_quality_exhausted_scrubs() {
        let mut a = ok_audit();
        a.overall = 0.5;
        let mut c = ctx("");
        c.attempts_used_for_task = 2;
        c.attempts_used_for_mission = 3;
        let d = decide(&a, &c, &ArbiterPolicy::default());
        assert!(matches!(d, ArbiterDecision::Scrub { .. }));
    }

    #[test]
    fn empty_audit_is_unusable() {
        let d = decide(&AuditReport::default(), &ctx(""), &ArbiterPolicy::default());
        assert!(matches!(
            d,
            ArbiterDecision::Scrub {
                reason: ScrubReason::AuditUnusable,
                ..
            }
        ));
    }
}

#[cfg(test)]
mod proptest_decide {
    use super::*;
    use crate::audit::{AuditReport, ScopeScore, SecurityFlag, SecurityFlagKind};
    use proptest::prelude::*;

    fn any_overall() -> impl Strategy<Value = f64> {
        prop_oneof![Just(0.0), Just(0.5), Just(0.7), Just(0.85), Just(1.0)]
    }

    fn any_scope_score() -> impl Strategy<Value = ScopeScore> {
        (0u32..5, 0u32..5).prop_map(|(in_s, out)| ScopeScore {
            in_scope: in_s,
            out_of_scope: out,
            score: if in_s + out == 0 {
                1.0
            } else {
                in_s as f64 / (in_s + out) as f64
            },
        })
    }

    fn any_attempts() -> impl Strategy<Value = (u8, u8)> {
        (0u8..6, 0u8..6)
    }

    proptest! {
        #[test]
        fn escalation_never_accepts(overall in any_overall(), scope in any_scope_score()) {
            let audit = AuditReport {
                overall,
                scope: Some(scope.clone()),
                security_flags: vec![SecurityFlag {
                    kind: SecurityFlagKind::SecretFile,
                    path: ".env".into(),
                    detail: String::new(),
                }],
                ..AuditReport::default()
            };

            // Risk should always trigger escalation, regardless of
            // quality or scope.
            let d = decide(&audit, &DecisionContext::for_test(""), &ArbiterPolicy::default());
            let ok = matches!(d, ArbiterDecision::Escalate { bound: AuthorityBound::Risk, .. });
            prop_assert!(ok, "expected Escalate(Risk), got {:?}", d);
        }

        #[test]
        fn budget_exhausted_never_extends(
            overall in 0.0f64..0.69,
            (used_task, used_mission) in any_attempts(),
        ) {
            let audit = AuditReport {
                overall,
                scope: Some(ScopeScore {
                    in_scope: 1,
                    out_of_scope: 0,
                    score: 1.0,
                }),
                ..AuditReport::default()
            };

            let mut ctx = DecisionContext::for_test("");
            ctx.attempts_used_for_task = used_task;
            ctx.attempts_used_for_mission = used_mission;
            let policy = ArbiterPolicy::default();

            let task_remaining = policy.rework_budget_per_task.saturating_sub(used_task);
            let mission_remaining = policy.rework_budget_per_mission.saturating_sub(used_mission);
            let budget_left = task_remaining.min(mission_remaining);

            let d = decide(&audit, &ctx, &policy);
            if budget_left == 0 {
                let ok = matches!(d, ArbiterDecision::Scrub { .. });
                prop_assert!(ok, "expected Scrub (budget=0), got {:?}", d);
            } else {
                let ok = matches!(d, ArbiterDecision::Extend { .. });
                prop_assert!(ok, "expected Extend (budget={}), got {:?}", budget_left, d);
            }
        }

        #[test]
        fn passing_score_with_no_violations_always_accepts(
            overall in 0.7f64..=1.0,
        ) {
            let audit = AuditReport {
                overall,
                scope: Some(ScopeScore {
                    in_scope: 1,
                    out_of_scope: 0,
                    score: 1.0,
                }),
                ..AuditReport::default()
            };

            let d = decide(&audit, &DecisionContext::for_test(""), &ArbiterPolicy::default());
            let ok = matches!(d, ArbiterDecision::Accept(_));
            prop_assert!(ok, "expected Accept, got {:?}", d);
        }
    }
}
