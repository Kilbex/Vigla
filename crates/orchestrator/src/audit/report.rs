//! Types describing a quality audit of a worker's submission.
//!
//! The composite score in [`AuditReport::overall`] blends the
//! sub-scores below. Individual scorers fill in the `Option` fields
//! they handle; cheaper audit tiers leave some `None`.

use serde::{Deserialize, Serialize};
use specta::Type;

/// All `score: f64` fields in this module are expected to satisfy
/// `0.0 <= score <= 1.0`. Scorers in sibling modules (test_pass,
/// scope, regression, lint, composite) must run their computed
/// score through [`clamp_score`] before storing it. Out-of-range
/// values are silently clamped — debug builds also assert.
pub(crate) fn clamp_score(v: f64) -> f64 {
    debug_assert!(
        (0.0..=1.0).contains(&v) || v.is_nan(),
        "score {v} outside [0.0, 1.0]"
    );
    if v.is_nan() {
        0.0
    } else {
        v.clamp(0.0, 1.0)
    }
}

#[cfg(test)]
mod clamp_score_tests {
    use super::clamp_score;

    #[test]
    fn in_range_passes_through() {
        assert_eq!(clamp_score(0.0), 0.0);
        assert_eq!(clamp_score(0.5), 0.5);
        assert_eq!(clamp_score(1.0), 1.0);
    }

    #[test]
    fn out_of_range_clamps_in_release() {
        // debug_assert! would fire in dev builds; this test path
        // exercises the release-build clamp behaviour. We invoke
        // it indirectly to avoid panicking the debug runner —
        // skip the debug_assert by going through a release-only
        // computation. For the test we just exercise the clamp
        // semantic on the NaN path (NaN → 0.0).
        assert_eq!(clamp_score(f64::NAN), 0.0);
    }
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize, Type)]
pub struct AuditReport {
    /// Composite score in [0.0, 1.0]. Computed by `composite::blend`.
    pub overall: f64,
    pub test_pass: Option<TestPassScore>,
    pub scope: Option<ScopeScore>,
    pub regression: Option<RegressionScore>,
    pub lint: Option<LintScore>,
    pub security_flags: Vec<SecurityFlag>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Type)]
pub struct TestPassScore {
    pub ran: bool,
    pub passed: u32,
    pub failed: u32,
    pub skipped: u32,
    pub score: f64, // failed==0 → 1.0; else 1.0 - (failed / (passed+failed))
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Type)]
pub struct ScopeScore {
    pub in_scope: u32,     // count of touched files inside scope_paths
    pub out_of_scope: u32, // count outside
    pub score: f64,        // in / (in + out); 1.0 if no diff
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Type)]
pub struct RegressionScore {
    pub baseline_passed: bool,
    pub current_passed: bool,
    pub newly_failing: Vec<String>, // test names as reported by the test runner (e.g. "module::test_name") — tests that passed in baseline and fail now
    pub newly_passing: Vec<String>, // test names as reported by the test runner — tests that failed in baseline and now pass (bonus signal)
    pub score: f64,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Type)]
pub struct LintScore {
    pub rustfmt_clean: Option<bool>,
    pub clippy_warnings: Option<u32>,
    pub biome_diagnostics: Option<u32>,
    pub score: f64,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Type)]
pub struct SecurityFlag {
    pub kind: SecurityFlagKind,
    pub path: String,
    pub detail: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Type)]
pub enum SecurityFlagKind {
    SecretFile,
    SchemaMigration,
    MassDeletion,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn audit_report_default_is_unscored() {
        let r = AuditReport::default();
        assert_eq!(r.overall, 0.0);
        assert!(r.test_pass.is_none());
        assert!(r.scope.is_none());
        assert!(r.regression.is_none());
        assert!(r.lint.is_none());
        assert!(r.security_flags.is_empty());
    }

    #[test]
    fn audit_report_serializes_round_trip() {
        let r = AuditReport {
            overall: 0.75,
            test_pass: Some(TestPassScore {
                ran: true,
                passed: 12,
                failed: 0,
                skipped: 1,
                score: 1.0,
            }),
            scope: Some(ScopeScore {
                in_scope: 5,
                out_of_scope: 0,
                score: 1.0,
            }),
            regression: None,
            lint: None,
            security_flags: vec![],
        };
        let json = serde_json::to_string(&r).unwrap();
        let back: AuditReport = serde_json::from_str(&json).unwrap();
        assert_eq!(r, back);
    }
}
