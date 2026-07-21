//! Drift heuristic.
//!
//! Consumes the audit's [`ScopeScore`] and decides whether the
//! worker's touched-file pattern crossed the "no longer working
//! on what we asked for" threshold. Pure function — no IO, no
//! mutation. Recovery's policy consults this when classifying
//! failures so a TaskDrift class is produced before the worker is
//! re-dispatched with a generic "improve" directive.
//!
//! The heuristic has two knobs:
//! - `min_touched`: ignore drift on tiny diffs. A worker that
//!   touches 1 file outside scope is not "drifting"; they may have
//!   missed an import. Below this floor we report `NoDrift`.
//! - `threshold_ratio`: minimum fraction of out-of-scope files
//!   that triggers drift. Default 0.35 — > 1/3 of touched files
//!   outside scope.

use std::path::PathBuf;

use crate::audit::ScopeScore;

#[derive(Debug, Clone, Copy)]
pub struct ScopeDriftHeuristic {
    pub min_touched: u32,
    pub threshold_ratio: f64,
}

impl Default for ScopeDriftHeuristic {
    fn default() -> Self {
        Self {
            min_touched: 3,
            threshold_ratio: 0.35,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ScopeDriftVerdict {
    NoDrift,
    Drift {
        ratio_pct: u32, // 0..=100, for the inbox display
    },
}

impl ScopeDriftHeuristic {
    pub fn evaluate(&self, score: &ScopeScore) -> ScopeDriftVerdict {
        let total = score.in_scope + score.out_of_scope;
        if total < self.min_touched {
            return ScopeDriftVerdict::NoDrift;
        }
        if score.out_of_scope == 0 {
            return ScopeDriftVerdict::NoDrift;
        }
        let ratio = score.out_of_scope as f64 / total as f64;
        if ratio >= self.threshold_ratio {
            ScopeDriftVerdict::Drift {
                ratio_pct: (ratio * 100.0).round() as u32,
            }
        } else {
            ScopeDriftVerdict::NoDrift
        }
    }
}

/// Convenience: build a `FailureClass::TaskDrift` from a verdict +
/// the touched-files / declared-scope lists. Returns `None` if the
/// verdict is `NoDrift`.
pub fn drift_to_failure_class(
    verdict: &ScopeDriftVerdict,
    observed_files: Vec<String>,
    declared_scope: Vec<PathBuf>,
) -> Option<crate::recovery::types::FailureClass> {
    match verdict {
        ScopeDriftVerdict::NoDrift => None,
        ScopeDriftVerdict::Drift { .. } => Some(crate::recovery::types::FailureClass::TaskDrift {
            observed_files,
            declared_scope,
        }),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn s(in_scope: u32, out_of_scope: u32) -> ScopeScore {
        let total = (in_scope + out_of_scope) as f64;
        let score = if total == 0.0 {
            1.0
        } else {
            in_scope as f64 / total
        };
        ScopeScore {
            in_scope,
            out_of_scope,
            score,
        }
    }

    #[test]
    fn empty_diff_is_no_drift() {
        let h = ScopeDriftHeuristic::default();
        assert_eq!(h.evaluate(&s(0, 0)), ScopeDriftVerdict::NoDrift);
    }

    #[test]
    fn tiny_diff_below_min_touched_is_no_drift() {
        let h = ScopeDriftHeuristic::default();
        // 1 file in, 1 file out = 50% out-of-scope but total < 3
        assert_eq!(h.evaluate(&s(1, 1)), ScopeDriftVerdict::NoDrift);
    }

    #[test]
    fn all_in_scope_is_no_drift() {
        let h = ScopeDriftHeuristic::default();
        assert_eq!(h.evaluate(&s(10, 0)), ScopeDriftVerdict::NoDrift);
    }

    #[test]
    fn one_third_out_of_scope_above_floor_is_drift() {
        let h = ScopeDriftHeuristic::default();
        match h.evaluate(&s(4, 3)) {
            ScopeDriftVerdict::Drift { ratio_pct } => {
                assert!((40..=50).contains(&ratio_pct));
            }
            other => panic!("expected Drift, got {other:?}"),
        }
    }

    #[test]
    fn just_below_threshold_is_no_drift() {
        // 10 in, 4 out → 28% out → below default 35%.
        let h = ScopeDriftHeuristic::default();
        assert_eq!(h.evaluate(&s(10, 4)), ScopeDriftVerdict::NoDrift);
    }

    #[test]
    fn drift_to_failure_class_wraps_observed_and_declared() {
        let h = ScopeDriftHeuristic::default();
        let verdict = h.evaluate(&s(4, 3));
        let class = drift_to_failure_class(
            &verdict,
            vec!["unrelated/a.rs".into(), "unrelated/b.rs".into()],
            vec![PathBuf::from("src")],
        );
        assert!(matches!(
            class,
            Some(crate::recovery::types::FailureClass::TaskDrift { .. })
        ));
    }

    #[test]
    fn no_drift_does_not_produce_failure_class() {
        let class = drift_to_failure_class(&ScopeDriftVerdict::NoDrift, vec![], vec![]);
        assert!(class.is_none());
    }
}
