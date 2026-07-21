//! Compose `AuditReport.overall` from the sub-scores via a
//! configurable weight profile.
//!
//! Default weights match the T2 quality floor described in the
//! roadmap (0.7 is the supervisor-as-arbiter default acceptance
//! threshold). Unscored sub-scores (None) contribute 0 weight.
//! Final result routed through [`crate::audit::report::clamp_score`]
//! to inherit the NaN guard.

use crate::audit::report::{clamp_score, AuditReport};

#[derive(Debug, Clone)]
pub struct WeightProfile {
    pub test_pass: f64,
    pub scope: f64,
    pub regression: f64,
    pub lint: f64,
    /// Subtracted from the final score per SecurityFlag entry.
    pub security_penalty_per_flag: f64,
}

impl Default for WeightProfile {
    fn default() -> Self {
        let profile = Self {
            test_pass: 0.40,
            scope: 0.20,
            regression: 0.25,
            lint: 0.15,
            security_penalty_per_flag: 0.10,
        };
        debug_assert!(
            (profile.test_pass + profile.scope + profile.regression + profile.lint - 1.0).abs()
                < 1e-9,
            "WeightProfile blending weights must sum to 1.0"
        );
        profile
    }
}

/// Compute the blended overall score. Unscored components do not
/// participate in the denominator, so a Smoke audit (test+scope
/// only) can still produce a meaningful overall.
pub fn blend_overall(r: &AuditReport, w: &WeightProfile) -> f64 {
    let mut numerator = 0.0;
    let mut denominator = 0.0;

    if let Some(s) = &r.test_pass {
        numerator += s.score * w.test_pass;
        denominator += w.test_pass;
    }
    if let Some(s) = &r.scope {
        numerator += s.score * w.scope;
        denominator += w.scope;
    }
    if let Some(s) = &r.regression {
        numerator += s.score * w.regression;
        denominator += w.regression;
    }
    if let Some(s) = &r.lint {
        numerator += s.score * w.lint;
        denominator += w.lint;
    }

    let base = if denominator == 0.0 {
        0.0
    } else {
        numerator / denominator
    };
    let penalty = r.security_flags.len() as f64 * w.security_penalty_per_flag;
    clamp_score(base - penalty)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::audit::report::{AuditReport, LintScore, ScopeScore, TestPassScore};

    fn perfect_test() -> TestPassScore {
        TestPassScore {
            ran: true,
            passed: 5,
            failed: 0,
            skipped: 0,
            score: 1.0,
        }
    }

    fn perfect_scope() -> ScopeScore {
        ScopeScore {
            in_scope: 3,
            out_of_scope: 0,
            score: 1.0,
        }
    }

    fn perfect_lint() -> LintScore {
        LintScore {
            rustfmt_clean: Some(true),
            clippy_warnings: Some(0),
            biome_diagnostics: None,
            score: 1.0,
        }
    }

    #[test]
    fn empty_report_blends_to_zero() {
        let r = AuditReport::default();
        assert_eq!(blend_overall(&r, &WeightProfile::default()), 0.0);
    }

    #[test]
    fn all_perfect_blends_to_one() {
        let r = AuditReport {
            overall: 0.0,
            test_pass: Some(perfect_test()),
            scope: Some(perfect_scope()),
            regression: None,
            lint: Some(perfect_lint()),
            security_flags: vec![],
        };
        let got = blend_overall(&r, &WeightProfile::default());
        assert!((got - 1.0).abs() < 1e-6, "expected ≈1.0, got {got}");
    }

    #[test]
    fn security_flag_lowers_score() {
        let r = AuditReport {
            overall: 0.0,
            test_pass: Some(perfect_test()),
            scope: Some(perfect_scope()),
            regression: None,
            lint: Some(perfect_lint()),
            security_flags: vec![crate::audit::report::SecurityFlag {
                kind: crate::audit::report::SecurityFlagKind::SchemaMigration,
                path: "migrations/x.sql".into(),
                detail: "".into(),
            }],
        };
        let got = blend_overall(&r, &WeightProfile::default());
        assert!(got < 1.0, "security flag should reduce score, got {got}");
    }

    #[test]
    fn partial_score_normalizes_via_denominator() {
        // Only test_pass present; the denominator normalization
        // should hand back exactly that sub-score regardless of
        // how heavily test_pass is weighted relative to the others.
        let r = AuditReport {
            overall: 0.0,
            test_pass: Some(TestPassScore {
                ran: true,
                passed: 6,
                failed: 4,
                skipped: 0,
                score: 0.6,
            }),
            scope: None,
            regression: None,
            lint: None,
            security_flags: vec![],
        };
        let got = blend_overall(&r, &WeightProfile::default());
        assert!(
            (got - 0.6).abs() < 1e-6,
            "Smoke-tier partial audit should pass-through the sole sub-score; got {got}"
        );
    }
}
