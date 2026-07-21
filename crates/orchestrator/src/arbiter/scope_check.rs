//! Scope-bound check. Reads [`crate::audit::ScopeScore`]
//! (`in_scope` / `out_of_scope` counts) and the policy. If any
//! files were touched outside the declared scope_paths, returns the
//! list of paths for evidence. Empty `scope_paths` means
//! "unconstrained" — the check always passes.

use crate::arbiter::{
    bound::{AuthorityBound, EscalationEvidence},
    context::DecisionContext,
    decision::SuggestedUserAction,
};
use crate::audit::AuditReport;
use std::path::{Path, PathBuf};

pub struct ScopeViolation {
    pub bound: AuthorityBound,
    pub evidence: EscalationEvidence,
    pub suggested_user_action: SuggestedUserAction,
}

/// Return `Some(ScopeViolation)` if any of `ctx.touched_files` is
/// outside `ctx.scope_paths`. `None` means no violation.
///
/// The arbiter's own path-walk is the authoritative scope gate. The
/// `audit` argument is accepted for signature parity with the other bound
/// checks but is intentionally NOT consulted for the decision: a stale or
/// buggy `audit.scope` count must never be able to wave an out-of-scope
/// submission through to Accept (the audit summarises counts; the arbiter
/// decides) (F-15).
pub fn check_scope(audit: &AuditReport, ctx: &DecisionContext) -> Option<ScopeViolation> {
    let _ = audit; // scope is decided by the path-walk below, not the audit count

    // Empty allow-list = unconstrained.
    if ctx.scope_paths.is_empty() {
        return None;
    }

    let oos: Vec<String> = ctx
        .touched_files
        .iter()
        .filter(|f| !is_in_scope(f, &ctx.scope_paths))
        .cloned()
        .collect();

    if oos.is_empty() {
        return None;
    }

    let payload = serde_json::to_string(&oos).ok();
    Some(ScopeViolation {
        bound: AuthorityBound::Scope,
        evidence: EscalationEvidence {
            summary: format!(
                "Worker touched {} file(s) outside declared scope",
                oos.len()
            ),
            payload_json: payload,
        },
        suggested_user_action: SuggestedUserAction::ConfirmScope {
            out_of_scope_paths: oos,
        },
    })
}

fn is_in_scope(file: &str, scope_paths: &[PathBuf]) -> bool {
    let p = Path::new(file);
    scope_paths.iter().any(|sp| p.starts_with(sp))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::audit::{AuditReport, ScopeScore};

    fn ctx(touched: &[&str], scope: &[&str]) -> DecisionContext {
        DecisionContext {
            attempts_used_for_task: 0,
            attempts_used_for_mission: 0,
            submission_summary: String::new(),
            touched_files: touched.iter().map(|s| s.to_string()).collect(),
            scope_paths: scope.iter().map(PathBuf::from).collect(),
            preferred_rework_kind: None,
        }
    }

    fn audit_with_scope(in_scope: u32, out_of_scope: u32) -> AuditReport {
        AuditReport {
            scope: Some(ScopeScore {
                in_scope,
                out_of_scope,
                score: if out_of_scope == 0 {
                    1.0
                } else {
                    in_scope as f64 / (in_scope + out_of_scope) as f64
                },
            }),
            ..AuditReport::default()
        }
    }

    #[test]
    fn empty_scope_list_is_unconstrained() {
        let c = ctx(&["any/file.rs"], &[]);
        assert!(check_scope(&audit_with_scope(0, 0), &c).is_none());
    }

    #[test]
    fn touched_inside_scope_passes() {
        let c = ctx(&["src/lib.rs", "src/util.rs"], &["src"]);
        assert!(check_scope(&audit_with_scope(2, 0), &c).is_none());
    }

    #[test]
    fn touched_outside_scope_violates() {
        let c = ctx(&["src/lib.rs", "wild/oops.rs"], &["src"]);
        let v = check_scope(&audit_with_scope(1, 1), &c).expect("violation");
        assert_eq!(v.bound, AuthorityBound::Scope);
        assert!(v.evidence.summary.contains("1"));
        match v.suggested_user_action {
            SuggestedUserAction::ConfirmScope { out_of_scope_paths } => {
                assert_eq!(out_of_scope_paths, vec!["wild/oops.rs".to_string()]);
            }
            _ => panic!("expected ConfirmScope"),
        }
    }

    #[test]
    fn path_walk_is_authority_even_when_audit_count_is_zero() {
        // F-15: the arbiter's path-walk is the authoritative scope gate.
        // Even if the audit's ScopeScore under-reports (out_of_scope == 0)
        // due to a stale or buggy scorer, a touched file outside scope must
        // still escalate — the audit count must not be able to wave it
        // through.
        let c = ctx(&["src/lib.rs", "wild/oops.rs"], &["src"]);
        let audit = AuditReport {
            scope: Some(ScopeScore {
                in_scope: 2,
                out_of_scope: 0,
                score: 1.0,
            }),
            ..AuditReport::default()
        };
        let v =
            check_scope(&audit, &c).expect("path-walk must catch OOS regardless of audit count");
        assert_eq!(v.bound, AuthorityBound::Scope);
        match v.suggested_user_action {
            SuggestedUserAction::ConfirmScope { out_of_scope_paths } => {
                assert_eq!(out_of_scope_paths, vec!["wild/oops.rs".to_string()]);
            }
            _ => panic!("expected ConfirmScope"),
        }
    }
}
