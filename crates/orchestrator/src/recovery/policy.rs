//! Deterministic recovery transition: [`FailureClass`] + mutable
//! [`RecoveryHistory`] → [`RecoveryAction`].
//!
//! One match arm per failure class. Compiler enforces exhaustiveness
//! — adding a `FailureClass` variant breaks this match and forces
//! whoever added the variant to articulate its recovery policy.
//!
//! Defaults (overridable via [`RecoveryPolicy`]):
//! - Missing file: 1 retry budget (the first retry is the supervisor
//!   trying to supply context; the second escalates as Scope).
//! - Transient command error: 1 retry; persistent: 0 retries.
//! - Merge conflict: 0 retries — escalate as Quality (S4 owns the
//!   auto-rebase first attempt).
//! - Permissions: 0 retries — escalate as Risk (user co-sign).
//! - Inadequate context: never blocks; emit RequestSupervisor.
//! - Task drift: never blocks; emit RequestSupervisor + 1 rework
//!   directive worth of retry budget.
//! - Vendor crash: 2 retries; escalate as Risk after.

use crate::arbiter::{AuthorityBound, EscalationEvidence};
use crate::recovery::history::RecoveryHistory;
use crate::recovery::types::{
    CommandErrorKind, FailureClass, PauseReason, RecoveryAction, SupervisorRequestKind,
};

/// Tunable retry budgets. Defaults match the per-class rationale in
/// the module docstring. Lives next to the policy because changing a
/// budget here has visible behavioral consequences — moving a knob
/// is intentionally a code change, not a config edit.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RecoveryPolicy {
    pub missing_file_retries: u8,
    pub transient_command_retries: u8,
    pub persistent_command_retries: u8,
    pub merge_conflict_retries: u8,
    pub permissions_retries: u8,
    pub task_drift_retries: u8,
    pub vendor_crash_retries: u8,
}

impl Default for RecoveryPolicy {
    fn default() -> Self {
        Self {
            missing_file_retries: 1,
            transient_command_retries: 1,
            persistent_command_retries: 0,
            merge_conflict_retries: 0,
            permissions_retries: 0,
            task_drift_retries: 1,
            vendor_crash_retries: 2,
        }
    }
}

/// Decide the next action and then record this failure occurrence.
///
/// Keeping those operations in one API is intentional: retry budgets are
/// based on prior decisions, so recording before deciding silently consumes
/// the first retry. `now_unix_ms` is supplied by the caller so the decision
/// remains deterministic in tests.
pub fn recover(
    class: &FailureClass,
    history: &mut RecoveryHistory,
    policy: &RecoveryPolicy,
    now_unix_ms: u64,
) -> RecoveryAction {
    let action = decide(class, history, policy, now_unix_ms);
    history.record(class);
    action
}

fn decide(
    class: &FailureClass,
    history: &RecoveryHistory,
    policy: &RecoveryPolicy,
    now_unix_ms: u64,
) -> RecoveryAction {
    let _ = now_unix_ms; // reserved for the QuotaExhausted branch
    let attempts_so_far = history.count(class);
    match class {
        FailureClass::MissingFile { path } => {
            if attempts_so_far < policy.missing_file_retries {
                RecoveryAction::RequestSupervisor {
                    kind: SupervisorRequestKind::NeedsContext {
                        request: crate::recovery::types::ContextRequest {
                            kind: crate::recovery::types::ContextRequestKind::FileContent,
                            detail: format!("missing file: {}", path.display()),
                        },
                    },
                }
            } else {
                RecoveryAction::Escalate {
                    bound: AuthorityBound::Scope,
                    evidence: EscalationEvidence {
                        summary: format!(
                            "worker repeatedly referenced missing file: {}",
                            path.display()
                        ),
                        payload_json: None,
                    },
                }
            }
        }
        FailureClass::CommandError { exit_code, kind } => {
            let max = match kind {
                CommandErrorKind::Transient => policy.transient_command_retries,
                CommandErrorKind::Persistent => policy.persistent_command_retries,
            };
            if attempts_so_far < max {
                RecoveryAction::Retry {
                    attempt: attempts_so_far + 1,
                    max,
                }
            } else {
                RecoveryAction::Escalate {
                    bound: AuthorityBound::Quality,
                    evidence: EscalationEvidence {
                        summary: format!(
                            "command exited {exit_code} ({:?}); retry budget {max} exhausted",
                            kind
                        ),
                        payload_json: None,
                    },
                }
            }
        }
        FailureClass::MergeConflict { against_ref } => {
            if attempts_so_far < policy.merge_conflict_retries {
                RecoveryAction::Retry {
                    attempt: attempts_so_far + 1,
                    max: policy.merge_conflict_retries,
                }
            } else {
                RecoveryAction::Escalate {
                    bound: AuthorityBound::Quality,
                    evidence: EscalationEvidence {
                        summary: format!(
                            "merge conflict against {against_ref}; S4 auto-rebase did not resolve"
                        ),
                        payload_json: None,
                    },
                }
            }
        }
        FailureClass::Permissions { path } => RecoveryAction::Escalate {
            bound: AuthorityBound::Risk,
            evidence: EscalationEvidence {
                summary: format!("permission denied on {}", path.display()),
                payload_json: None,
            },
        },
        FailureClass::InadequateContext { request } => RecoveryAction::RequestSupervisor {
            kind: SupervisorRequestKind::NeedsContext {
                request: request.clone(),
            },
        },
        FailureClass::TaskDrift {
            observed_files,
            declared_scope,
        } => {
            if attempts_so_far < policy.task_drift_retries {
                RecoveryAction::RequestSupervisor {
                    kind: SupervisorRequestKind::DriftDetected {
                        observed_files: observed_files.clone(),
                        declared_scope: declared_scope.clone(),
                    },
                }
            } else {
                RecoveryAction::Escalate {
                    bound: AuthorityBound::Scope,
                    evidence: EscalationEvidence {
                        summary: format!(
                            "drift persisted: {} files outside scope after rework",
                            observed_files.len()
                        ),
                        payload_json: None,
                    },
                }
            }
        }
        FailureClass::VendorCrash {
            vendor,
            last_exit_code,
            signal,
        } => {
            if attempts_so_far < policy.vendor_crash_retries {
                RecoveryAction::Retry {
                    attempt: attempts_so_far + 1,
                    max: policy.vendor_crash_retries,
                }
            } else {
                let detail = match (last_exit_code, signal) {
                    (Some(code), false) => format!("{vendor:?} exited {code} repeatedly"),
                    (Some(code), true) => format!("{vendor:?} killed by signal (last code {code})"),
                    (None, true) => format!("{vendor:?} killed by signal"),
                    (None, false) => format!("{vendor:?} died without a recoverable exit code"),
                };
                RecoveryAction::Escalate {
                    bound: AuthorityBound::Risk,
                    evidence: EscalationEvidence {
                        summary: detail,
                        payload_json: None,
                    },
                }
            }
        }
        FailureClass::QuotaExhausted {
            vendor,
            estimated_reset_at_ms,
        } => RecoveryAction::Pause {
            until_unix_ms: *estimated_reset_at_ms,
            reason: PauseReason::WaitingForQuota { vendor: *vendor },
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::recovery::types::{ContextRequest, ContextRequestKind};
    use event_schema::Vendor;
    use std::path::PathBuf;

    fn policy() -> RecoveryPolicy {
        RecoveryPolicy::default()
    }

    #[test]
    fn missing_file_first_time_requests_supervisor() {
        let class = FailureClass::MissingFile {
            path: PathBuf::from("src/lib.rs"),
        };
        let action = recover(&class, &mut RecoveryHistory::new(), &policy(), 0);
        assert!(matches!(
            action,
            RecoveryAction::RequestSupervisor {
                kind: SupervisorRequestKind::NeedsContext { .. },
            }
        ));
    }

    #[test]
    fn missing_file_after_budget_escalates_scope() {
        let class = FailureClass::MissingFile {
            path: PathBuf::from("src/lib.rs"),
        };
        let mut h = RecoveryHistory::new();
        h.record(&class); // 1 attempt already burned
        let action = recover(&class, &mut h, &policy(), 0);
        assert!(matches!(
            action,
            RecoveryAction::Escalate {
                bound: AuthorityBound::Scope,
                ..
            }
        ));
    }

    #[test]
    fn transient_command_error_retries_once() {
        let class = FailureClass::CommandError {
            exit_code: 75,
            kind: CommandErrorKind::Transient,
        };
        let action = recover(&class, &mut RecoveryHistory::new(), &policy(), 0);
        assert!(matches!(
            action,
            RecoveryAction::Retry { attempt: 1, max: 1 }
        ));
    }

    #[test]
    fn transient_command_error_after_retry_escalates_quality() {
        let class = FailureClass::CommandError {
            exit_code: 75,
            kind: CommandErrorKind::Transient,
        };
        let mut h = RecoveryHistory::new();
        h.record(&class);
        let action = recover(&class, &mut h, &policy(), 0);
        assert!(matches!(
            action,
            RecoveryAction::Escalate {
                bound: AuthorityBound::Quality,
                ..
            }
        ));
    }

    #[test]
    fn persistent_command_error_escalates_immediately() {
        let class = FailureClass::CommandError {
            exit_code: 2,
            kind: CommandErrorKind::Persistent,
        };
        let action = recover(&class, &mut RecoveryHistory::new(), &policy(), 0);
        assert!(matches!(
            action,
            RecoveryAction::Escalate {
                bound: AuthorityBound::Quality,
                ..
            }
        ));
    }

    #[test]
    fn permissions_always_escalates_risk() {
        let class = FailureClass::Permissions {
            path: PathBuf::from("/etc/passwd"),
        };
        let action = recover(&class, &mut RecoveryHistory::new(), &policy(), 0);
        assert!(matches!(
            action,
            RecoveryAction::Escalate {
                bound: AuthorityBound::Risk,
                ..
            }
        ));
    }

    #[test]
    fn inadequate_context_emits_supervisor_request_unbounded() {
        let req = ContextRequest {
            kind: ContextRequestKind::Documentation,
            detail: "async-trait".into(),
        };
        let class = FailureClass::InadequateContext {
            request: req.clone(),
        };
        let action = recover(&class, &mut RecoveryHistory::new(), &policy(), 0);
        assert!(matches!(
            action,
            RecoveryAction::RequestSupervisor {
                kind: SupervisorRequestKind::NeedsContext { .. },
            }
        ));
    }

    #[test]
    fn task_drift_first_time_requests_supervisor() {
        let class = FailureClass::TaskDrift {
            observed_files: vec!["unrelated/a.rs".into()],
            declared_scope: vec![PathBuf::from("src/")],
        };
        let action = recover(&class, &mut RecoveryHistory::new(), &policy(), 0);
        assert!(matches!(
            action,
            RecoveryAction::RequestSupervisor {
                kind: SupervisorRequestKind::DriftDetected { .. },
            }
        ));
    }

    #[test]
    fn task_drift_after_budget_escalates_scope() {
        let class = FailureClass::TaskDrift {
            observed_files: vec!["unrelated/a.rs".into()],
            declared_scope: vec![PathBuf::from("src/")],
        };
        let mut h = RecoveryHistory::new();
        h.record(&class);
        let action = recover(&class, &mut h, &policy(), 0);
        assert!(matches!(
            action,
            RecoveryAction::Escalate {
                bound: AuthorityBound::Scope,
                ..
            }
        ));
    }

    #[test]
    fn vendor_crash_retries_twice_then_escalates_risk() {
        let class = FailureClass::VendorCrash {
            vendor: Vendor::Claude,
            last_exit_code: Some(139),
            signal: true,
        };
        let mut h = RecoveryHistory::new();
        // First fire: attempt 1, max 2
        let a = recover(&class, &mut h, &policy(), 0);
        assert!(matches!(a, RecoveryAction::Retry { attempt: 1, max: 2 }));
        // Second fire
        let a = recover(&class, &mut h, &policy(), 0);
        assert!(matches!(a, RecoveryAction::Retry { attempt: 2, max: 2 }));
        // Third fire: escalate
        let a = recover(&class, &mut h, &policy(), 0);
        assert!(matches!(
            a,
            RecoveryAction::Escalate {
                bound: AuthorityBound::Risk,
                ..
            }
        ));
    }

    #[test]
    fn quota_exhausted_pauses_until_estimated_reset() {
        let class = FailureClass::QuotaExhausted {
            vendor: Vendor::Claude,
            estimated_reset_at_ms: 1_716_018_000_000,
        };
        let action = recover(&class, &mut RecoveryHistory::new(), &policy(), 0);
        match action {
            RecoveryAction::Pause {
                until_unix_ms,
                reason,
            } => {
                assert_eq!(until_unix_ms, 1_716_018_000_000);
                assert_eq!(
                    reason,
                    PauseReason::WaitingForQuota {
                        vendor: Vendor::Claude
                    }
                );
            }
            other => panic!("expected Pause, got {other:?}"),
        }
    }

    #[test]
    fn quota_exhausted_ignores_history() {
        let class = FailureClass::QuotaExhausted {
            vendor: Vendor::Claude,
            estimated_reset_at_ms: 1_716_018_000_000,
        };
        let mut h = RecoveryHistory::new();
        for _ in 0..10 {
            h.record(&class); // record() is a no-op for quota
        }
        let action = recover(&class, &mut h, &policy(), 0);
        assert!(matches!(action, RecoveryAction::Pause { .. }));
    }

    #[test]
    fn merge_conflict_escalates_quality_immediately() {
        let class = FailureClass::MergeConflict {
            against_ref: "supervisor/main".into(),
        };
        let action = recover(&class, &mut RecoveryHistory::new(), &policy(), 0);
        assert!(matches!(
            action,
            RecoveryAction::Escalate {
                bound: AuthorityBound::Quality,
                ..
            }
        ));
    }
}
