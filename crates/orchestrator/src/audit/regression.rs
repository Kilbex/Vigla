//! Compare a pre-mission test baseline against the post-integration
//! current state.
//!
//! v1 keeps this simple: aggregate counts plus optionally-supplied
//! lists of test names that newly fail or newly pass. Per-test
//! tracking requires structured test output (libtest JSON), which
//! is unstable; v1 accepts what the caller can derive. Empty
//! `newly_failing` means "no detected regressions."

use crate::audit::report::{clamp_score, RegressionScore, TestPassScore};

/// Score a regression delta.
///
/// If `baseline` is `None`, we have no comparison point; return
/// `score = 1.0` (no penalty). If baseline itself was failing,
/// treat the regression check as a no-op to avoid double-penalising
/// the worker for pre-existing breakage. Otherwise score 1.0 when
/// no regressions, 0.0 when explicit newly_failing names are
/// supplied, 0.5 when current has failures but caller didn't list
/// them by name (soft regression).
/// The `newly_passing` list is stored as context for the caller
/// but does not affect the score in v1.
pub fn score_regression(
    baseline: Option<&TestPassScore>,
    current: &TestPassScore,
    newly_passing: &[String],
    newly_failing: &[String],
) -> RegressionScore {
    let Some(b) = baseline else {
        return RegressionScore {
            baseline_passed: false,
            current_passed: current.failed == 0,
            newly_failing: vec![],
            newly_passing: vec![],
            score: clamp_score(1.0),
        };
    };

    let baseline_passed = b.failed == 0;
    let current_passed = current.failed == 0;

    if !baseline_passed {
        return RegressionScore {
            baseline_passed: false,
            current_passed,
            newly_failing: vec![],
            newly_passing: vec![],
            score: clamp_score(1.0),
        };
    }

    let raw_score = if newly_failing.is_empty() && current_passed {
        1.0
    } else if !newly_failing.is_empty() {
        0.0
    } else {
        // baseline passed but current has failures the caller didn't list
        // by name — treat as a soft regression.
        // Deliberately use a stable soft penalty: this path knows only that
        // the aggregate regressed, not which tests are newly failing.
        0.5
    };

    RegressionScore {
        baseline_passed: true,
        current_passed,
        newly_failing: newly_failing.to_vec(),
        newly_passing: newly_passing.to_vec(),
        score: clamp_score(raw_score),
    }
}

/// Score regression only when a baseline exists.
///
/// Regression measures *delta vs a baseline*; with no baseline (e.g. a
/// first run, before any baseline capture) there is nothing to compare,
/// so this returns `None`. Returning `None` (rather than a `1.0` score)
/// keeps the component out of [`super::composite::blend_overall`]'s
/// denominator entirely — otherwise a baseline-less run earns a free
/// full-weight `1.0`, inflating `overall` toward the integration
/// quality floor with no actual regression signal (F-1).
pub fn regression_if_baselined(
    baseline: Option<&TestPassScore>,
    current: &TestPassScore,
    newly_passing: &[String],
    newly_failing: &[String],
) -> Option<RegressionScore> {
    baseline?;
    Some(score_regression(
        baseline,
        current,
        newly_passing,
        newly_failing,
    ))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::audit::report::TestPassScore;

    fn pass(n: u32) -> TestPassScore {
        TestPassScore {
            ran: true,
            passed: n,
            failed: 0,
            skipped: 0,
            score: 1.0,
        }
    }

    fn fail(n_pass: u32, n_fail: u32) -> TestPassScore {
        TestPassScore {
            ran: true,
            passed: n_pass,
            failed: n_fail,
            skipped: 0,
            score: n_pass as f64 / (n_pass + n_fail) as f64,
        }
    }

    #[test]
    fn no_baseline_means_unscored_score_one() {
        let baseline = None;
        let current = pass(10);
        let r = score_regression(baseline.as_ref(), &current, &[], &[]);
        assert_eq!(r.score, 1.0);
        assert!(r.newly_failing.is_empty());
    }

    #[test]
    fn no_regression_when_both_pass() {
        let baseline = Some(pass(10));
        let current = pass(10);
        let r = score_regression(baseline.as_ref(), &current, &[], &[]);
        assert_eq!(r.score, 1.0);
    }

    #[test]
    fn introduced_failures_score_zero() {
        let baseline = Some(pass(10));
        let current = fail(8, 2);
        let r = score_regression(
            baseline.as_ref(),
            &current,
            &[],
            &["t::a".into(), "t::b".into()],
        );
        assert_eq!(
            r.newly_failing,
            vec!["t::a".to_string(), "t::b".to_string()]
        );
        assert_eq!(r.score, 0.0);
    }

    #[test]
    fn soft_regression_scores_half_when_failures_unlisted() {
        let baseline = Some(pass(10));
        let current = fail(8, 2); // caller doesn't supply newly_failing
        let r = score_regression(baseline.as_ref(), &current, &[], &[]);
        assert_eq!(r.score, 0.5);
        assert!(r.newly_failing.is_empty());
    }

    #[test]
    fn regression_excluded_from_blend_when_no_baseline() {
        // F-1: with no baseline there is nothing to compare, so the
        // component must be None (excluded from the blend denominator)
        // rather than a free 1.0 that inflates overall.
        let current = pass(10);
        assert!(
            regression_if_baselined(None, &current, &[], &[]).is_none(),
            "no baseline must yield None"
        );
        let baseline = pass(10);
        assert!(
            regression_if_baselined(Some(&baseline), &current, &[], &[]).is_some(),
            "with a baseline, regression is scored normally"
        );
    }
}
