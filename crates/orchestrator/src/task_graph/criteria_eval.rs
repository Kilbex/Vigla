//! Acceptance-criteria evaluator. Pure function over an
//! [`AuditReport`] and an [`AcceptanceCriteria`]; returns either
//! `Pass` or `Fail { reasons }`. The mission_loop folds a `Fail`
//! into the arbiter's Quality bound (Extend within budget,
//! otherwise Scrub).

use crate::audit::AuditReport;
use crate::task_graph::descriptor::{AcceptanceCriteria, CriteriaOutcome};

pub fn evaluate(criteria: &AcceptanceCriteria, audit: &AuditReport) -> CriteriaOutcome {
    let mut reasons: Vec<String> = Vec::new();

    if let Some(floor) = criteria.min_audit_overall {
        if audit.overall < floor {
            reasons.push(format!(
                "min_audit_overall: {:.2} below per-task floor {:.2}",
                audit.overall, floor,
            ));
        }
    }

    if let Some(require) = criteria.require_tests_pass {
        if require {
            match &audit.test_pass {
                None => {
                    reasons.push("require_tests_pass: test runner did not record results".into())
                }
                Some(tp) if tp.failed > 0 => {
                    reasons.push(format!("require_tests_pass: {} failing test(s)", tp.failed,))
                }
                Some(tp) if !tp.ran => {
                    reasons.push("require_tests_pass: test runner did not run".into())
                }
                _ => {}
            }
        }
    }

    if let Some(forbid) = criteria.forbid_new_security_flags {
        if forbid && !audit.security_flags.is_empty() {
            let names: Vec<String> = audit
                .security_flags
                .iter()
                .map(|f| format!("{:?}:{}", f.kind, f.path))
                .collect();
            reasons.push(format!("forbid_new_security_flags: {}", names.join(", "),));
        }
    }

    if reasons.is_empty() {
        CriteriaOutcome::Pass
    } else {
        CriteriaOutcome::Fail { reasons }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::audit::{AuditReport, ScopeScore, SecurityFlag, SecurityFlagKind, TestPassScore};
    use crate::task_graph::AcceptanceCriteria;

    fn audit_with_overall(overall: f64) -> AuditReport {
        AuditReport {
            overall,
            scope: Some(ScopeScore {
                in_scope: 1,
                out_of_scope: 0,
                score: 1.0,
            }),
            ..AuditReport::default()
        }
    }

    #[test]
    fn empty_criteria_always_passes() {
        let r = audit_with_overall(0.5);
        assert_eq!(
            evaluate(&AcceptanceCriteria::default(), &r),
            CriteriaOutcome::Pass
        );
    }

    #[test]
    fn min_overall_satisfied() {
        let r = audit_with_overall(0.85);
        let c = AcceptanceCriteria {
            min_audit_overall: Some(0.8),
            ..Default::default()
        };
        assert_eq!(evaluate(&c, &r), CriteriaOutcome::Pass);
    }

    #[test]
    fn min_overall_violated() {
        let r = audit_with_overall(0.6);
        let c = AcceptanceCriteria {
            min_audit_overall: Some(0.8),
            ..Default::default()
        };
        let outcome = evaluate(&c, &r);
        match outcome {
            CriteriaOutcome::Fail { reasons } => {
                assert!(reasons.iter().any(|r| r.contains("min_audit_overall")));
            }
            _ => panic!("expected Fail"),
        }
    }

    #[test]
    fn require_tests_pass_with_no_failures_passes() {
        let mut r = audit_with_overall(0.85);
        r.test_pass = Some(TestPassScore {
            ran: true,
            passed: 10,
            failed: 0,
            skipped: 0,
            score: 1.0,
        });
        let c = AcceptanceCriteria {
            require_tests_pass: Some(true),
            ..Default::default()
        };
        assert_eq!(evaluate(&c, &r), CriteriaOutcome::Pass);
    }

    #[test]
    fn require_tests_pass_with_failures_fails() {
        let mut r = audit_with_overall(0.85);
        r.test_pass = Some(TestPassScore {
            ran: true,
            passed: 9,
            failed: 1,
            skipped: 0,
            score: 0.9,
        });
        let c = AcceptanceCriteria {
            require_tests_pass: Some(true),
            ..Default::default()
        };
        match evaluate(&c, &r) {
            CriteriaOutcome::Fail { reasons } => {
                assert!(reasons.iter().any(|r| r.contains("test")));
            }
            _ => panic!("expected Fail"),
        }
    }

    #[test]
    fn require_tests_pass_with_no_test_run_fails() {
        let r = audit_with_overall(0.85);
        let c = AcceptanceCriteria {
            require_tests_pass: Some(true),
            ..Default::default()
        };
        match evaluate(&c, &r) {
            CriteriaOutcome::Fail { reasons } => {
                assert!(reasons.iter().any(|r| r.contains("test")));
            }
            _ => panic!("expected Fail"),
        }
    }

    #[test]
    fn forbid_security_flags_with_no_flags_passes() {
        let r = audit_with_overall(0.85);
        let c = AcceptanceCriteria {
            forbid_new_security_flags: Some(true),
            ..Default::default()
        };
        assert_eq!(evaluate(&c, &r), CriteriaOutcome::Pass);
    }

    #[test]
    fn forbid_security_flags_with_flag_fails() {
        let mut r = audit_with_overall(0.85);
        r.security_flags.push(SecurityFlag {
            kind: SecurityFlagKind::SecretFile,
            path: ".env".into(),
            detail: String::new(),
        });
        let c = AcceptanceCriteria {
            forbid_new_security_flags: Some(true),
            ..Default::default()
        };
        match evaluate(&c, &r) {
            CriteriaOutcome::Fail { reasons } => {
                assert!(reasons.iter().any(|r| r.contains("security")));
            }
            _ => panic!("expected Fail"),
        }
    }

    #[test]
    fn multiple_failures_accumulate_reasons() {
        let r = audit_with_overall(0.4);
        let c = AcceptanceCriteria {
            min_audit_overall: Some(0.8),
            require_tests_pass: Some(true),
            ..Default::default()
        };
        match evaluate(&c, &r) {
            CriteriaOutcome::Fail { reasons } => {
                assert!(reasons.len() >= 2);
            }
            _ => panic!("expected Fail"),
        }
    }
}
