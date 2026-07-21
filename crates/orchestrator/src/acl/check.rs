//! Pure pre-integration gate: given a worker's diff path list and
//! its effective [`FileAcl`], return either `Ok(())` (every path
//! is inside the allow-list) or an [`AclViolation`] enumerating
//! the denied paths.
//!
//! Called from the mission loop **before** the audit pass so an
//! obvious scope violation does not waste audit subprocess time.
//! The audit's own scope subscore remains as the slower, more
//! granular backup signal.

use crate::acl::FileAcl;
use serde::{Deserialize, Serialize};

/// Pre-integration gate. Empty `diff_paths` is always Ok (worker
/// produced no changes — nothing to audit). Unconstrained ACL is
/// always Ok (no mission scope declared).
pub fn check_diff(diff_paths: &[String], acl: &FileAcl) -> Result<(), AclViolation> {
    if acl.is_unconstrained() {
        return Ok(());
    }

    let mut denied: Vec<String> = Vec::new();
    let mut allowed_count: u32 = 0;
    for f in diff_paths {
        if acl.is_path_allowed(f) {
            allowed_count = allowed_count.saturating_add(1);
        } else {
            denied.push(f.clone());
        }
    }

    if denied.is_empty() {
        Ok(())
    } else {
        Err(AclViolation {
            denied_count: denied.len() as u32,
            allowed_count,
            denied_paths: denied,
        })
    }
}

/// Outcome of a violated ACL check. The payload is small enough
/// to ship inside `EscalationEvidence.payload_json`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AclViolation {
    /// Files inside the allow-list that the worker did touch.
    pub allowed_count: u32,
    /// Files outside the allow-list — count and the path list.
    pub denied_count: u32,
    pub denied_paths: Vec<String>,
}

impl AclViolation {
    /// Short human-readable summary for `EscalationEvidence.summary`.
    /// Stay under ~150 chars so inbox cards don't truncate.
    pub fn summary(&self) -> String {
        format!(
            "Worker submitted {} file(s) outside declared scope (allowed: {})",
            self.denied_count, self.allowed_count,
        )
    }

    /// Serialize the violation as the JSON payload the supervisor
    /// hands to `EscalationEvidence.payload_json`. Stable shape —
    /// inbox cards can rely on the `denied_paths` field.
    pub fn payload_json(&self) -> String {
        serde_json::to_string(self).unwrap_or_else(|_| "{}".to_string())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::acl::FileAcl;
    use std::path::PathBuf;

    fn pb(s: &str) -> PathBuf {
        PathBuf::from(s)
    }

    #[test]
    fn unconstrained_acl_admits_everything() {
        let acl = FileAcl::from_mission_and_task(&[], None);
        let files = vec!["a.rs".to_string(), "b/c.rs".to_string()];
        assert!(check_diff(&files, &acl).is_ok());
    }

    #[test]
    fn all_files_inside_scope_is_ok() {
        let acl = FileAcl::from_mission_and_task(&[pb("src")], None);
        let files = vec!["src/lib.rs".to_string(), "src/util.rs".to_string()];
        assert!(check_diff(&files, &acl).is_ok());
    }

    #[test]
    fn one_file_outside_scope_violates() {
        let acl = FileAcl::from_mission_and_task(&[pb("src")], None);
        let files = vec!["src/lib.rs".to_string(), "docs/README.md".to_string()];
        let err = check_diff(&files, &acl).expect_err("expected violation");
        assert_eq!(err.denied_paths, vec!["docs/README.md".to_string()]);
        assert_eq!(err.allowed_count, 1);
        assert_eq!(err.denied_count, 1);
    }

    #[test]
    fn empty_diff_with_constrained_acl_is_ok() {
        let acl = FileAcl::from_mission_and_task(&[pb("src")], None);
        let files: Vec<String> = Vec::new();
        assert!(check_diff(&files, &acl).is_ok());
    }

    #[test]
    fn empty_intersection_denies_every_touched_file() {
        let acl = FileAcl::from_mission_and_task(&[pb("src")], Some(&[pb("docs")]));
        let files = vec!["src/lib.rs".to_string()];
        let err = check_diff(&files, &acl).expect_err("expected violation");
        assert_eq!(err.denied_paths, vec!["src/lib.rs".to_string()]);
    }

    #[test]
    fn violation_summary_includes_count() {
        let acl = FileAcl::from_mission_and_task(&[pb("src")], None);
        let files = vec![
            "wild/a.rs".to_string(),
            "wild/b.rs".to_string(),
            "src/ok.rs".to_string(),
        ];
        let err = check_diff(&files, &acl).expect_err("expected violation");
        assert_eq!(err.denied_count, 2);
        assert!(err.summary().contains("2"));
    }
}
