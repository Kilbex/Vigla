//! Per-task descriptor extensions. The base [`crate::mission_event::TaskDescriptor`]
//! holds the canonical wire shape; this module exports the typed enums and
//! structs used by its policy fields.

use serde::{Deserialize, Serialize};
use specta::Type;

/// What kind of work this task represents. The arbiter uses this to
/// pick a default vendor in [`super::role_routing::select_vendor_for_role`].
///
/// Distinct from [`crate::mission_runtime::WorkerRole`] (which is the
/// Supervisor/Employee boundary check). Do not reuse that name.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Type)]
#[serde(rename_all = "snake_case")]
#[derive(Default)]
pub enum TaskRole {
    /// Default — produces production code or docs.
    #[default]
    Implementer,
    /// Writes / extends tests, runs the suite, surfaces regressions.
    Tester,
    /// Reads the integrated branch, surfaces critique or polish.
    Reviewer,
}

/// Per-task pass/fail conditions evaluated post-audit. Each field
/// is optional; an empty `AcceptanceCriteria` reduces to "any
/// arbiter Accept is fine".
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize, Type)]
pub struct AcceptanceCriteria {
    /// Minimum composite audit score for this specific task.
    /// `None` falls back to `ArbiterPolicy::quality_min`.
    #[serde(default)]
    pub min_audit_overall: Option<f64>,
    /// If `Some(true)`, the task's audit must include a TestPass
    /// sub-score with `failed == 0`. `Some(false)` explicitly
    /// allows test failures (e.g. for a docs-only task).
    #[serde(default)]
    pub require_tests_pass: Option<bool>,
    /// If `Some(true)`, the task must NOT introduce any new
    /// security flags. `Some(false)` is interpreted as "the
    /// supervisor has reviewed and accepts the risk".
    #[serde(default)]
    pub forbid_new_security_flags: Option<bool>,
    /// Human-readable summary. Surfaced in the inbox card on
    /// criteria failure; never used to make a decision.
    #[serde(default)]
    pub summary: Option<String>,
}

/// Result of folding [`AcceptanceCriteria`] over a worker's
/// [`crate::audit::AuditReport`]. Returned by
/// [`super::criteria_eval::evaluate`].
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Type)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum CriteriaOutcome {
    /// All declared criteria satisfied. The arbiter's decision
    /// stands as-is.
    Pass,
    /// One or more criteria failed. `reasons` lists each. The
    /// caller folds this into an arbiter Quality bound (Extend if
    /// rework budget remains, else Scrub).
    Fail { reasons: Vec<String> },
}

/// Compute the effective scope paths for a task.
///
/// - Empty `task_scope` → inherit `mission_scope` (no narrowing).
/// - Non-empty `task_scope` with empty `mission_scope` → use
///   `task_scope` as-is (mission says "everything in scope").
/// - Both non-empty → intersection (each task path must live
///   inside some mission path via [`std::path::Path::starts_with`]).
pub fn effective_scope_paths(
    task_scope: &[std::path::PathBuf],
    mission_scope: &[std::path::PathBuf],
) -> Vec<std::path::PathBuf> {
    if task_scope.is_empty() {
        return mission_scope.to_vec();
    }
    if mission_scope.is_empty() {
        return task_scope.to_vec();
    }
    task_scope
        .iter()
        .filter(|task_path| {
            mission_scope
                .iter()
                .any(|mission_path| task_path.starts_with(mission_path))
        })
        .cloned()
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn task_role_default_is_implementer() {
        assert_eq!(TaskRole::default(), TaskRole::Implementer);
    }

    #[test]
    fn task_role_round_trips_snake_case() {
        let r = TaskRole::Tester;
        let json = serde_json::to_string(&r).unwrap();
        assert_eq!(json, "\"tester\"");
        let back: TaskRole = serde_json::from_str(&json).unwrap();
        assert_eq!(r, back);
    }

    #[test]
    fn acceptance_criteria_default_is_empty() {
        let c = AcceptanceCriteria::default();
        assert!(c.min_audit_overall.is_none());
        assert!(c.require_tests_pass.is_none());
        assert!(c.forbid_new_security_flags.is_none());
        assert!(c.summary.is_none());
    }

    #[test]
    fn criteria_outcome_pass_is_kindless() {
        let o = CriteriaOutcome::Pass;
        let json = serde_json::to_string(&o).unwrap();
        assert!(json.contains("\"kind\":\"pass\""));
        let back: CriteriaOutcome = serde_json::from_str(&json).unwrap();
        assert_eq!(o, back);
    }

    #[test]
    fn criteria_outcome_fail_carries_reasons() {
        let o = CriteriaOutcome::Fail {
            reasons: vec!["audit below per-task floor".into()],
        };
        let json = serde_json::to_string(&o).unwrap();
        assert!(json.contains("\"kind\":\"fail\""));
        let back: CriteriaOutcome = serde_json::from_str(&json).unwrap();
        assert_eq!(o, back);
    }

    #[test]
    fn effective_scope_empty_per_task_inherits_mission() {
        let mission_scope = vec![std::path::PathBuf::from("src")];
        let task_scope: Vec<std::path::PathBuf> = vec![];
        let eff = effective_scope_paths(&task_scope, &mission_scope);
        assert_eq!(eff, mission_scope);
    }

    #[test]
    fn effective_scope_narrows_inside_mission() {
        let mission_scope = vec![std::path::PathBuf::from("src")];
        let task_scope = vec![std::path::PathBuf::from("src/auth")];
        let eff = effective_scope_paths(&task_scope, &mission_scope);
        assert_eq!(eff, vec![std::path::PathBuf::from("src/auth")]);
    }

    #[test]
    fn effective_scope_outside_mission_drops() {
        let mission_scope = vec![std::path::PathBuf::from("src")];
        let task_scope = vec![std::path::PathBuf::from("tests")];
        let eff = effective_scope_paths(&task_scope, &mission_scope);
        assert!(eff.is_empty());
    }

    #[test]
    fn effective_scope_mission_empty_returns_per_task() {
        let mission_scope: Vec<std::path::PathBuf> = vec![];
        let task_scope = vec![std::path::PathBuf::from("src/auth")];
        let eff = effective_scope_paths(&task_scope, &mission_scope);
        assert_eq!(eff, task_scope);
    }

    #[test]
    fn effective_scope_both_empty_is_empty() {
        let eff = effective_scope_paths(&[], &[]);
        assert!(eff.is_empty());
    }
}
