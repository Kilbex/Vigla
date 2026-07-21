//! Audit tier — determines which scorers run.
//!
//! Tiers map to time budgets: Smoke ≤30s, Standard ≤2min,
//! Deep ≤5min (enforced by per-scorer timeouts in their modules,
//! not here). This module decides *which* tier applies to a given
//! submission.

use serde::{Deserialize, Serialize};
use specta::Type;

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize, Type)]
#[serde(rename_all = "snake_case")]
pub enum AuditTier {
    #[default]
    Smoke, // cargo check + scope + security only
    Standard, // smoke + tests for changed crates + lint
    Deep,     // standard + full-workspace tests + regression
}

#[derive(Debug, Clone)]
pub struct TierInput {
    pub files_changed: u32,
    pub lines_changed: u32,
    pub risk_hits: u32, // count of SecurityFlag entries (any kind) detected pre-audit
}

impl AuditTier {
    /// Auto-select a tier. Risky diffs always escalate to Deep.
    /// Large diffs without risk hits go to Standard or Deep based
    /// on size thresholds.
    pub fn auto_select(input: &TierInput) -> Self {
        if input.risk_hits > 0 {
            return AuditTier::Deep;
        }
        if input.files_changed > 20 || input.lines_changed > 1000 {
            return AuditTier::Deep;
        }
        if input.files_changed > 3 || input.lines_changed > 50 {
            return AuditTier::Standard;
        }
        AuditTier::Smoke
    }
}

impl std::fmt::Display for AuditTier {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let s = match self {
            AuditTier::Smoke => "smoke",
            AuditTier::Standard => "standard",
            AuditTier::Deep => "deep",
        };
        f.write_str(s)
    }
}

#[cfg(test)]
mod display_tests {
    use super::*;

    #[test]
    fn display_matches_serde_snake_case() {
        assert_eq!(AuditTier::Smoke.to_string(), "smoke");
        assert_eq!(AuditTier::Standard.to_string(), "standard");
        assert_eq!(AuditTier::Deep.to_string(), "deep");
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn smoke_is_default() {
        assert_eq!(AuditTier::default(), AuditTier::Smoke);
    }

    #[test]
    fn select_smoke_for_trivial_diff() {
        let input = TierInput {
            files_changed: 1,
            lines_changed: 5,
            risk_hits: 0,
        };
        assert_eq!(AuditTier::auto_select(&input), AuditTier::Smoke);
    }

    #[test]
    fn select_standard_for_moderate_diff() {
        let input = TierInput {
            files_changed: 8,
            lines_changed: 200,
            risk_hits: 0,
        };
        assert_eq!(AuditTier::auto_select(&input), AuditTier::Standard);
    }

    #[test]
    fn select_deep_for_risky_diff() {
        let input = TierInput {
            files_changed: 2,
            lines_changed: 10,
            risk_hits: 1,
        };
        assert_eq!(AuditTier::auto_select(&input), AuditTier::Deep);
    }

    #[test]
    fn select_deep_for_large_diff() {
        let input = TierInput {
            files_changed: 30,
            lines_changed: 2000,
            risk_hits: 0,
        };
        assert_eq!(AuditTier::auto_select(&input), AuditTier::Deep);
    }

    #[test]
    fn boundary_files_20_is_standard_not_deep() {
        let input = TierInput {
            files_changed: 20,
            lines_changed: 0,
            risk_hits: 0,
        };
        assert_eq!(AuditTier::auto_select(&input), AuditTier::Standard);
    }

    #[test]
    fn boundary_files_21_crosses_to_deep() {
        let input = TierInput {
            files_changed: 21,
            lines_changed: 0,
            risk_hits: 0,
        };
        assert_eq!(AuditTier::auto_select(&input), AuditTier::Deep);
    }

    #[test]
    fn boundary_lines_1000_is_standard_not_deep() {
        let input = TierInput {
            files_changed: 0,
            lines_changed: 1000,
            risk_hits: 0,
        };
        assert_eq!(AuditTier::auto_select(&input), AuditTier::Standard);
    }

    #[test]
    fn boundary_lines_1001_crosses_to_deep() {
        let input = TierInput {
            files_changed: 0,
            lines_changed: 1001,
            risk_hits: 0,
        };
        assert_eq!(AuditTier::auto_select(&input), AuditTier::Deep);
    }

    #[test]
    fn boundary_files_3_is_smoke_not_standard() {
        let input = TierInput {
            files_changed: 3,
            lines_changed: 0,
            risk_hits: 0,
        };
        assert_eq!(AuditTier::auto_select(&input), AuditTier::Smoke);
    }

    #[test]
    fn boundary_files_4_crosses_to_standard() {
        let input = TierInput {
            files_changed: 4,
            lines_changed: 0,
            risk_hits: 0,
        };
        assert_eq!(AuditTier::auto_select(&input), AuditTier::Standard);
    }

    #[test]
    fn boundary_lines_50_is_smoke_not_standard() {
        let input = TierInput {
            files_changed: 0,
            lines_changed: 50,
            risk_hits: 0,
        };
        assert_eq!(AuditTier::auto_select(&input), AuditTier::Smoke);
    }

    #[test]
    fn boundary_lines_51_crosses_to_standard() {
        let input = TierInput {
            files_changed: 0,
            lines_changed: 51,
            risk_hits: 0,
        };
        assert_eq!(AuditTier::auto_select(&input), AuditTier::Standard);
    }
}
