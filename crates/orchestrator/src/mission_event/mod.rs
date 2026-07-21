//! Mission-level events emitted by [`crate::mission_runtime`].
//!
//! Sits one layer above the canonical worker event schema (the
//! `event-schema` crate): mission, supervisor, worker (as a mission
//! task), and branch concerns.
//!
//! The complete mission stream is retained in-memory for bounded replay via
//! `tokio::sync::broadcast`. Audit summaries are also persisted independently
//! for cross-mission History; UI forwarding is not a persistence boundary.

use crate::mission::MissionSpec;
use crate::vendor_profile::DeclaredSideEffectKind;
use serde::{Deserialize, Serialize};
use specta::Type;

pub mod plan_envelope;

pub use plan_envelope::{BoundFit, BoundFitKind, EnvelopeFit, TechChoice};

/// One unit of work in the supervisor's decomposition.
///
/// Dependency edges, role, acceptance criteria, scope, and description all
/// default for compatibility with earlier `{ index, title }` event rows.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Type)]
pub struct TaskDescriptor {
    pub index: u32,
    pub title: String,
    /// Index references to other tasks in the same decomposition
    /// that must complete before this one starts. Must form a DAG
    /// (no cycles, no orphan refs); validated by
    /// [`crate::task_graph::validate`] at decomposition emit time.
    #[serde(default)]
    pub depends_on: Vec<u32>,
    /// What kind of work this is. Influences vendor routing via
    /// [`crate::task_graph::select_vendor_for_role`].
    #[serde(default)]
    pub role: crate::task_graph::TaskRole,
    /// Pass/fail conditions evaluated post-audit. Empty criteria
    /// reduce to "any arbiter Accept is sufficient".
    #[serde(default)]
    pub criteria: crate::task_graph::AcceptanceCriteria,
    /// Per-task allow-list. Intersected with
    /// `MissionSpec.scope_paths` (per-task wins inside the
    /// intersection). Empty inherits the mission-level scope.
    /// Enforced before audit and integration by the per-worker ACL gate.
    #[serde(default)]
    pub scope_paths: Vec<std::path::PathBuf>,
    /// Free-form supervisor-authored elaboration of `title`. Memory retrieval
    /// includes its keywords in the worker's `RetrievalBrief` query.
    #[serde(default)]
    pub description: Option<String>,
}

impl Default for TaskDescriptor {
    fn default() -> Self {
        Self {
            index: 0,
            title: String::new(),
            depends_on: Vec::new(),
            role: crate::task_graph::TaskRole::Implementer,
            criteria: crate::task_graph::AcceptanceCriteria::default(),
            scope_paths: Vec::new(),
            description: None,
        }
    }
}

/// How the user disposed of a completed mission.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Type)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum MergeResolution {
    Merged,
    Discarded,
    /// Retained for decoding historical/replayed events. Current runtimes do
    /// not emit this resolution until a real supervisor re-entry path exists.
    Extended {
        directive: Option<String>,
    },
}

/// Envelope shared by every mission event. `seq` is monotonic per
/// mission; `ts` is RFC 3339 UTC with millisecond precision.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Type)]
pub struct MissionEvent {
    pub mission_id: String,
    pub seq: u64,
    pub ts: String,
    #[serde(flatten)]
    pub kind: MissionEventKind,
}

/// Discriminated payload union. Variant names use the dotted wire
/// form from proposal v2 §3.6 (e.g. `"mission.created"`).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Type)]
#[serde(tag = "type", content = "payload")]
pub enum MissionEventKind {
    #[serde(rename = "mission.created")]
    Created { spec: MissionSpec },

    #[serde(rename = "mission.execution_started")]
    ExecutionStarted,

    #[serde(rename = "supervisor.decomposition")]
    Decomposition { tasks: Vec<TaskDescriptor> },

    /// The supervisor produced a decomposition that does not form a DAG
    /// (cycle / orphan dependency / duplicate index / empty).
    /// `reason` is the serialized [`crate::task_graph::GraphError`]
    /// for forward-compat — the orchestrator deserializes for the
    /// inbox card; the frontend treats it opaquely.
    #[serde(rename = "supervisor.decomposition_rejected")]
    DecompositionRejected { reason: String },

    /// Emitted after each (re)decomposition when plan confirmation is required.
    /// when `MissionSpec.confirm_plan == Some(true)`. `generation` is
    /// monotonically increasing within a mission; 0 is the first
    /// proposed plan, incremented on each regeneration.
    ///
    /// The optional rich-context fields default so older event rows and
    /// adapters retain the same confirmation behavior.
    #[serde(rename = "plan.proposed")]
    PlanProposed {
        tasks: Vec<TaskDescriptor>,
        generation: u32,
        /// Short prose summary of the proposed plan. Rendered
        /// above the task list in `MissionPlanPreview`. `None` for
        /// supervisor adapters that haven't been updated to emit it.
        #[serde(default)]
        overview: Option<String>,
        /// Typed tech-stack rows. Rendered as a small badge
        /// list; `is_new` rows get a `[new]` glyph.
        #[serde(default)]
        tech_stack: Option<Vec<TechChoice>>,
        /// Supervisor's four-bound self-assessment of the
        /// plan. When any bound is `Exceeds`, the orchestrator
        /// forces `PendingPlanApproval` even if
        /// `MissionSpec.confirm_plan != Some(true)`. `None` means
        /// the supervisor did not classify (legacy or
        /// envelope-unaware adapter).
        #[serde(default)]
        envelope_fit: Option<EnvelopeFit>,
    },

    /// The user accepted the proposed plan; workers are about to
    /// spawn. The `generation` matches the accepted `PlanProposed`.
    #[serde(rename = "plan.confirmed")]
    PlanConfirmed { generation: u32 },

    /// The user asked for a new decomposition. `hint` is the
    /// optional natural-language feedback the user supplied;
    /// `prior_generation` identifies which proposed plan they
    /// rejected.
    #[serde(rename = "plan.regeneration_requested")]
    PlanRegenerationRequested {
        hint: Option<String>,
        prior_generation: u32,
    },

    /// The user rejected the proposed plan from
    /// `MissionPlanPreview`. The mission transitions to `Aborted`
    /// directly after this event; the `reason` is the optional
    /// free-form text from the FE reject form. `generation` matches
    /// the rejected `PlanProposed`.
    #[serde(rename = "plan.rejected")]
    PlanRejected {
        generation: u32,
        #[serde(default)]
        reason: Option<String>,
    },

    #[serde(rename = "worker.spawned")]
    WorkerSpawned {
        worker_id: String,
        task_index: u32,
        task_title: String,
    },

    #[serde(rename = "worker.progress")]
    WorkerProgress { worker_id: String, note: String },

    #[serde(rename = "worker.result_submitted")]
    WorkerResultSubmitted {
        worker_id: String,
        files: Vec<String>,
        summary: String,
    },

    #[serde(rename = "supervisor.review_started")]
    ReviewStarted { worker_id: String },

    #[serde(rename = "supervisor.integrated")]
    Integrated {
        worker_id: String,
        integration_sha: String,
        snapshot_tag: String,
    },

    #[serde(rename = "supervisor.test_result")]
    TestResult { passed: bool, summary: String },

    #[serde(rename = "supervisor.audit_completed")]
    AuditCompleted {
        /// Tier as a string ("smoke" | "standard" | "deep") so the
        /// schema doesn't depend on the audit module's AuditTier enum
        /// (which lives in a different crate root).
        tier: String,
        overall: f64,
        /// Full serialized AuditReport. Opaque to the event-schema
        /// boundary; consumers parse on demand.
        payload_json: String,
    },

    /// The arbiter has rendered a decision over a
    /// worker's submission. Carries the decision as JSON (typed enums
    /// would force a churning event-schema dependency) plus the audit
    /// composite for the inbox card. `bound` is set only for the
    /// Escalate variant; `None` for Accept/Extend/Scrub.
    #[serde(rename = "arbiter.decided")]
    ArbiterDecided {
        worker_id: String,
        decision_json: String,
        audit_overall: f64,
        bound: Option<crate::arbiter::AuthorityBound>,
    },

    /// Every in-flight task has quiesced after a scrub or escalation, so the
    /// user may now safely choose Merge or Discard for the partial mission.
    #[serde(rename = "mission.attention_ready")]
    AttentionReady,

    /// A mission previously merged into its target branch has been undone by
    /// the `revert_mission` Tauri command. `restored_sha` is the new revert
    /// commit. The `pre_merge_tag` field name is retained for wire
    /// compatibility; for final merges it carries the persistent `before`
    /// rollback anchor.
    #[serde(rename = "mission.reverted")]
    MissionReverted {
        restored_sha: String,
        pre_merge_tag: String,
    },

    /// Audit re-run after worker integration on the supervisor
    /// branch. Detects regressions introduced by the rebase/merge
    /// itself (e.g., a test that passed in the worker's worktree but
    /// fails post-merge because of an unrelated change in
    /// supervisor/main since the rebase base). When `overall` falls
    /// below `policy.quality_min`, the mission enters Attention with a
    /// Reversibility escalation and preserves the workspace for review.
    #[serde(rename = "supervisor.post_integration_audit_completed")]
    PostIntegrationAuditCompleted {
        worker_id: String,
        tier: String,
        overall: f64,
        payload_json: String,
    },

    #[serde(rename = "mission.completed")]
    Completed { summary: String, files_changed: u32 },

    #[serde(rename = "mission.merge_resolved")]
    MergeResolved { resolution: MergeResolution },

    #[serde(rename = "mission.aborted")]
    Aborted { reason: String },

    /// A worker surfaced an operation that can
    /// affect the machine or outside world beyond the repo diff. This
    /// is intentionally visible and auditable rather than magically
    /// reversible: Discard cleans Vigla branches/worktrees, but
    /// package installs, paid API calls, network egress, and external
    /// mutations remain real side effects.
    #[serde(rename = "boundary.side_effect_logged")]
    SideEffectLogged {
        worker_id: String,
        kind: DeclaredSideEffectKind,
        summary: String,
        declared: bool,
    },

    /// Permanent single-supervisor boundary. Emitted when the supervisor's intent
    /// stream requests spawning a worker whose role would itself be
    /// "supervisor" — the orchestrator refuses the spawn and surfaces
    /// the refusal on the mission-event channel so it lands in
    /// Attention. The team metaphor stops at one level of supervision.
    #[serde(rename = "boundary.sub_supervisor_refused")]
    SubSupervisorRefused {
        requested_by_supervisor_id: String,
        requested_worker_id: String,
    },

    /// Retained for forward-compatible replay of the earlier extension event
    /// shape. Current runtimes reject Extend rather than emit an event without
    /// scheduling a real supervisor turn.
    #[serde(rename = "mission.extended")]
    MissionExtended { directive: Option<String> },

    /// The recovery engine decided on a failure path. Carries the
    /// classified FailureClass and the chosen RecoveryAction both
    /// as JSON strings — the recovery types live in the orchestrator
    /// crate and the event schema is forward-compat-only, so we
    /// keep typed enums out of the wire format. The escalation layer owns
    /// visibility routing.
    #[serde(rename = "supervisor.recovery_decided")]
    RecoveryDecided {
        worker_id: String,
        class_json: String,
        action_json: String,
    },

    /// The mission entered `MissionState::Paused`. `reason_json`
    /// carries the typed PauseReason; `estimated_resume_at_ms` is
    /// the Unix-ms timestamp the wake-up task will trigger on.
    #[serde(rename = "mission.paused")]
    MissionPaused {
        reason_json: String,
        estimated_resume_at_ms: u64,
    },

    /// The mission transitioned from Paused back to Executing.
    /// `vendor` identifies which vendor's quota window reopened.
    #[serde(rename = "mission.resumed")]
    MissionResumed { vendor: event_schema::Vendor },

    /// Memory attach exceeded the rendered bundle's token budget.
    /// bundle because the worker's token budget was below the
    /// composer's natural output. Telemetry only — not surfaced
    /// to the user.
    #[serde(rename = "supervisor.context_budget_exceeded")]
    ContextBudgetExceeded {
        worker_id: String,
        requested_tokens: u32,
        granted_tokens: u32,
        dropped_entries: u32,
    },

    /// A worker left a structured note for a downstream task. Routed by the
    /// DAG scheduler and
    /// optionally persisted by the memory kernel. `to_role` is
    /// the worker role the upstream worker intends as the
    /// audience.
    #[serde(rename = "supervisor.handoff_note")]
    HandoffNote {
        from_worker: String,
        to_role: crate::mission_runtime::WorkerRole,
        note: String,
    },

    /// A worker emitted a `RequestContext`
    /// signal and the supervisor's memory matcher could not
    /// satisfy it. Telemetry only — the follow-up
    /// `ArbiterDecided { bound: Some(Scope), … }` event carries
    /// the inbox card.
    #[serde(rename = "supervisor.context_request_unmet")]
    ContextRequestUnmet {
        worker_id: String,
        /// Stringified [`crate::recovery::ContextRequestKind`]
        /// (e.g. "file_content", "documentation", "prior_decision").
        kind: String,
        detail: String,
    },

    /// The mission runtime assembled the mission-level completion verdict.
    /// Carries the serialized
    /// [`crate::judgment::CompletionVerdict`] as JSON. Visibility
    /// routes this through [`crate::escalation::visibility_for`]:
    /// happy verdicts (`recommendation = Accept`) land as
    /// `Inbox{Completion, Info}`; fail-closed `Scrub` recommendations
    /// land as `Inbox{Escalation, Warning}`.
    #[serde(rename = "mission.completion_verdict_rendered")]
    CompletionVerdictRendered { payload_json: String },

    /// The memory composer truncated a
    /// worker's bundle to fit the token budget. This event
    /// fires AFTER successful re-render — it signals truncation
    /// success. The pre-existing
    /// [`Self::ContextBudgetExceeded`] event still fires first
    /// (telemetry "we noticed the overflow"); this event signals
    /// "we recovered." Both events live for replay and
    /// debugging. Unresolved-issues collector consumes this
    /// variant.
    #[serde(rename = "supervisor.context_budget_truncated")]
    ContextBudgetTruncated {
        worker_id: String,
        original_bytes: u32,
        rendered_bytes: u32,
        dropped_note_ids: Vec<String>,
    },

    /// A memory bundle was composed
    /// for a worker. Fires from both compose paths:
    ///   * `Manual` — explicit pin set passed in (the legacy
    ///     [`crate::memory::attach_to_worktree`] path); `mmr_lambda`
    ///     is always `None`.
    ///   * `Retrieval` — driven by [`crate::memory::composer::RetrievalBrief`]
    ///     through [`crate::memory::MemoryKernel::compose_retrieval`].
    ///     `mmr_lambda` is `Some(λ)` **iff MMR re-rank actually ran**
    ///     (i.e. at least one candidate had a stored embedding —
    ///     hybrid relevance + MMR diversity); `None` if the path
    ///     fell back to BM25-only (no stored vectors yet during the
    ///     embedding-backfill warm-up window, or the `embeddings`
    ///     cargo feature is off). Replay can therefore tell hybrid
    ///     retrieval, BM25-only fallback, and manual composer
    ///     apart from this field alone.
    ///
    /// Telemetry only — never user-visible. `candidate_count` is
    /// the number of notes the composer was handed by its picker
    /// **before any composer-side budget truncation** — post-MMR
    /// for the retrieval path; equal to the supplied id-list
    /// length for the manual path. (NB: not the same as the final
    /// rendered page-table length, which may be shorter once the
    /// worker's token budget is enforced.)
    #[serde(rename = "memory.context_bundle_composed")]
    ContextBundleComposed {
        bundle_id: String,
        source: ComposeSource,
        candidate_count: u32,
        mmr_lambda: Option<f64>,
    },

    /// Telemetry: curated skill playbooks attached to a worker before dispatch.
    /// Telemetry-only (no escalation/inbox routing), like `ContextBundleComposed`.
    #[serde(rename = "skills.attached")]
    SkillsAttached {
        worker_id: String,
        skill_ids: Vec<String>,
        tokens: u32,
        dropped: Vec<String>,
    },
}

/// Which compose path produced a memory bundle. Carried on
/// [`MissionEventKind::ContextBundleComposed`] so replay can tell
/// `compose_manual` from `compose_retrieval` without re-deriving
/// it from the surrounding event stream.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Type)]
#[serde(rename_all = "lowercase")]
pub enum ComposeSource {
    Manual,
    Retrieval,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn task_descriptor_round_trips_with_new_fields() {
        let t = TaskDescriptor {
            index: 0,
            title: "implement logout".into(),
            depends_on: vec![],
            role: crate::task_graph::TaskRole::Implementer,
            criteria: crate::task_graph::AcceptanceCriteria {
                min_audit_overall: Some(0.8),
                require_tests_pass: Some(true),
                forbid_new_security_flags: Some(true),
                summary: Some("ship cleanly".into()),
            },
            scope_paths: vec![std::path::PathBuf::from("src/auth")],
            description: Some(
                "invalidate refresh tokens server-side and \
                clear the session cookie on the client"
                    .into(),
            ),
        };
        let json = serde_json::to_string(&t).unwrap();
        let back: TaskDescriptor = serde_json::from_str(&json).unwrap();
        assert_eq!(t, back);
        assert!(json.contains("\"description\":\"invalidate refresh tokens"));
    }

    #[test]
    fn task_descriptor_deserializes_legacy_two_field_shape() {
        // Legacy event logs persisted before description was added must
        // still round-trip cleanly with description defaulting to None.
        // This is load-bearing for the event-replay invariant.
        let legacy_json = r#"{"index":0,"title":"old task"}"#;
        let t: TaskDescriptor = serde_json::from_str(legacy_json).unwrap();
        assert_eq!(t.index, 0);
        assert_eq!(t.title, "old task");
        assert!(t.depends_on.is_empty());
        assert_eq!(t.role, crate::task_graph::TaskRole::Implementer);
        assert_eq!(t.criteria, crate::task_graph::AcceptanceCriteria::default());
        assert!(t.scope_paths.is_empty());
        assert_eq!(t.description, None);
    }

    #[test]
    fn task_descriptor_pre_v13_legacy_shape_with_all_pre_existing_fields() {
        // Wider legacy fixture: serialized TaskDescriptor from a real
        // pre-V1.3 event log (every field that existed before
        // description landed). description must default to None and
        // the rest must rehydrate identically.
        let legacy_json = r#"{
            "index": 2,
            "title": "wire vendor adapter",
            "depends_on": [0, 1],
            "role": "implementer",
            "criteria": {
                "min_audit_overall": 0.7,
                "require_tests_pass": true
            },
            "scope_paths": ["adapters/claude/src", "orchestrator/src"]
        }"#;
        let t: TaskDescriptor = serde_json::from_str(legacy_json).unwrap();
        assert_eq!(t.index, 2);
        assert_eq!(t.title, "wire vendor adapter");
        assert_eq!(t.depends_on, vec![0, 1]);
        assert_eq!(t.scope_paths.len(), 2);
        assert_eq!(t.description, None);
    }

    #[test]
    fn decompose_intent_preserves_supervisor_descriptions_through_conversion() {
        // The supervisor adapter parses `{title, description}` from the
        // raw model JSON; the orchestrator must preserve `description`
        // when building the per-task `TaskDescriptor`s that downstream
        // retrieval consumes. We mimic the conversion shape used in
        // mission_loop.rs::Decompose (and in worker_pass.rs::Split) so
        // a future refactor that drops the field is caught here.
        use supervisor_adapter::SupervisorTaskDescriptor;
        let raw = [
            SupervisorTaskDescriptor {
                title: "extract auth middleware".into(),
                description: Some(
                    "pull JWT validation into a tower::Layer so it \
                     can be reused across the REST and gRPC servers"
                        .into(),
                ),
                ..Default::default()
            },
            SupervisorTaskDescriptor {
                title: "stand up integration tests".into(),
                description: None,
                ..Default::default()
            },
        ];
        let converted: Vec<TaskDescriptor> = raw
            .iter()
            .enumerate()
            .map(|(i, t)| TaskDescriptor {
                index: i as u32,
                title: t.title.clone(),
                description: t.description.clone(),
                depends_on: t.depends_on.clone(),
                scope_paths: t.scope_paths.clone(),
                ..Default::default()
            })
            .collect();
        assert_eq!(converted.len(), 2);
        assert_eq!(
            converted[0].description.as_deref(),
            Some(
                "pull JWT validation into a tower::Layer so it \
                 can be reused across the REST and gRPC servers"
            ),
            "supervisor description must survive the orchestrator boundary"
        );
        assert_eq!(converted[1].description, None);
    }

    #[test]
    fn created_serializes_with_dotted_tag() {
        let spec = MissionSpec {
            title: "T".into(),
            objective: "O".into(),
            target_ref: "main".into(),
            tests: None,
            supervisor_model: None,
            worker_model: None,
            worker_count: None,
            confirm_plan: None,
            scope_paths: vec![],
        };
        let ev = MissionEvent {
            mission_id: "demo-0000".into(),
            seq: 0,
            ts: "2026-05-12T00:00:00.000Z".into(),
            kind: MissionEventKind::Created { spec: spec.clone() },
        };
        let json = serde_json::to_string(&ev).unwrap();
        assert!(json.contains("\"type\":\"mission.created\""));
        assert!(json.contains("\"payload\""));
        let round: MissionEvent = serde_json::from_str(&json).unwrap();
        assert_eq!(round, ev);
    }

    #[test]
    fn worker_event_serializes_with_dotted_tag() {
        let ev = MissionEvent {
            mission_id: "demo-0000".into(),
            seq: 1,
            ts: "2026-05-12T00:00:00.000Z".into(),
            kind: MissionEventKind::WorkerSpawned {
                worker_id: "mock-1".into(),
                task_index: 0,
                task_title: "step".into(),
            },
        };
        let json = serde_json::to_string(&ev).unwrap();
        assert!(json.contains("\"type\":\"worker.spawned\""));
    }

    #[test]
    fn merge_resolution_serializes_snake_case() {
        let r = MergeResolution::Extended {
            directive: Some("more".into()),
        };
        let json = serde_json::to_string(&r).unwrap();
        assert!(json.contains("\"type\":\"extended\""));
    }

    #[test]
    fn mission_extended_round_trips_with_some_directive() {
        let ev = MissionEvent {
            mission_id: "demo-2000".into(),
            seq: 12,
            ts: "2026-05-13T00:00:00.000Z".into(),
            kind: MissionEventKind::MissionExtended {
                directive: Some("widen retry policy".into()),
            },
        };
        let json = serde_json::to_string(&ev).unwrap();
        assert!(json.contains("\"type\":\"mission.extended\""));
        assert!(json.contains("widen retry policy"));
        let round: MissionEvent = serde_json::from_str(&json).unwrap();
        assert_eq!(round, ev);
    }

    #[test]
    fn mission_extended_round_trips_with_none_directive() {
        let ev = MissionEvent {
            mission_id: "demo-2001".into(),
            seq: 13,
            ts: "2026-05-13T00:00:00.000Z".into(),
            kind: MissionEventKind::MissionExtended { directive: None },
        };
        let json = serde_json::to_string(&ev).unwrap();
        assert!(json.contains("\"type\":\"mission.extended\""));
        let round: MissionEvent = serde_json::from_str(&json).unwrap();
        assert_eq!(round, ev);
    }

    #[test]
    fn plan_proposed_round_trips_with_dotted_tag() {
        let ev = MissionEvent {
            mission_id: "demo-0001".into(),
            seq: 4,
            ts: "2026-05-13T00:00:00.000Z".into(),
            kind: MissionEventKind::PlanProposed {
                tasks: vec![TaskDescriptor {
                    index: 0,
                    title: "Plan integration".into(),
                    ..Default::default()
                }],
                generation: 0,
                overview: None,
                tech_stack: None,
                envelope_fit: None,
            },
        };
        let json = serde_json::to_string(&ev).unwrap();
        assert!(json.contains("\"type\":\"plan.proposed\""));
        assert!(json.contains("\"generation\":0"));
        let round: MissionEvent = serde_json::from_str(&json).unwrap();
        assert_eq!(round, ev);
    }

    #[test]
    fn plan_confirmed_round_trips() {
        let ev = MissionEvent {
            mission_id: "demo-0002".into(),
            seq: 5,
            ts: "2026-05-13T00:00:00.000Z".into(),
            kind: MissionEventKind::PlanConfirmed { generation: 1 },
        };
        let json = serde_json::to_string(&ev).unwrap();
        assert!(json.contains("\"type\":\"plan.confirmed\""));
        assert!(json.contains("\"generation\":1"));
        let round: MissionEvent = serde_json::from_str(&json).unwrap();
        assert_eq!(round, ev);
    }

    #[test]
    fn plan_regeneration_requested_round_trips_with_and_without_hint() {
        // With hint.
        let ev_with = MissionEvent {
            mission_id: "demo-0003".into(),
            seq: 6,
            ts: "2026-05-13T00:00:00.000Z".into(),
            kind: MissionEventKind::PlanRegenerationRequested {
                hint: Some("Make the tasks smaller".into()),
                prior_generation: 0,
            },
        };
        let json = serde_json::to_string(&ev_with).unwrap();
        assert!(json.contains("\"type\":\"plan.regeneration_requested\""));
        assert!(json.contains("\"hint\":\"Make the tasks smaller\""));
        assert_eq!(
            serde_json::from_str::<MissionEvent>(&json).unwrap(),
            ev_with
        );

        // Without hint.
        let ev_no = MissionEvent {
            mission_id: "demo-0004".into(),
            seq: 7,
            ts: "2026-05-13T00:00:00.000Z".into(),
            kind: MissionEventKind::PlanRegenerationRequested {
                hint: None,
                prior_generation: 2,
            },
        };
        let json = serde_json::to_string(&ev_no).unwrap();
        assert!(json.contains("\"hint\":null"));
        assert_eq!(serde_json::from_str::<MissionEvent>(&json).unwrap(), ev_no);
    }

    #[test]
    fn plan_proposed_round_trips_with_envelope_fields() {
        use super::{BoundFit, BoundFitKind, EnvelopeFit, TechChoice};
        let ev = MissionEventKind::PlanProposed {
            tasks: vec![],
            generation: 0,
            overview: Some("Add OAuth callback handler.".into()),
            tech_stack: Some(vec![TechChoice {
                layer: "auth_provider".into(),
                choice: "Auth0".into(),
                rationale: "matches existing setup".into(),
                is_new: false,
            }]),
            envelope_fit: Some(EnvelopeFit {
                scope: BoundFit {
                    fit: BoundFitKind::Within,
                    note: "".into(),
                },
                reversibility: BoundFit {
                    fit: BoundFitKind::NearLimit,
                    note: "migration".into(),
                },
                risk: BoundFit {
                    fit: BoundFitKind::Within,
                    note: "".into(),
                },
                quality: BoundFit {
                    fit: BoundFitKind::Within,
                    note: "".into(),
                },
            }),
        };
        let json = serde_json::to_string(&ev).unwrap();
        assert!(json.contains("\"type\":\"plan.proposed\""));
        let back: MissionEventKind = serde_json::from_str(&json).unwrap();
        assert_eq!(ev, back);
    }

    #[test]
    fn plan_proposed_legacy_two_field_payload_deserializes_with_none() {
        // The existing event log holds PlanProposed rows from before
        // QC-3. They must deserialize cleanly with the three new
        // fields all None.
        let legacy = r#"{"type":"plan.proposed","payload":{"tasks":[],"generation":0}}"#;
        let ev: MissionEventKind = serde_json::from_str(legacy).unwrap();
        match ev {
            MissionEventKind::PlanProposed {
                generation,
                overview,
                tech_stack,
                envelope_fit,
                ..
            } => {
                assert_eq!(generation, 0);
                assert!(overview.is_none());
                assert!(tech_stack.is_none());
                assert!(envelope_fit.is_none());
            }
            other => panic!("expected PlanProposed, got {other:?}"),
        }
    }

    #[test]
    fn plan_rejected_round_trips_with_reason() {
        let ev = MissionEventKind::PlanRejected {
            generation: 2,
            reason: Some("scope too broad".into()),
        };
        let json = serde_json::to_string(&ev).unwrap();
        assert!(json.contains("\"type\":\"plan.rejected\""));
        let back: MissionEventKind = serde_json::from_str(&json).unwrap();
        assert_eq!(ev, back);
    }

    #[test]
    fn plan_rejected_round_trips_with_none_reason() {
        let ev = MissionEventKind::PlanRejected {
            generation: 0,
            reason: None,
        };
        let json = serde_json::to_string(&ev).unwrap();
        let back: MissionEventKind = serde_json::from_str(&json).unwrap();
        assert_eq!(ev, back);
    }

    #[test]
    fn side_effect_logged_round_trips_with_dotted_tag() {
        let ev = MissionEvent {
            mission_id: "demo-side-effect".into(),
            seq: 11,
            ts: "2026-05-14T00:00:00.000Z".into(),
            kind: MissionEventKind::SideEffectLogged {
                worker_id: "mock-1".into(),
                kind: crate::vendor_profile::DeclaredSideEffectKind::PackageInstall,
                summary: "package install observed: pip install".into(),
                declared: true,
            },
        };
        let json = serde_json::to_string(&ev).unwrap();
        assert!(json.contains("\"type\":\"boundary.side_effect_logged\""));
        assert!(json.contains("\"kind\":\"package_install\""));
        assert!(json.contains("\"declared\":true"));
        let round: MissionEvent = serde_json::from_str(&json).unwrap();
        assert_eq!(round, ev);
    }

    #[test]
    fn sub_supervisor_refused_round_trips_with_dotted_tag() {
        let ev = MissionEvent {
            mission_id: "demo-subsup".into(),
            seq: 12,
            ts: "2026-05-13T00:00:00.000Z".into(),
            kind: MissionEventKind::SubSupervisorRefused {
                requested_by_supervisor_id: "sup-1".into(),
                requested_worker_id: "would-be-sub-1".into(),
            },
        };
        let json = serde_json::to_string(&ev).unwrap();
        assert!(json.contains("\"type\":\"boundary.sub_supervisor_refused\""));
        assert!(json.contains("\"requested_by_supervisor_id\":\"sup-1\""));
        let round: MissionEvent = serde_json::from_str(&json).unwrap();
        assert_eq!(round, ev);
    }

    #[test]
    fn mission_reverted_round_trips() {
        let e = MissionEventKind::MissionReverted {
            restored_sha: "abc123".into(),
            pre_merge_tag: "vigla/pre-merge/mid-1/0".into(),
        };
        let json = serde_json::to_string(&e).unwrap();
        assert!(json.contains(r#""type":"mission.reverted""#));
        let back: MissionEventKind = serde_json::from_str(&json).unwrap();
        assert_eq!(e, back);
    }

    #[test]
    fn post_integration_audit_round_trips() {
        let e = MissionEventKind::PostIntegrationAuditCompleted {
            worker_id: "mock-1".into(),
            tier: "Standard".into(),
            overall: 0.91,
            payload_json: "{}".into(),
        };
        let json = serde_json::to_string(&e).unwrap();
        assert!(json.contains(r#""type":"supervisor.post_integration_audit_completed""#));
        let back: MissionEventKind = serde_json::from_str(&json).unwrap();
        assert_eq!(e, back);
    }

    #[test]
    fn recovery_decided_round_trip() {
        let ev = MissionEventKind::RecoveryDecided {
            worker_id: "w1".into(),
            class_json: r#"{"kind":"missing_file","path":"src/lib.rs"}"#.into(),
            action_json: r#"{"kind":"request_supervisor"}"#.into(),
        };
        let s = serde_json::to_string(&ev).unwrap();
        let back: MissionEventKind = serde_json::from_str(&s).unwrap();
        match back {
            MissionEventKind::RecoveryDecided { worker_id, .. } => {
                assert_eq!(worker_id, "w1");
            }
            other => panic!("expected RecoveryDecided, got {other:?}"),
        }
    }

    #[test]
    fn mission_paused_round_trip() {
        let ev = MissionEventKind::MissionPaused {
            reason_json: r#"{"kind":"waiting_for_quota","vendor":"claude"}"#.into(),
            estimated_resume_at_ms: 1_716_018_000_000,
        };
        let s = serde_json::to_string(&ev).unwrap();
        let back: MissionEventKind = serde_json::from_str(&s).unwrap();
        match back {
            MissionEventKind::MissionPaused {
                estimated_resume_at_ms,
                ..
            } => assert_eq!(estimated_resume_at_ms, 1_716_018_000_000),
            other => panic!("expected MissionPaused, got {other:?}"),
        }
    }

    #[test]
    fn mission_resumed_round_trip() {
        let ev = MissionEventKind::MissionResumed {
            vendor: event_schema::Vendor::Claude,
        };
        let s = serde_json::to_string(&ev).unwrap();
        let back: MissionEventKind = serde_json::from_str(&s).unwrap();
        match back {
            MissionEventKind::MissionResumed { vendor } => {
                assert_eq!(vendor, event_schema::Vendor::Claude);
            }
            other => panic!("expected MissionResumed, got {other:?}"),
        }
    }

    #[test]
    fn decomposition_rejected_round_trips() {
        let ev = MissionEventKind::DecompositionRejected {
            reason: r#"{"kind":"cycle","involved":[0,1]}"#.into(),
        };
        let json = serde_json::to_string(&ev).unwrap();
        assert!(json.contains("\"type\":\"supervisor.decomposition_rejected\""));
        let back: MissionEventKind = serde_json::from_str(&json).unwrap();
        assert_eq!(ev, back);
    }

    #[test]
    fn context_budget_exceeded_round_trips() {
        let ev = MissionEvent {
            mission_id: "mid-1".into(),
            seq: 7,
            ts: "2026-05-19T00:00:00.000Z".into(),
            kind: MissionEventKind::ContextBudgetExceeded {
                worker_id: "mock-1".into(),
                requested_tokens: 12000,
                granted_tokens: 8000,
                dropped_entries: 3,
            },
        };
        let json = serde_json::to_string(&ev).unwrap();
        assert!(json.contains("\"type\":\"supervisor.context_budget_exceeded\""));
        let back: MissionEvent = serde_json::from_str(&json).unwrap();
        assert_eq!(ev, back);
    }

    #[test]
    fn handoff_note_round_trips() {
        let ev = MissionEvent {
            mission_id: "mid-1".into(),
            seq: 8,
            ts: "2026-05-19T00:00:00.000Z".into(),
            kind: MissionEventKind::HandoffNote {
                from_worker: "mock-1".into(),
                to_role: crate::mission_runtime::WorkerRole::Employee,
                note: "Memoized parser results in /tmp/parser-cache.json".into(),
            },
        };
        let json = serde_json::to_string(&ev).unwrap();
        assert!(json.contains("\"type\":\"supervisor.handoff_note\""));
        let back: MissionEvent = serde_json::from_str(&json).unwrap();
        assert_eq!(ev, back);
    }

    #[test]
    fn context_request_unmet_round_trips() {
        let ev = MissionEvent {
            mission_id: "mid-1".into(),
            seq: 9,
            ts: "2026-05-19T00:00:00.000Z".into(),
            kind: MissionEventKind::ContextRequestUnmet {
                worker_id: "mock-1".into(),
                kind: "documentation".into(),
                detail: "rust async trait conventions".into(),
            },
        };
        let json = serde_json::to_string(&ev).unwrap();
        assert!(json.contains("\"type\":\"supervisor.context_request_unmet\""));
        let back: MissionEvent = serde_json::from_str(&json).unwrap();
        assert_eq!(ev, back);
    }

    #[test]
    fn completion_verdict_rendered_serializes_round_trip() {
        let ev = MissionEvent {
            mission_id: "demo-0000".into(),
            seq: 99,
            ts: "2026-05-30T00:00:00.000Z".into(),
            kind: MissionEventKind::CompletionVerdictRendered {
                payload_json: r#"{"all_subtasks_accepted":true}"#.into(),
            },
        };
        let json = serde_json::to_string(&ev).unwrap();
        assert!(json.contains("\"type\":\"mission.completion_verdict_rendered\""));
        let back: MissionEvent = serde_json::from_str(&json).unwrap();
        assert_eq!(ev, back);
    }

    #[test]
    fn context_budget_truncated_serializes_round_trip() {
        let ev = MissionEvent {
            mission_id: "demo-0000".into(),
            seq: 17,
            ts: "2026-05-30T00:00:00.000Z".into(),
            kind: MissionEventKind::ContextBudgetTruncated {
                worker_id: "mock-1".into(),
                original_bytes: 12_345,
                rendered_bytes: 8_000,
                dropped_note_ids: vec!["note-a".into(), "note-b".into()],
            },
        };
        let json = serde_json::to_string(&ev).unwrap();
        assert!(json.contains("\"type\":\"supervisor.context_budget_truncated\""));
        let back: MissionEvent = serde_json::from_str(&json).unwrap();
        assert_eq!(ev, back);
    }

    #[test]
    fn skills_attached_serializes_with_tag() {
        let ev = MissionEventKind::SkillsAttached {
            worker_id: "w1".into(),
            skill_ids: vec!["systematic-debugging".into()],
            tokens: 123,
            dropped: vec![],
        };
        let json = serde_json::to_value(&ev).unwrap();
        assert_eq!(json["type"], "skills.attached");
        assert_eq!(json["payload"]["worker_id"], "w1");
        assert_eq!(json["payload"]["skill_ids"][0], "systematic-debugging");
        let back: MissionEventKind = serde_json::from_value(json).unwrap();
        assert_eq!(ev, back);
    }
}

#[cfg(test)]
mod audit_event_tests {
    use super::MissionEventKind;
    use serde_json::json;

    #[test]
    fn audit_completed_round_trips() {
        let ev = MissionEventKind::AuditCompleted {
            tier: "smoke".into(),
            overall: 0.85,
            payload_json: json!({"test_pass": null}).to_string(),
        };
        let s = serde_json::to_string(&ev).unwrap();
        let back: MissionEventKind = serde_json::from_str(&s).unwrap();
        match back {
            MissionEventKind::AuditCompleted {
                tier,
                overall,
                payload_json: _,
            } => {
                assert_eq!(tier, "smoke");
                assert!((overall - 0.85).abs() < 1e-6);
            }
            other => panic!("expected AuditCompleted, got {other:?}"),
        }
    }

    #[test]
    fn arbiter_decided_serializes_with_dotted_tag() {
        let e = MissionEventKind::ArbiterDecided {
            worker_id: "mock-1".into(),
            decision_json: "{\"kind\":\"accept\"}".into(),
            audit_overall: 0.85,
            bound: None,
        };
        let json = serde_json::to_string(&e).unwrap();
        assert!(json.contains(r#""type":"arbiter.decided""#));
        let back: MissionEventKind = serde_json::from_str(&json).unwrap();
        assert_eq!(e, back);
    }
}
