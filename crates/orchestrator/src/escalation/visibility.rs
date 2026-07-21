//! Visibility verdict types. See [`visibility_for`] for the
//! mapping function (added in Task 3).

use serde::{Deserialize, Serialize};
use specta::Type;

/// Verdict for a single mission event. The frontend ingest layer
/// consults this verdict via a Tauri command (Task 11) and routes
/// the event accordingly.
///
/// `Internal` events ARE persisted to the event log (for debugging
/// and replay) but never surfaced to the UI in any mode.
/// `PowerUserOnly` events build the legacy attention/comms feed
/// state only when the user enables "Show all events".
/// `Inbox` events always produce an inbox card with the carried
/// `kind` + `severity`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Type)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum EventVisibility {
    Internal,
    PowerUserOnly,
    Inbox {
        // Renamed in JSON to avoid colliding with the internal
        // discriminant tag (also `kind`). The Rust-side name stays
        // `kind` so the plan's variant signature reads naturally.
        #[serde(rename = "inbox_kind")]
        kind: InboxKind,
        severity: Severity,
    },
}

/// What kind of inbox card the event produces.
///
/// - `Escalation` — the arbiter escalated; user input is required
///   before the mission can proceed.
/// - `Completion` — the mission reached a terminal Accept or
///   Scrub. The card stays in the inbox as the user's record of
///   what shipped.
/// - `SideEffect` — a declared side effect (package install, API
///   call, etc.) requires user awareness even though no decision
///   is needed. Always `Severity::Warning`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Type)]
#[serde(rename_all = "snake_case")]
pub enum InboxKind {
    Escalation,
    Completion,
    SideEffect,
}

/// Severity of an inbox card. Drives visual treatment + the
/// "should we fire the macOS native banner" decision (only
/// `ActionRequired` triggers the banner).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Type)]
#[serde(rename_all = "snake_case")]
pub enum Severity {
    /// Informational. Renders as a soft card; no notification.
    Info,
    /// Something to be aware of. Renders with a warning glyph; no
    /// native banner.
    Warning,
    /// User input required. Always fires the macOS native banner
    /// when the app is not focused.
    ActionRequired,
}

use crate::mission_event::MissionEventKind;

/// Route a `MissionEventKind` to its visibility verdict.
///
/// O(1) exhaustive match — adding a new `MissionEventKind` variant
/// will fail to compile until classified. This is the
/// maintainability invariant of the module.
pub fn visibility_for(event: &MissionEventKind) -> EventVisibility {
    match event {
        // ── Internal: pure plumbing ────────────────────────────────
        MissionEventKind::Created { .. } => EventVisibility::Internal,
        MissionEventKind::ExecutionStarted => EventVisibility::Internal,
        MissionEventKind::AuditCompleted { .. } => EventVisibility::Internal,
        // S4 telemetry: only escalates indirectly through a follow-up
        // Escalate event emitted by mission_loop when overall falls
        // below policy.quality_min.
        MissionEventKind::PostIntegrationAuditCompleted { .. } => EventVisibility::Internal,
        // S5 telemetry: the recovery engine's classification + chosen
        // action. When recovery escalates to the user, that goes via a
        // separate ArbiterDecided emit (which IS surfaced). This event
        // exists purely for replay/debugging.
        MissionEventKind::RecoveryDecided { .. } => EventVisibility::Internal,
        // S5: auto-resume signal — no user action required. The
        // earlier MissionPaused card stays in the inbox as the visible
        // record; resumption is silent plumbing.
        MissionEventKind::MissionResumed { .. } => EventVisibility::Internal,

        // S8: context-budget truncation is internal telemetry.
        MissionEventKind::ContextBudgetExceeded { .. } => EventVisibility::Internal,
        // S8: a worker → downstream handoff is operator-visible
        // (power user only) — the DAG scheduler reads it and the
        // memory kernel persists a copy.
        MissionEventKind::HandoffNote { .. } => EventVisibility::PowerUserOnly,
        // S8: an unmet context request is internal telemetry; the
        // follow-up ArbiterDecided with bound=Scope carries the
        // user-visible escalation.
        MissionEventKind::ContextRequestUnmet { .. } => EventVisibility::Internal,

        // S9 (S8 carryover): the re-render succeeded — this is
        // purely internal telemetry. The pre-existing
        // ContextBudgetExceeded event ALSO fires and IS internal;
        // both events together let replay verify the truncation
        // pipeline ran end-to-end.
        MissionEventKind::ContextBudgetTruncated { .. } => EventVisibility::Internal,

        // V1.3: a memory bundle was composed (Manual or Retrieval
        // path). Pure replay/telemetry — never surfaced to the
        // user. Distinguished from ContextBudgetTruncated because
        // it fires on every compose, not just budget overflows.
        MissionEventKind::ContextBundleComposed { .. } => EventVisibility::Internal,

        // Telemetry-only: skill playbooks attached to a worker before
        // dispatch. Never user-visible, same as ContextBundleComposed.
        MissionEventKind::SkillsAttached { .. } => EventVisibility::Internal,

        // S9: the mission-level completion verdict. Visibility
        // depends on the recommendation field inside the
        // serialized payload — Accept → Completion/Info; Extend
        // or Scrub → Escalation with appropriate severity.
        MissionEventKind::CompletionVerdictRendered { payload_json } => {
            verdict_visibility(payload_json)
        }

        // ── PowerUserOnly: live debug / observation ────────────────
        MissionEventKind::Decomposition { .. } => EventVisibility::PowerUserOnly,
        MissionEventKind::DecompositionRejected { .. } => EventVisibility::Inbox {
            kind: InboxKind::Escalation,
            severity: Severity::ActionRequired,
        },
        MissionEventKind::PlanConfirmed { .. } => EventVisibility::PowerUserOnly,
        MissionEventKind::PlanRegenerationRequested { .. } => EventVisibility::PowerUserOnly,
        // QC-3: the user already drove the reject action; no inbox card
        // needed — the next visible event is `Aborted`, which carries
        // an escalation card with the reject reason embedded.
        MissionEventKind::PlanRejected { .. } => EventVisibility::Internal,
        MissionEventKind::WorkerSpawned { .. } => EventVisibility::PowerUserOnly,
        MissionEventKind::WorkerProgress { .. } => EventVisibility::PowerUserOnly,
        MissionEventKind::WorkerResultSubmitted { .. } => EventVisibility::PowerUserOnly,
        MissionEventKind::ReviewStarted { .. } => EventVisibility::PowerUserOnly,
        MissionEventKind::Integrated { .. } => EventVisibility::PowerUserOnly,
        MissionEventKind::TestResult { .. } => EventVisibility::PowerUserOnly,
        MissionEventKind::MissionExtended { .. } => EventVisibility::PowerUserOnly,
        // The actionable card is carried by the preceding ArbiterDecided;
        // this event is a synchronization boundary for lifecycle controls.
        MissionEventKind::AttentionReady => EventVisibility::Internal,

        // ── Inbox: terminal / required attention ───────────────────
        MissionEventKind::PlanProposed { .. } => EventVisibility::Inbox {
            kind: InboxKind::Escalation,
            severity: Severity::ActionRequired,
        },
        MissionEventKind::Completed { .. } => EventVisibility::Inbox {
            kind: InboxKind::Completion,
            severity: Severity::Info,
        },
        MissionEventKind::MergeResolved { .. } => EventVisibility::Inbox {
            kind: InboxKind::Completion,
            severity: Severity::Info,
        },
        // S4 Auto-Integration & Rollback: revert is a terminal mission
        // state — the user (or mission_loop on regression) chose to
        // unwind the integration. Surfaced as a Completion card so the
        // inbox reflects the final disposition.
        MissionEventKind::MissionReverted { .. } => EventVisibility::Inbox {
            kind: InboxKind::Completion,
            severity: Severity::Info,
        },
        MissionEventKind::Aborted { .. } => EventVisibility::Inbox {
            kind: InboxKind::Escalation,
            severity: Severity::Warning,
        },
        MissionEventKind::SideEffectLogged { .. } => EventVisibility::Inbox {
            kind: InboxKind::SideEffect,
            severity: Severity::Warning,
        },
        MissionEventKind::SubSupervisorRefused { .. } => EventVisibility::Inbox {
            kind: InboxKind::Escalation,
            severity: Severity::Warning,
        },
        // S5: informational — mission is sleeping until the vendor's
        // quota window closes. The user CAN choose to wait, switch
        // vendors, or abort, but no input is required: the wake-up
        // task will auto-resume.
        MissionEventKind::MissionPaused { .. } => EventVisibility::Inbox {
            kind: InboxKind::Completion,
            severity: Severity::Info,
        },

        // ── ArbiterDecided: discriminate by `bound` + JSON ─────────
        MissionEventKind::ArbiterDecided {
            bound,
            decision_json,
            ..
        } => arbiter_visibility(bound.as_ref(), decision_json),
    }
}

/// Sub-mapping for the `ArbiterDecided` variant. `bound` is
/// `Some` for `Escalate` and `None` for `Accept` / `Extend` /
/// `Scrub`. The JSON payload is inspected only on the `None`
/// branch (cheap — short string `contains` checks).
fn arbiter_visibility(
    bound: Option<&crate::arbiter::AuthorityBound>,
    decision_json: &str,
) -> EventVisibility {
    // Escalate — always an Inbox Escalation requiring action.
    if bound.is_some() {
        return EventVisibility::Inbox {
            kind: InboxKind::Escalation,
            severity: Severity::ActionRequired,
        };
    }

    // bound == None: discriminate by the decision kind in the
    // JSON payload. Order matches the four variants of
    // `ArbiterDecision`. `Extend` is internal (the arbiter will
    // re-emit a fresh decision after rework); `Scrub` is a
    // Warning escalation (mission failed but no user input can
    // recover it); `Accept` is a Completion.
    // S6: a nested ReworkKind::MarkUnachievable inside Extend is
    // semantically terminal (supervisor declares no progress
    // possible). Surface as Warning so the user sees the
    // declaration; the user can decide how to proceed manually.
    // The sniff key is the wire form of the rework kind tag.
    if decision_json.contains("\"kind\":\"extend\"")
        && decision_json.contains("\"kind\":\"mark_unachievable\"")
    {
        return EventVisibility::Inbox {
            kind: InboxKind::Escalation,
            severity: Severity::Warning,
        };
    }

    if decision_json.contains("\"kind\":\"extend\"") {
        return EventVisibility::Internal;
    }
    if decision_json.contains("\"kind\":\"scrub\"") {
        return EventVisibility::Inbox {
            kind: InboxKind::Escalation,
            severity: Severity::Warning,
        };
    }
    // Default branch for `bound == None`: treat as Accept. This
    // includes the literal "accept" kind. We default to
    // Completion rather than Internal so a future malformed
    // `ArbiterDecided` event still surfaces.
    EventVisibility::Inbox {
        kind: InboxKind::Completion,
        severity: Severity::Info,
    }
}

/// Sub-mapping for the `CompletionVerdictRendered` variant. The
/// outer event always lands as an `Inbox` card; the question is
/// what `kind` + `severity`. The dispatch inspects the
/// serialized `recommendation.kind` (`accept` | `extend` |
/// `scrub` | `escalate`) inside the payload. We use simple
/// string `contains` checks to avoid pulling in a full JSON
/// parse on the hot path.
///
/// Mapping:
///
///   * `accept` → `Inbox{Completion, Info}`
///   * `extend` → `Inbox{Escalation, ActionRequired}` (user must
///     co-sign additional rework or scrub the mission)
///   * `scrub`  → `Inbox{Escalation, Warning}`
///   * `escalate` (rare — would only land here if the assemble
///     step pushed an Escalate; today's heuristic never does) →
///     `Inbox{Escalation, ActionRequired}`
fn verdict_visibility(payload_json: &str) -> EventVisibility {
    // Sniff the recommendation field. Order is intentional: the
    // string `"kind":"accept"` may appear in the AcceptPayload's
    // nested audit — but only the OUTER `recommendation.kind`
    // matters. To be robust we look for the
    // `"recommendation":{"kind":"X"` sub-pattern.
    if payload_json.contains(r#""recommendation":{"kind":"extend""#) {
        return EventVisibility::Inbox {
            kind: InboxKind::Escalation,
            severity: Severity::ActionRequired,
        };
    }
    if payload_json.contains(r#""recommendation":{"kind":"scrub""#) {
        return EventVisibility::Inbox {
            kind: InboxKind::Escalation,
            severity: Severity::Warning,
        };
    }
    if payload_json.contains(r#""recommendation":{"kind":"escalate""#) {
        return EventVisibility::Inbox {
            kind: InboxKind::Escalation,
            severity: Severity::ActionRequired,
        };
    }
    // Default: Accept (happy completion).
    EventVisibility::Inbox {
        kind: InboxKind::Completion,
        severity: Severity::Info,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn internal_round_trips() {
        let v = EventVisibility::Internal;
        let json = serde_json::to_string(&v).unwrap();
        let back: EventVisibility = serde_json::from_str(&json).unwrap();
        assert_eq!(v, back);
    }

    #[test]
    fn power_user_only_round_trips() {
        let v = EventVisibility::PowerUserOnly;
        let json = serde_json::to_string(&v).unwrap();
        let back: EventVisibility = serde_json::from_str(&json).unwrap();
        assert_eq!(v, back);
    }

    #[test]
    fn inbox_escalation_action_required_round_trips() {
        let v = EventVisibility::Inbox {
            kind: InboxKind::Escalation,
            severity: Severity::ActionRequired,
        };
        let json = serde_json::to_string(&v).unwrap();
        let back: EventVisibility = serde_json::from_str(&json).unwrap();
        assert_eq!(v, back);
    }

    #[test]
    fn inbox_completion_info_round_trips() {
        let v = EventVisibility::Inbox {
            kind: InboxKind::Completion,
            severity: Severity::Info,
        };
        let json = serde_json::to_string(&v).unwrap();
        let back: EventVisibility = serde_json::from_str(&json).unwrap();
        assert_eq!(v, back);
    }

    #[test]
    fn inbox_side_effect_warning_round_trips() {
        let v = EventVisibility::Inbox {
            kind: InboxKind::SideEffect,
            severity: Severity::Warning,
        };
        let json = serde_json::to_string(&v).unwrap();
        let back: EventVisibility = serde_json::from_str(&json).unwrap();
        assert_eq!(v, back);
    }

    #[test]
    fn discriminant_uses_snake_case_tag() {
        let v = EventVisibility::Inbox {
            kind: InboxKind::Escalation,
            severity: Severity::ActionRequired,
        };
        let json = serde_json::to_string(&v).unwrap();
        assert!(json.contains(r#""kind":"inbox""#));
        assert!(json.contains(r#""severity":"action_required""#));
    }

    fn extend_event(rework_kind_json: &str) -> MissionEventKind {
        MissionEventKind::ArbiterDecided {
            worker_id: "mock-1".into(),
            decision_json: format!(
                r#"{{"kind":"extend","rework_kind":{rework_kind_json},"attempts_remaining":1}}"#
            ),
            audit_overall: 0.5,
            bound: None,
        }
    }

    #[test]
    fn extend_with_mark_unachievable_is_warning_inbox() {
        let event =
            extend_event(r#"{"kind":"mark_unachievable","rationale":"manual review needed"}"#);
        assert_eq!(
            visibility_for(&event),
            EventVisibility::Inbox {
                kind: InboxKind::Escalation,
                severity: Severity::Warning,
            }
        );
    }

    #[test]
    fn extend_with_other_rework_kinds_stays_internal() {
        for rk in [
            r#"{"kind":"revise","directive":"fix"}"#,
            r#"{"kind":"reassign","from_worker":"mock-1","to_vendor":"codex"}"#,
            r#"{"kind":"split","sub_tasks":[]}"#,
            r#"{"kind":"narrow","reduced_scope":[]}"#,
            r#"{"kind":"rebrief","new_brief":"x"}"#,
        ] {
            let event = extend_event(rk);
            assert_eq!(visibility_for(&event), EventVisibility::Internal, "{rk}");
        }
    }
}

#[cfg(test)]
mod mapping_tests {
    use super::*;
    use crate::arbiter::AuthorityBound;
    use crate::mission::MissionSpec;
    use crate::mission_event::{MergeResolution, MissionEventKind, TaskDescriptor};
    use crate::vendor_profile::DeclaredSideEffectKind;

    fn spec() -> MissionSpec {
        MissionSpec {
            title: "t".into(),
            objective: "o".into(),
            target_ref: "main".into(),
            tests: None,
            supervisor_model: None,
            worker_model: None,
            worker_count: None,
            confirm_plan: None,
            scope_paths: vec![],
        }
    }

    // ── Internal — pure plumbing, never user-visible ──────────────

    #[test]
    fn created_is_internal() {
        assert_eq!(
            visibility_for(&MissionEventKind::Created { spec: spec() }),
            EventVisibility::Internal
        );
    }

    #[test]
    fn execution_started_is_internal() {
        assert_eq!(
            visibility_for(&MissionEventKind::ExecutionStarted),
            EventVisibility::Internal
        );
    }

    #[test]
    fn audit_completed_is_internal() {
        // Audit results are intermediate signal; the arbiter
        // decision is what reaches the inbox.
        assert_eq!(
            visibility_for(&MissionEventKind::AuditCompleted {
                tier: "smoke".into(),
                overall: 0.85,
                payload_json: "{}".into(),
            }),
            EventVisibility::Internal
        );
    }

    #[test]
    fn arbiter_extend_is_internal() {
        // Extend = the arbiter is reworking the mission silently.
        // No user-visible event; only the terminal decision lands.
        let event = MissionEventKind::ArbiterDecided {
            worker_id: "w-1".into(),
            decision_json: r#"{"kind":"extend"}"#.into(),
            audit_overall: 0.5,
            bound: None,
        };
        assert_eq!(visibility_for(&event), EventVisibility::Internal);
    }

    // ── PowerUserOnly — useful for debugging / live observation ──

    #[test]
    fn decomposition_is_power_user_only() {
        assert_eq!(
            visibility_for(&MissionEventKind::Decomposition {
                tasks: vec![TaskDescriptor {
                    index: 0,
                    title: "T".into(),
                    ..Default::default()
                }],
            }),
            EventVisibility::PowerUserOnly
        );
    }

    #[test]
    fn decomposition_rejected_is_inbox_escalation_action_required() {
        let v = visibility_for(&MissionEventKind::DecompositionRejected {
            reason: r#"{"kind":"empty_decomposition"}"#.into(),
        });
        assert_eq!(
            v,
            EventVisibility::Inbox {
                kind: InboxKind::Escalation,
                severity: Severity::ActionRequired,
            }
        );
    }

    #[test]
    fn context_budget_exceeded_is_internal() {
        assert_eq!(
            visibility_for(&MissionEventKind::ContextBudgetExceeded {
                worker_id: "mock-1".into(),
                requested_tokens: 12000,
                granted_tokens: 8000,
                dropped_entries: 3,
            }),
            EventVisibility::Internal,
        );
    }

    #[test]
    fn context_budget_truncated_is_internal() {
        assert_eq!(
            visibility_for(&MissionEventKind::ContextBudgetTruncated {
                worker_id: "mock-1".into(),
                original_bytes: 12_000,
                rendered_bytes: 8_000,
                dropped_note_ids: vec!["note-a".into(), "note-b".into()],
            }),
            EventVisibility::Internal,
        );
    }

    #[test]
    fn completion_verdict_rendered_routes_by_recommendation_kind() {
        let cases: &[(&str, &str, EventVisibility)] = &[
            (
                "accept",
                r#"{"recommendation":{"kind":"accept","audit":{},"summary":"ok"}}"#,
                EventVisibility::Inbox {
                    kind: InboxKind::Completion,
                    severity: Severity::Info,
                },
            ),
            (
                "extend",
                r#"{"recommendation":{"kind":"extend","attempts_remaining":1}}"#,
                EventVisibility::Inbox {
                    kind: InboxKind::Escalation,
                    severity: Severity::ActionRequired,
                },
            ),
            (
                "scrub",
                r#"{"recommendation":{"kind":"scrub","reason":{"kind":"quality_exhausted"}}}"#,
                EventVisibility::Inbox {
                    kind: InboxKind::Escalation,
                    severity: Severity::Warning,
                },
            ),
        ];
        for (label, payload_json, expected) in cases {
            let v = visibility_for(&MissionEventKind::CompletionVerdictRendered {
                payload_json: payload_json.to_string(),
            });
            assert_eq!(v, *expected, "case {label}");
        }
    }

    #[test]
    fn handoff_note_is_power_user_only() {
        assert_eq!(
            visibility_for(&MissionEventKind::HandoffNote {
                from_worker: "mock-1".into(),
                to_role: crate::mission_runtime::WorkerRole::Employee,
                note: "n".into(),
            }),
            EventVisibility::PowerUserOnly,
        );
    }

    #[test]
    fn context_request_unmet_is_internal() {
        assert_eq!(
            visibility_for(&MissionEventKind::ContextRequestUnmet {
                worker_id: "mock-1".into(),
                kind: "file_content".into(),
                detail: "src/missing.rs".into(),
            }),
            EventVisibility::Internal,
        );
    }

    #[test]
    fn worker_progress_is_power_user_only() {
        assert_eq!(
            visibility_for(&MissionEventKind::WorkerProgress {
                worker_id: "w-1".into(),
                note: "looking at lib.rs".into(),
            }),
            EventVisibility::PowerUserOnly
        );
    }

    #[test]
    fn worker_spawned_is_power_user_only() {
        assert_eq!(
            visibility_for(&MissionEventKind::WorkerSpawned {
                worker_id: "w-1".into(),
                task_index: 0,
                task_title: "step".into(),
            }),
            EventVisibility::PowerUserOnly
        );
    }

    #[test]
    fn worker_result_submitted_is_power_user_only() {
        assert_eq!(
            visibility_for(&MissionEventKind::WorkerResultSubmitted {
                worker_id: "w-1".into(),
                files: vec![],
                summary: "ok".into(),
            }),
            EventVisibility::PowerUserOnly
        );
    }

    #[test]
    fn review_started_is_power_user_only() {
        assert_eq!(
            visibility_for(&MissionEventKind::ReviewStarted {
                worker_id: "w-1".into(),
            }),
            EventVisibility::PowerUserOnly
        );
    }

    #[test]
    fn integrated_is_power_user_only() {
        assert_eq!(
            visibility_for(&MissionEventKind::Integrated {
                worker_id: "w-1".into(),
                integration_sha: "deadbeef".into(),
                snapshot_tag: "snap".into(),
            }),
            EventVisibility::PowerUserOnly
        );
    }

    #[test]
    fn test_result_is_power_user_only() {
        assert_eq!(
            visibility_for(&MissionEventKind::TestResult {
                passed: true,
                summary: "all green".into(),
            }),
            EventVisibility::PowerUserOnly
        );
    }

    // ── Inbox — terminal decisions / required attention ───────────

    #[test]
    fn plan_proposed_is_inbox_action_required() {
        // User must confirm or regenerate the plan; this is a
        // blocking interaction.
        let v = visibility_for(&MissionEventKind::PlanProposed {
            tasks: vec![],
            generation: 0,
            overview: None,
            tech_stack: None,
            envelope_fit: None,
        });
        assert_eq!(
            v,
            EventVisibility::Inbox {
                kind: InboxKind::Escalation,
                severity: Severity::ActionRequired,
            }
        );
    }

    #[test]
    fn plan_confirmed_is_power_user_only() {
        // The user's own confirmation is mirrored as power-user-
        // only context; the user already knows they clicked it.
        assert_eq!(
            visibility_for(&MissionEventKind::PlanConfirmed { generation: 0 }),
            EventVisibility::PowerUserOnly
        );
    }

    #[test]
    fn plan_regeneration_requested_is_power_user_only() {
        assert_eq!(
            visibility_for(&MissionEventKind::PlanRegenerationRequested {
                hint: None,
                prior_generation: 0,
            }),
            EventVisibility::PowerUserOnly
        );
    }

    #[test]
    fn plan_rejected_is_internal() {
        // The user already drove the reject action via
        // MissionPlanPreview; no inbox card is needed here. The
        // next visible event is Aborted, which surfaces an
        // escalation inbox card with the reject reason embedded.
        let v = visibility_for(&MissionEventKind::PlanRejected {
            generation: 0,
            reason: Some("scope too broad".into()),
        });
        assert_eq!(v, EventVisibility::Internal);
    }

    #[test]
    fn plan_rejected_without_reason_is_internal() {
        let v = visibility_for(&MissionEventKind::PlanRejected {
            generation: 0,
            reason: None,
        });
        assert_eq!(v, EventVisibility::Internal);
    }

    #[test]
    fn mission_completed_is_inbox_completion_info() {
        let v = visibility_for(&MissionEventKind::Completed {
            summary: "3 tasks integrated".into(),
            files_changed: 3,
        });
        assert_eq!(
            v,
            EventVisibility::Inbox {
                kind: InboxKind::Completion,
                severity: Severity::Info,
            }
        );
    }

    #[test]
    fn mission_aborted_is_inbox_escalation_warning() {
        let v = visibility_for(&MissionEventKind::Aborted {
            reason: "user cancelled".into(),
        });
        assert_eq!(
            v,
            EventVisibility::Inbox {
                kind: InboxKind::Escalation,
                severity: Severity::Warning,
            }
        );
    }

    #[test]
    fn merge_resolved_is_inbox_completion_info() {
        let v = visibility_for(&MissionEventKind::MergeResolved {
            resolution: MergeResolution::Merged,
        });
        assert_eq!(
            v,
            EventVisibility::Inbox {
                kind: InboxKind::Completion,
                severity: Severity::Info,
            }
        );
    }

    #[test]
    fn mission_extended_is_power_user_only() {
        // The user explicitly extended; the supervisor strip
        // surfaces the directive. Inbox doesn't need its own card.
        assert_eq!(
            visibility_for(&MissionEventKind::MissionExtended {
                directive: Some("more".into()),
            }),
            EventVisibility::PowerUserOnly
        );
    }

    #[test]
    fn side_effect_logged_is_inbox_side_effect_warning() {
        let v = visibility_for(&MissionEventKind::SideEffectLogged {
            worker_id: "w-1".into(),
            kind: DeclaredSideEffectKind::PackageInstall,
            summary: "pip install x".into(),
            declared: true,
        });
        assert_eq!(
            v,
            EventVisibility::Inbox {
                kind: InboxKind::SideEffect,
                severity: Severity::Warning,
            }
        );
    }

    #[test]
    fn sub_supervisor_refused_is_inbox_escalation_warning() {
        let v = visibility_for(&MissionEventKind::SubSupervisorRefused {
            requested_by_supervisor_id: "sup-1".into(),
            requested_worker_id: "would-be-sub".into(),
        });
        assert_eq!(
            v,
            EventVisibility::Inbox {
                kind: InboxKind::Escalation,
                severity: Severity::Warning,
            }
        );
    }

    #[test]
    fn arbiter_accept_is_inbox_completion_info() {
        // The terminal Accept decision is a per-worker completion
        // card. The aggregate Completed event (above) is the
        // mission-level card.
        let event = MissionEventKind::ArbiterDecided {
            worker_id: "w-1".into(),
            decision_json: r#"{"kind":"accept"}"#.into(),
            audit_overall: 0.85,
            bound: None,
        };
        // accept = bound None + decision_json contains "accept".
        // For now we treat any (bound==None && !extend) ArbiterDecided
        // as Completion; the JSON inspection lives in the mapping
        // function. See visibility_for for the exact discrimination.
        match visibility_for(&event) {
            EventVisibility::Inbox {
                kind: InboxKind::Completion,
                severity: Severity::Info,
            } => {}
            other => panic!("expected Inbox{{Completion, Info}}, got {other:?}"),
        }
    }

    #[test]
    fn arbiter_scrub_is_inbox_escalation_warning() {
        let event = MissionEventKind::ArbiterDecided {
            worker_id: "w-1".into(),
            decision_json: r#"{"kind":"scrub","reason":"quality_exhausted"}"#.into(),
            audit_overall: 0.2,
            bound: None,
        };
        match visibility_for(&event) {
            EventVisibility::Inbox {
                kind: InboxKind::Escalation,
                severity: Severity::Warning,
            } => {}
            other => panic!("expected Inbox{{Escalation, Warning}}, got {other:?}"),
        }
    }

    #[test]
    fn arbiter_escalate_quality_is_inbox_escalation_action_required() {
        let event = MissionEventKind::ArbiterDecided {
            worker_id: "w-1".into(),
            decision_json: r#"{"kind":"escalate","bound":"quality"}"#.into(),
            audit_overall: 0.4,
            bound: Some(AuthorityBound::Quality),
        };
        match visibility_for(&event) {
            EventVisibility::Inbox {
                kind: InboxKind::Escalation,
                severity: Severity::ActionRequired,
            } => {}
            other => panic!("expected Inbox{{Escalation, ActionRequired}}, got {other:?}"),
        }
    }

    #[test]
    fn arbiter_escalate_scope_is_inbox_escalation_action_required() {
        let event = MissionEventKind::ArbiterDecided {
            worker_id: "w-1".into(),
            decision_json: r#"{"kind":"escalate","bound":"scope"}"#.into(),
            audit_overall: 0.85,
            bound: Some(AuthorityBound::Scope),
        };
        match visibility_for(&event) {
            EventVisibility::Inbox {
                kind: InboxKind::Escalation,
                severity: Severity::ActionRequired,
            } => {}
            other => panic!("expected Inbox{{Escalation, ActionRequired}}, got {other:?}"),
        }
    }

    #[test]
    fn arbiter_escalate_risk_is_inbox_escalation_action_required() {
        let event = MissionEventKind::ArbiterDecided {
            worker_id: "w-1".into(),
            decision_json: r#"{"kind":"escalate","bound":"risk"}"#.into(),
            audit_overall: 0.85,
            bound: Some(AuthorityBound::Risk),
        };
        match visibility_for(&event) {
            EventVisibility::Inbox {
                kind: InboxKind::Escalation,
                severity: Severity::ActionRequired,
            } => {}
            other => panic!("expected Inbox{{Escalation, ActionRequired}}, got {other:?}"),
        }
    }

    #[test]
    fn arbiter_escalate_reversibility_is_inbox_escalation_action_required() {
        let event = MissionEventKind::ArbiterDecided {
            worker_id: "w-1".into(),
            decision_json: r#"{"kind":"escalate","bound":"reversibility"}"#.into(),
            audit_overall: 0.85,
            bound: Some(AuthorityBound::Reversibility),
        };
        match visibility_for(&event) {
            EventVisibility::Inbox {
                kind: InboxKind::Escalation,
                severity: Severity::ActionRequired,
            } => {}
            other => panic!("expected Inbox{{Escalation, ActionRequired}}, got {other:?}"),
        }
    }
}

#[cfg(test)]
mod invariant_tests {
    use super::*;
    use crate::arbiter::AuthorityBound;
    use crate::mission_event::MissionEventKind;
    use proptest::prelude::*;

    fn any_authority_bound() -> impl Strategy<Value = AuthorityBound> {
        prop_oneof![
            Just(AuthorityBound::Scope),
            Just(AuthorityBound::Reversibility),
            Just(AuthorityBound::Risk),
            Just(AuthorityBound::Quality),
        ]
    }

    proptest! {
        #[test]
        fn arbiter_escalate_always_action_required(
            bound in any_authority_bound(),
            worker_id in "[a-z0-9-]{1,10}",
            audit_overall in 0.0f64..=1.0,
        ) {
            let event = MissionEventKind::ArbiterDecided {
                worker_id,
                decision_json: r#"{"kind":"escalate"}"#.into(),
                audit_overall,
                bound: Some(bound),
            };
            prop_assert_eq!(
                visibility_for(&event),
                EventVisibility::Inbox {
                    kind: InboxKind::Escalation,
                    severity: Severity::ActionRequired,
                }
            );
        }

        #[test]
        fn arbiter_extend_is_always_internal(
            worker_id in "[a-z0-9-]{1,10}",
            audit_overall in 0.0f64..=1.0,
        ) {
            let event = MissionEventKind::ArbiterDecided {
                worker_id,
                decision_json: r#"{"kind":"extend","attempts_remaining":1}"#.into(),
                audit_overall,
                bound: None,
            };
            prop_assert_eq!(visibility_for(&event), EventVisibility::Internal);
        }

        #[test]
        fn action_required_only_appears_on_inbox_escalation(
            // Sample every variant via discriminant ordering. proptest
            // can't easily enumerate sum types, so encode the index
            // and pick within a fixed table.
            idx in 0usize..33,
        ) {
            // Build a fixture event for every MissionEventKind
            // variant. This proves the mapping never produces
            // ActionRequired on a non-Escalation card.
            let event = fixture_event(idx);
            let v = visibility_for(&event);
            if let EventVisibility::Inbox { severity: Severity::ActionRequired, kind } = v {
                prop_assert_eq!(kind, InboxKind::Escalation);
            }
        }
    }

    /// Map an index to a representative `MissionEventKind` fixture.
    /// Used by the proptest above; keep this in lock-step with the
    /// `MissionEventKind` variant set. If a variant is added,
    /// extend this table.
    fn fixture_event(idx: usize) -> MissionEventKind {
        use crate::mission::MissionSpec;
        use crate::mission_event::{MergeResolution, TaskDescriptor};
        use crate::vendor_profile::DeclaredSideEffectKind;
        let spec = MissionSpec {
            title: "t".into(),
            objective: "o".into(),
            target_ref: "main".into(),
            tests: None,
            supervisor_model: None,
            worker_model: None,
            worker_count: None,
            confirm_plan: None,
            scope_paths: vec![],
        };
        match idx % 33 {
            0 => MissionEventKind::Created { spec },
            1 => MissionEventKind::ExecutionStarted,
            2 => MissionEventKind::Decomposition { tasks: vec![] },
            3 => MissionEventKind::PlanProposed {
                tasks: vec![TaskDescriptor {
                    index: 0,
                    title: "t".into(),
                    ..Default::default()
                }],
                generation: 0,
                overview: None,
                tech_stack: None,
                envelope_fit: None,
            },
            4 => MissionEventKind::PlanConfirmed { generation: 0 },
            5 => MissionEventKind::PlanRegenerationRequested {
                hint: None,
                prior_generation: 0,
            },
            6 => MissionEventKind::WorkerSpawned {
                worker_id: "w".into(),
                task_index: 0,
                task_title: "t".into(),
            },
            7 => MissionEventKind::WorkerProgress {
                worker_id: "w".into(),
                note: "n".into(),
            },
            8 => MissionEventKind::WorkerResultSubmitted {
                worker_id: "w".into(),
                files: vec![],
                summary: "s".into(),
            },
            9 => MissionEventKind::ReviewStarted {
                worker_id: "w".into(),
            },
            10 => MissionEventKind::Integrated {
                worker_id: "w".into(),
                integration_sha: "sha".into(),
                snapshot_tag: "snap".into(),
            },
            11 => MissionEventKind::TestResult {
                passed: true,
                summary: "ok".into(),
            },
            12 => MissionEventKind::AuditCompleted {
                tier: "smoke".into(),
                overall: 0.85,
                payload_json: "{}".into(),
            },
            13 => MissionEventKind::ArbiterDecided {
                worker_id: "w".into(),
                decision_json: r#"{"kind":"accept"}"#.into(),
                audit_overall: 0.85,
                bound: None,
            },
            14 => MissionEventKind::Completed {
                summary: "s".into(),
                files_changed: 1,
            },
            15 => MissionEventKind::MergeResolved {
                resolution: MergeResolution::Merged,
            },
            16 => MissionEventKind::Aborted { reason: "r".into() },
            17 => MissionEventKind::SideEffectLogged {
                worker_id: "w".into(),
                kind: DeclaredSideEffectKind::PackageInstall,
                summary: "s".into(),
                declared: true,
            },
            18 => MissionEventKind::SubSupervisorRefused {
                requested_by_supervisor_id: "s".into(),
                requested_worker_id: "w".into(),
            },
            19 => MissionEventKind::MissionReverted {
                restored_sha: "abc123".into(),
                pre_merge_tag: "vigla/pre-merge/mid-1/0".into(),
            },
            20 => MissionEventKind::PostIntegrationAuditCompleted {
                worker_id: "w".into(),
                tier: "Standard".into(),
                overall: 0.91,
                payload_json: "{}".into(),
            },
            21 => MissionEventKind::MissionExtended { directive: None },
            22 => MissionEventKind::RecoveryDecided {
                worker_id: "w".into(),
                class_json: r#"{"kind":"missing_file"}"#.into(),
                action_json: r#"{"kind":"retry"}"#.into(),
            },
            23 => MissionEventKind::MissionPaused {
                reason_json: r#"{"kind":"waiting_for_quota"}"#.into(),
                estimated_resume_at_ms: 1_716_018_000_000,
            },
            25 => MissionEventKind::DecompositionRejected {
                reason: r#"{"kind":"cycle","involved":[0,1]}"#.into(),
            },
            26 => MissionEventKind::ContextBudgetExceeded {
                worker_id: "w".into(),
                requested_tokens: 12000,
                granted_tokens: 8000,
                dropped_entries: 3,
            },
            27 => MissionEventKind::HandoffNote {
                from_worker: "w".into(),
                to_role: crate::mission_runtime::WorkerRole::Employee,
                note: "n".into(),
            },
            28 => MissionEventKind::ContextRequestUnmet {
                worker_id: "w".into(),
                kind: "file_content".into(),
                detail: "src/missing.rs".into(),
            },
            29 => MissionEventKind::ContextBudgetTruncated {
                worker_id: "w".into(),
                original_bytes: 12_000,
                rendered_bytes: 8_000,
                dropped_note_ids: vec!["a".into()],
            },
            30 => MissionEventKind::CompletionVerdictRendered {
                payload_json: r#"{"recommendation":{"kind":"accept"}}"#.into(),
            },
            // QC-3: PlanRejected — classified Internal in
            // visibility_for; the proptest verifies it never
            // produces an ActionRequired card here.
            31 => MissionEventKind::PlanRejected {
                generation: 0,
                reason: Some("scope too broad".into()),
            },
            32 => MissionEventKind::SkillsAttached {
                worker_id: "w1".into(),
                skill_ids: vec!["systematic-debugging".into()],
                tokens: 100,
                dropped: vec![],
            },
            _ => MissionEventKind::MissionResumed {
                vendor: event_schema::Vendor::Claude,
            },
        }
    }
}
