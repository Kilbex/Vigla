//! Role → vendor mapping. Heuristic, configurable at the
//! mission-spec level. Returns `Some(WorkerVendor)` when the
//! mapping yields a definite answer, `None` when the caller
//! should fall back to its own default.

use crate::task_graph::TaskRole;
use crate::vendor_profile::WorkerVendor;

pub fn select_vendor_for_role(
    role: TaskRole,
    spec_worker_model: Option<&str>,
) -> Option<WorkerVendor> {
    if let Some(s) = spec_worker_model {
        if !s.eq_ignore_ascii_case("auto") {
            if let Some(v) = WorkerVendor::parse(s) {
                return Some(v);
            }
        }
    }
    match role {
        TaskRole::Implementer => None,
        TaskRole::Tester => Some(WorkerVendor::Codex),
        TaskRole::Reviewer => Some(WorkerVendor::Claude),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::task_graph::TaskRole;
    use crate::vendor_profile::WorkerVendor;

    #[test]
    fn implementer_follows_mission_spec() {
        let v = select_vendor_for_role(TaskRole::Implementer, Some("codex"));
        assert_eq!(v, Some(WorkerVendor::Codex));
    }

    #[test]
    fn implementer_with_no_spec_returns_none() {
        let v = select_vendor_for_role(TaskRole::Implementer, None);
        assert_eq!(v, None);
    }

    #[test]
    fn tester_prefers_codex() {
        let v = select_vendor_for_role(TaskRole::Tester, None);
        assert_eq!(v, Some(WorkerVendor::Codex));
    }

    #[test]
    fn tester_respects_explicit_spec() {
        let v = select_vendor_for_role(TaskRole::Tester, Some("claude"));
        assert_eq!(v, Some(WorkerVendor::Claude));
    }

    #[test]
    fn reviewer_prefers_claude() {
        let v = select_vendor_for_role(TaskRole::Reviewer, None);
        assert_eq!(v, Some(WorkerVendor::Claude));
    }

    #[test]
    fn auto_spec_is_treated_as_none() {
        let v = select_vendor_for_role(TaskRole::Tester, Some("auto"));
        assert_eq!(v, Some(WorkerVendor::Codex));
    }

    #[test]
    fn unknown_spec_falls_back_to_role_default() {
        let v = select_vendor_for_role(TaskRole::Reviewer, Some("not-a-vendor"));
        assert_eq!(v, Some(WorkerVendor::Claude));
    }
}
