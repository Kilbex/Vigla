//! [`ArbiterPolicy`] carries the policy values enforced by the pure
//! arbiter and mission scheduler. Runtime settings enforced elsewhere
//! do not belong in this struct.

use serde::{Deserialize, Serialize};
use specta::Type;

/// Authority-model tuning. Every field is consumed by the arbiter or
/// scheduler and has a safe default for `ArbiterPolicy::default()`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Type)]
pub struct ArbiterPolicy {
    /// T2: composite audit-score floor for Accept. Below this →
    /// Extend (if rework budget) or Scrub/Escalate.
    pub quality_min: f64,
    /// T3a: rework attempts allowed per task.
    pub rework_budget_per_task: u8,
    /// T3b: rework attempts allowed per mission (across all tasks).
    pub rework_budget_per_mission: u8,
    /// T5: default audit tier when the auto-selector finds no
    /// hint. `AuditTier::Standard` is the safe default; smoke is
    /// for prototypes, deep for risky changes.
    pub default_audit_tier: crate::audit::AuditTier,
    /// T8: risk-detector list — names of detectors that emit
    /// `Escalate(Risk)` when they fire.
    pub risk_detectors_enabled: RiskDetectorSet,
    /// T10: parallel workers per mission. Currently enforced
    /// upstream in `mission_runtime`; arbiter records the cap so
    /// future per-mission overrides flow through one struct.
    pub max_parallel_workers: u8,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Type)]
pub struct RiskDetectorSet {
    pub schema_migration: bool,
    pub mass_deletion: bool,
    pub secret_files: bool,
}

impl Default for RiskDetectorSet {
    fn default() -> Self {
        Self {
            schema_migration: true,
            mass_deletion: true,
            secret_files: true,
        }
    }
}

impl Default for ArbiterPolicy {
    fn default() -> Self {
        Self {
            quality_min: 0.7,
            rework_budget_per_task: 2,
            rework_budget_per_mission: 3,
            default_audit_tier: crate::audit::AuditTier::Standard,
            risk_detectors_enabled: RiskDetectorSet::default(),
            max_parallel_workers: 4,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_matches_enforced_policy() {
        let p = ArbiterPolicy::default();
        assert!((p.quality_min - 0.7).abs() < 1e-9); // T2
        assert_eq!(p.rework_budget_per_task, 2); // T3a
        assert_eq!(p.rework_budget_per_mission, 3); // T3b
        assert_eq!(p.default_audit_tier, crate::audit::AuditTier::Standard); // T5
        assert!(p.risk_detectors_enabled.schema_migration); // T8
        assert_eq!(p.max_parallel_workers, 4); // T10
    }

    #[test]
    fn serializes_round_trip() {
        let p = ArbiterPolicy::default();
        let json = serde_json::to_string(&p).unwrap();
        let back: ArbiterPolicy = serde_json::from_str(&json).unwrap();
        assert_eq!(p, back);
    }
}
