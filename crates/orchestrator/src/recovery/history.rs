//! Per-task ledger of recovery attempts grouped by failure class.
//!
//! Lives in the per-task loop in
//! [`crate::mission_supervisor_run::mission_loop`]; reset between
//! tasks. Used by [`crate::recovery::policy::recover`] to decide
//! whether the per-class retry budget is exhausted.

use std::collections::HashMap;
use std::mem::discriminant;

use crate::recovery::types::FailureClass;

/// Tracks how many times each `FailureClass` variant has fired
/// for the current task. Variants are matched by discriminant
/// (so `MissingFile { path: "a.rs" }` and
/// `MissingFile { path: "b.rs" }` count as the *same* class — the
/// fact that the worker keeps missing files is what matters, not
/// which file).
///
/// Quota exhaustion is intentionally not counted: a paused mission
/// re-dispatches cleanly when the window resets, so it never burns
/// retry budget.
#[derive(Debug, Clone, Default)]
pub struct RecoveryHistory {
    /// Variant discriminant → number of times seen.
    counts: HashMap<&'static str, u8>,
}

impl RecoveryHistory {
    pub fn new() -> Self {
        Self::default()
    }

    /// Increment the count for the given class and return the new
    /// count. Quota exhaustion is a no-op.
    pub(crate) fn record(&mut self, class: &FailureClass) -> u8 {
        if matches!(class, FailureClass::QuotaExhausted { .. }) {
            return 0;
        }
        let key = variant_key(class);
        let entry = self.counts.entry(key).or_insert(0);
        *entry = entry.saturating_add(1);
        *entry
    }

    /// Number of times this class has fired so far. Quota always
    /// returns 0.
    pub(crate) fn count(&self, class: &FailureClass) -> u8 {
        if matches!(class, FailureClass::QuotaExhausted { .. }) {
            return 0;
        }
        let key = variant_key(class);
        self.counts.get(key).copied().unwrap_or(0)
    }

    /// Reset all counters. Called between tasks.
    pub fn reset(&mut self) {
        self.counts.clear();
    }

    /// Sum across all classes. Used as a sanity ceiling — the
    /// per-mission rework budget from the arbiter policy is checked
    /// separately at a higher level.
    pub fn total(&self) -> u8 {
        self.counts.values().fold(0u8, |a, b| a.saturating_add(*b))
    }
}

fn variant_key(class: &FailureClass) -> &'static str {
    let _ = discriminant(class); // forces match arms below to stay exhaustive
    match class {
        FailureClass::MissingFile { .. } => "missing_file",
        FailureClass::CommandError { .. } => "command_error",
        FailureClass::MergeConflict { .. } => "merge_conflict",
        FailureClass::Permissions { .. } => "permissions",
        FailureClass::InadequateContext { .. } => "inadequate_context",
        FailureClass::TaskDrift { .. } => "task_drift",
        FailureClass::VendorCrash { .. } => "vendor_crash",
        FailureClass::QuotaExhausted { .. } => "quota_exhausted",
    }
}

/// Wire-format name for a [`FailureClass`]. Snake_case to match
/// `#[serde(rename_all = "snake_case")]` on the enum. Used by S9's
/// judgment module to bucket recovery activity by class.
pub fn wire_name_for_class(class: &FailureClass) -> &'static str {
    match class {
        FailureClass::MissingFile { .. } => "missing_file",
        FailureClass::CommandError { .. } => "command_error",
        FailureClass::MergeConflict { .. } => "merge_conflict",
        FailureClass::Permissions { .. } => "permissions",
        FailureClass::InadequateContext { .. } => "inadequate_context",
        FailureClass::TaskDrift { .. } => "task_drift",
        FailureClass::VendorCrash { .. } => "vendor_crash",
        FailureClass::QuotaExhausted { .. } => "quota_exhausted",
    }
}

/// Wire-format name for a [`crate::recovery::types::RecoveryAction`].
/// Used by S9's judgment module to label "what did we ultimately
/// do?" per failure class.
pub fn wire_name_for_action(action: &crate::recovery::types::RecoveryAction) -> &'static str {
    match action {
        crate::recovery::types::RecoveryAction::Retry { .. } => "retry",
        crate::recovery::types::RecoveryAction::Pause { .. } => "pause",
        crate::recovery::types::RecoveryAction::Escalate { .. } => "escalate",
        crate::recovery::types::RecoveryAction::RequestSupervisor { .. } => "request_supervisor",
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::recovery::types::{CommandErrorKind, FailureClass};
    use event_schema::Vendor;
    use std::path::PathBuf;

    #[test]
    fn record_increments_then_count_reads() {
        let mut h = RecoveryHistory::new();
        let f = FailureClass::CommandError {
            exit_code: 1,
            kind: CommandErrorKind::Transient,
        };
        assert_eq!(h.count(&f), 0);
        assert_eq!(h.record(&f), 1);
        assert_eq!(h.count(&f), 1);
        assert_eq!(h.record(&f), 2);
        assert_eq!(h.count(&f), 2);
    }

    #[test]
    fn different_variants_count_independently() {
        let mut h = RecoveryHistory::new();
        let a = FailureClass::MissingFile {
            path: PathBuf::from("a.rs"),
        };
        let b = FailureClass::Permissions {
            path: PathBuf::from("/etc/passwd"),
        };
        h.record(&a);
        h.record(&a);
        h.record(&b);
        assert_eq!(h.count(&a), 2);
        assert_eq!(h.count(&b), 1);
        assert_eq!(h.total(), 3);
    }

    #[test]
    fn same_variant_with_different_fields_counts_as_one() {
        let mut h = RecoveryHistory::new();
        let a = FailureClass::MissingFile {
            path: PathBuf::from("a.rs"),
        };
        let b = FailureClass::MissingFile {
            path: PathBuf::from("b.rs"),
        };
        h.record(&a);
        h.record(&b);
        assert_eq!(h.count(&a), 2, "both increment the same bucket");
        assert_eq!(h.count(&b), 2);
        assert_eq!(h.total(), 2);
    }

    #[test]
    fn quota_exhausted_never_counts() {
        let mut h = RecoveryHistory::new();
        let q = FailureClass::QuotaExhausted {
            vendor: Vendor::Claude,
            estimated_reset_at_ms: 0,
        };
        for _ in 0..5 {
            assert_eq!(h.record(&q), 0);
        }
        assert_eq!(h.count(&q), 0);
        assert_eq!(h.total(), 0);
    }

    #[test]
    fn reset_clears_counters() {
        let mut h = RecoveryHistory::new();
        let f = FailureClass::CommandError {
            exit_code: 1,
            kind: CommandErrorKind::Transient,
        };
        h.record(&f);
        h.record(&f);
        h.reset();
        assert_eq!(h.count(&f), 0);
        assert_eq!(h.total(), 0);
    }

    #[test]
    fn wire_name_for_class_matches_serde_rename_all() {
        assert_eq!(
            wire_name_for_class(&FailureClass::MissingFile {
                path: std::path::PathBuf::from("x"),
            }),
            "missing_file",
        );
        assert_eq!(
            wire_name_for_class(&FailureClass::CommandError {
                exit_code: 1,
                kind: CommandErrorKind::Transient,
            }),
            "command_error",
        );
        assert_eq!(
            wire_name_for_class(&FailureClass::QuotaExhausted {
                vendor: event_schema::Vendor::Claude,
                estimated_reset_at_ms: 0,
            }),
            "quota_exhausted",
        );
    }

    #[test]
    fn wire_name_for_action_matches_serde_rename_all() {
        use crate::recovery::types::RecoveryAction;
        assert_eq!(
            wire_name_for_action(&RecoveryAction::Retry { attempt: 1, max: 2 }),
            "retry",
        );
        assert_eq!(
            wire_name_for_action(&RecoveryAction::Pause {
                until_unix_ms: 0,
                reason: crate::mission::PauseReason::WaitingForQuota {
                    vendor: event_schema::Vendor::Claude,
                },
            }),
            "pause",
        );
    }
}
