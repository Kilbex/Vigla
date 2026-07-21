//! Mission-level unresolved-issues collector + recovery summary.
//!
//! Pure functions over a filtered slice of [`MissionEventKind`].
//! No IO — the mission_loop driver feeds in the events it
//! observed during the mission run, and the collector returns a
//! deterministic `Vec<UnresolvedIssue>`.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};
use specta::Type;

use crate::judgment::verdict::UnresolvedIssue;
use crate::mission_event::MissionEventKind;

/// Aggregate recovery activity across an entire mission. Built up
/// by the mission_loop driver as it observes
/// [`MissionEventKind::RecoveryDecided`] events; consumed by the
/// risk-band scorer (busy history bumps risk) and by the
/// unresolved-issues collector (one entry per class).
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, Type)]
pub struct RecoveryHistorySummary {
    /// Map of FailureClass wire name → bucket. Ordered map for
    /// deterministic iteration.
    pub buckets: BTreeMap<String, RecoveryBucket>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, Type)]
pub struct RecoveryBucket {
    pub action_taken: String,
    pub occurrences: u32,
}

impl RecoveryHistorySummary {
    pub fn new() -> Self {
        Self::default()
    }

    /// Record one recovery decision. Idempotent on the bucket key
    /// (same class merges; the latest `action_taken` wins, which
    /// is the inbox-relevant signal — "what did we ultimately do?").
    pub fn add_class(&mut self, class: &str, action: &str, occurrences: u32) {
        let entry = self.buckets.entry(class.to_string()).or_default();
        entry.action_taken = action.to_string();
        entry.occurrences = entry.occurrences.saturating_add(occurrences);
    }

    /// Total occurrence count across every class.
    pub fn total_occurrences(&self) -> u32 {
        self.buckets
            .values()
            .fold(0u32, |a, b| a.saturating_add(b.occurrences))
    }

    /// True iff the history is empty (no recovery activity).
    pub fn is_empty(&self) -> bool {
        self.buckets.is_empty()
    }
}

/// Per-scrubbed-subtask record. The mission_loop driver builds a
/// `Vec<ScrubRecord>` as it observes per-task `Scrub` arbiter
/// decisions and feeds it to the collector. Scrub records are not
/// derived from the raw event stream because the `task_index` is
/// not on the `ArbiterDecided` event payload — the driver knows
/// it and surfaces it via this struct.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ScrubRecord {
    pub task_index: u32,
    pub reason: String,
}

/// Build the unresolved-issues list from the mission's telemetry.
///
///   * One `OpenEscalation` per `ArbiterDecided { bound: Some(_), .. }`
///     event.
///   * One `RecoveryAttempted` per bucket in `history`.
///   * One `ContextBudgetTruncated` per
///     `MissionEventKind::ContextBudgetTruncated` event.
///   * One `SubtaskScrubbed` per `ScrubRecord` in `scrubs`.
///
/// Order is deterministic: escalations (event order) → recovery
/// (sorted by class name) → truncations (event order) → scrubs
/// (input order). This stable ordering lets the e2e test assert
/// the exact verdict shape.
pub fn collect_unresolved(
    events: &[MissionEventKind],
    history: &RecoveryHistorySummary,
    scrubs: &[ScrubRecord],
) -> Vec<UnresolvedIssue> {
    let mut issues: Vec<UnresolvedIssue> = Vec::new();

    // 1. Open escalations from ArbiterDecided events.
    for ev in events {
        if let MissionEventKind::ArbiterDecided {
            bound: Some(b),
            decision_json,
            ..
        } = ev
        {
            let summary = extract_escalation_summary(decision_json)
                .unwrap_or_else(|| format!("{b:?} bound tripped"));
            issues.push(UnresolvedIssue::OpenEscalation { bound: *b, summary });
        }
    }

    // 2. Recovery buckets from the prepared summary.
    for (class, bucket) in &history.buckets {
        issues.push(UnresolvedIssue::RecoveryAttempted {
            class: class.clone(),
            action_taken: bucket.action_taken.clone(),
            occurrences: bucket.occurrences,
        });
    }

    // 3. Context-budget truncations.
    for ev in events {
        if let MissionEventKind::ContextBudgetTruncated {
            worker_id,
            dropped_note_ids,
            ..
        } = ev
        {
            issues.push(UnresolvedIssue::ContextBudgetTruncated {
                dropped_count: dropped_note_ids.len() as u32,
                worker_id: worker_id.clone(),
            });
        }
    }

    // 4. Subtask scrubs (driver-supplied records).
    for record in scrubs {
        issues.push(UnresolvedIssue::SubtaskScrubbed {
            task_index: record.task_index,
            reason: record.reason.clone(),
        });
    }

    issues
}

/// Pull the `evidence.summary` string out of a serialized
/// `ArbiterDecision::Escalate` JSON blob. Best-effort — if the
/// shape doesn't match, return `None` and the caller falls back
/// to the bound name.
fn extract_escalation_summary(decision_json: &str) -> Option<String> {
    let value: serde_json::Value = serde_json::from_str(decision_json).ok()?;
    let summary = value.get("evidence")?.get("summary")?.as_str()?.to_string();
    if summary.is_empty() {
        None
    } else {
        Some(summary)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::arbiter::decision::{ArbiterDecision, SuggestedUserAction};
    use crate::arbiter::{AuthorityBound, EscalationEvidence};

    fn open_escalation_event(bound: AuthorityBound) -> MissionEventKind {
        let decision = ArbiterDecision::Escalate {
            bound,
            evidence: EscalationEvidence {
                summary: format!("triggered {bound:?}"),
                payload_json: None,
            },
            suggested_user_action: SuggestedUserAction::ResolveMission,
        };
        MissionEventKind::ArbiterDecided {
            worker_id: "mock-1".into(),
            decision_json: serde_json::to_string(&decision).unwrap(),
            audit_overall: 0.5,
            bound: Some(bound),
        }
    }

    fn budget_truncated(worker: &str, dropped: u32) -> MissionEventKind {
        MissionEventKind::ContextBudgetTruncated {
            worker_id: worker.into(),
            original_bytes: 12_345,
            rendered_bytes: 8_000,
            dropped_note_ids: vec!["note-a".into(); dropped as usize],
        }
    }

    #[test]
    fn open_escalation_becomes_one_issue() {
        let events = vec![open_escalation_event(AuthorityBound::Scope)];
        let issues = collect_unresolved(&events, &RecoveryHistorySummary::default(), &[]);
        assert_eq!(issues.len(), 1);
        assert!(matches!(
            issues[0],
            UnresolvedIssue::OpenEscalation {
                bound: AuthorityBound::Scope,
                ..
            }
        ));
    }

    #[test]
    fn recovery_summary_becomes_one_issue_per_class() {
        let mut history = RecoveryHistorySummary::default();
        history.add_class("missing_file", "retry", 2);
        history.add_class("command_error", "escalate", 1);
        let issues = collect_unresolved(&[], &history, &[]);
        assert_eq!(issues.len(), 2);
        // BTreeMap iterates in key order: command_error before missing_file.
        if let UnresolvedIssue::RecoveryAttempted { class, .. } = &issues[0] {
            assert_eq!(class, "command_error");
        } else {
            panic!("unexpected variant");
        }
        if let UnresolvedIssue::RecoveryAttempted {
            class, occurrences, ..
        } = &issues[1]
        {
            assert_eq!(class, "missing_file");
            assert_eq!(*occurrences, 2);
        } else {
            panic!("unexpected variant");
        }
    }

    #[test]
    fn context_budget_truncated_becomes_one_issue_per_event() {
        let events = vec![
            budget_truncated("worker-a", 3),
            budget_truncated("worker-b", 1),
        ];
        let issues = collect_unresolved(&events, &RecoveryHistorySummary::default(), &[]);
        assert_eq!(issues.len(), 2);
        for issue in &issues {
            assert!(matches!(
                issue,
                UnresolvedIssue::ContextBudgetTruncated { .. }
            ));
        }
    }

    #[test]
    fn subtask_scrubbed_becomes_one_issue_per_record() {
        let scrubs = vec![ScrubRecord {
            task_index: 2,
            reason: "quality_exhausted".into(),
        }];
        let issues = collect_unresolved(&[], &RecoveryHistorySummary::default(), &scrubs);
        assert_eq!(issues.len(), 1);
        assert!(matches!(
            issues[0],
            UnresolvedIssue::SubtaskScrubbed { task_index: 2, .. }
        ));
    }

    #[test]
    fn empty_inputs_produce_no_issues() {
        let issues = collect_unresolved(&[], &RecoveryHistorySummary::default(), &[]);
        assert!(issues.is_empty());
    }

    #[test]
    fn mixed_inputs_produce_combined_issues() {
        let events = vec![
            open_escalation_event(AuthorityBound::Quality),
            budget_truncated("mock-1", 2),
        ];
        let mut history = RecoveryHistorySummary::default();
        history.add_class("missing_file", "retry", 1);
        let issues = collect_unresolved(
            &events,
            &history,
            &[ScrubRecord {
                task_index: 0,
                reason: "supervisor_marked_unachievable".into(),
            }],
        );
        assert_eq!(issues.len(), 4);
    }

    #[test]
    fn recovery_history_is_empty_helper() {
        assert!(RecoveryHistorySummary::default().is_empty());
        let mut h = RecoveryHistorySummary::default();
        h.add_class("missing_file", "retry", 1);
        assert!(!h.is_empty());
        assert_eq!(h.total_occurrences(), 1);
    }

    #[test]
    fn recovery_history_total_saturates() {
        let mut h = RecoveryHistorySummary::default();
        h.add_class("a", "retry", u32::MAX);
        h.add_class("b", "retry", 5);
        assert_eq!(h.total_occurrences(), u32::MAX);
    }
}
