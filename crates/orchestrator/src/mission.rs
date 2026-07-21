//! Mission domain types and state machines.
//!
//! Pure data + state-transition logic for the Low-Friction Autonomy
//! Model. Persistence, git mechanics, and IPC live in sibling modules.
//! See `ARCHITECTURE.md` ("Mission Lifecycle") for how these types fit
//! the runtime.

use serde::{Deserialize, Serialize};
use specta::Type;
use std::path::{Component, Path, PathBuf};
use thiserror::Error;

pub type MissionId = String;
pub type WorkerId = String;

/// User-provided mission input. `title` and `objective` are required;
/// `target_ref` defaults to the repo's current HEAD at the call site;
/// `tests` is optional. `supervisor_model` picks which vendor CLI
/// runs as the supervisor (default in the UI is Claude).
/// `worker_model` and `worker_count` are `None` by default —
/// meaning worker vendors are routed by task role and the supervisor
/// decides how many tasks to spawn based on the objective. A single
/// `worker_model` value pins all workers to one vendor; a comma-
/// separated value (for example `claude,codex,gemini`) pins each
/// selected worker independently by task index.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Type)]
pub struct MissionSpec {
    pub title: String,
    pub objective: String,
    pub target_ref: String,
    pub tests: Option<String>,
    pub supervisor_model: Option<String>,
    pub worker_model: Option<String>,
    pub worker_count: Option<u32>,
    /// QC-2: when `Some(true)`, the orchestrator pauses after the
    /// supervisor's decomposition turn (state moves to
    /// `MissionState::PendingPlanApproval`) and waits for the user
    /// to either confirm or request a regeneration via the
    /// `confirm_plan` / `regenerate_plan` IPC commands. Default
    /// `None` preserves the autonomous "two touches" flow.
    pub confirm_plan: Option<bool>,
    /// Allowed-files allow-list for audit scope-adherence scoring.
    /// Empty means "no constraint" (everything is in-scope). Paths
    /// are relative to the worktree root.
    #[serde(default)]
    pub scope_paths: Vec<PathBuf>,
}

impl MissionSpec {
    pub fn validate(&self) -> Result<(), MissionError> {
        if self.title.trim().is_empty() {
            return Err(MissionError::EmptyTitle);
        }
        if self.objective.trim().is_empty() {
            return Err(MissionError::EmptyObjective);
        }
        normalize_scope_paths(&self.scope_paths)?;
        Ok(())
    }

    pub fn normalized(mut self) -> Result<Self, MissionError> {
        self.validate()?;
        self.scope_paths = normalize_scope_paths(&self.scope_paths)?;
        Ok(self)
    }
}

/// Lexically normalize an untrusted scope allow-list without consulting the
/// filesystem. Root, prefix, parent, and root-equivalent entries are rejected;
/// current-directory and duplicate separators are removed.
pub fn normalize_scope_paths(paths: &[PathBuf]) -> Result<Vec<PathBuf>, MissionError> {
    let mut normalized = Vec::with_capacity(paths.len());
    for raw in paths {
        let raw_text = raw.to_string_lossy();
        if raw_text.contains('\\')
            || raw_text.starts_with('/')
            || raw_text.as_bytes().get(1) == Some(&b':')
        {
            return Err(MissionError::InvalidScopePath { path: raw.clone() });
        }
        let mut clean = PathBuf::new();
        for component in Path::new(raw).components() {
            match component {
                Component::Normal(part) => clean.push(part),
                Component::CurDir => {}
                Component::ParentDir | Component::RootDir | Component::Prefix(_) => {
                    return Err(MissionError::InvalidScopePath { path: raw.clone() });
                }
            }
        }
        if clean.as_os_str().is_empty() {
            return Err(MissionError::InvalidScopePath { path: raw.clone() });
        }
        if !normalized.contains(&clean) {
            normalized.push(clean);
        }
    }
    Ok(normalized)
}

/// Mission lifecycle. Diagram lives in `ARCHITECTURE.md`
/// ("Mission Lifecycle").
///
/// `Copy` and `Hash` are intentionally NOT derived: the S5 `Paused`
/// variant carries a typed [`PauseReason`] payload, which keeps the
/// shape from being a bit-for-bit copy. The trade-off is that every
/// callsite that previously relied on implicit copies now has to
/// `.clone()` explicitly — the diff is small (most lookups use
/// `matches!` or `PartialEq`, both still available).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", tag = "state", content = "data")]
pub enum MissionState {
    Created,
    Executing,
    /// QC-2: supervisor has emitted a proposed decomposition and the
    /// orchestrator is waiting for the user to call `confirm_plan`
    /// or `regenerate_plan`. Only reachable when
    /// `MissionSpec.confirm_plan == Some(true)`. Non-terminal; abort
    /// is reachable as from any other live state.
    PendingPlanApproval,
    Reviewing,
    Attention,
    /// S5: vendor quota window closed. The mission is paused with
    /// a typed reason carrying the vendor; the wake-up task auto-
    /// resumes at the estimated reset time. Distinct from
    /// `Attention` because no user input is required.
    Paused {
        reason: PauseReason,
    },
    CompletePendingMerge,
    Merged,
    Discarded,
    Aborted,
}

/// Why a mission is paused. The roadmap §2 names quota as the only
/// pause kind in scope for S5; future variants (e.g. waiting on a
/// long-running test in S9) can extend this enum without churning
/// the lifecycle.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum PauseReason {
    WaitingForQuota { vendor: event_schema::Vendor },
}

impl MissionState {
    pub fn is_terminal(&self) -> bool {
        matches!(self, Self::Merged | Self::Discarded | Self::Aborted)
    }

    pub fn can_transition_to(&self, next: Self) -> bool {
        use MissionState::*;
        match (self, &next) {
            (Created, Executing) => true,
            // QC-2: after the supervisor's decomposition turn, the
            // runtime may pause for user approval. Loops back into
            // Executing on confirm, stays in PendingPlanApproval
            // across regenerate cycles.
            (Executing, PendingPlanApproval) => true,
            (PendingPlanApproval, PendingPlanApproval) => true,
            (PendingPlanApproval, Executing) => true,
            (Executing, Reviewing) => true,
            (Reviewing, Executing) => true,
            (Executing | Reviewing, CompletePendingMerge) => true,
            (Executing | Reviewing, Attention) => true,
            (Attention, Executing) => true,
            // When the arbiter parks the mission at Attention, the user can
            // merge accepted partial work or discard the mission.
            (Attention, CompletePendingMerge | Merged | Discarded) => true,
            (CompletePendingMerge, Merged | Discarded) => true,
            // S5: pause/resume around quota windows. `Paused` is
            // self-healing — the wake-up task drives the
            // Paused -> Executing edge once the vendor window
            // reopens. Distinct from `Attention`, which requires the
            // user to choose merge or discard.
            (Executing | Reviewing, Paused { .. }) => true,
            (Paused { .. }, Executing) => true,
            // Abort is always reachable from any non-terminal state.
            (s, Aborted) if !s.is_terminal() => true,
            _ => false,
        }
    }

    pub fn transition(self, next: Self) -> Result<Self, MissionError> {
        if self.can_transition_to(next.clone()) {
            Ok(next)
        } else {
            Err(MissionError::InvalidMissionTransition {
                from: self,
                to: next,
            })
        }
    }
}

/// Per-worker-attempt lifecycle within a mission.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum WorkerTaskState {
    Spawned,
    Working,
    Submitting,
    UnderReview,
    Integrated,
    Revising,
    Discarded,
    Failed,
}

impl WorkerTaskState {
    pub fn is_terminal(self) -> bool {
        matches!(self, Self::Integrated | Self::Discarded | Self::Failed)
    }

    pub fn can_transition_to(self, next: Self) -> bool {
        use WorkerTaskState::*;
        match (self, next) {
            (Spawned, Working) => true,
            (Working, Submitting) => true,
            (Submitting, UnderReview) => true,
            (UnderReview, Integrated | Revising | Discarded) => true,
            (Revising, Working) => true,
            (s, Failed) if !s.is_terminal() => true,
            _ => false,
        }
    }

    pub fn transition(self, next: Self) -> Result<Self, MissionError> {
        if self.can_transition_to(next) {
            Ok(next)
        } else {
            Err(MissionError::InvalidWorkerTransition {
                from: self,
                to: next,
            })
        }
    }
}

/// User disposition at mission complete. Only valid from
/// [`MissionState::CompletePendingMerge`]; see [`Self::allowed_from`].
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Type)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ResolveAction {
    Merge,
    Discard,
    /// Reserved wire variant retained so older persisted events and clients
    /// remain decodable. The runtime fails closed with
    /// `ExtensionUnsupported` until supervisor re-entry is implemented.
    Extend {
        directive: Option<String>,
    },
}

impl ResolveAction {
    pub fn allowed_from(&self, state: MissionState) -> bool {
        !matches!(self, Self::Extend { .. })
            && matches!(
                state,
                MissionState::CompletePendingMerge | MissionState::Attention
            )
    }
}

/// Errors raised by mission-domain operations. No serde derive — the
/// host crate string-serializes via [`std::error::Error`] for the
/// Tauri boundary, matching the existing `RepositoryError` pattern.
#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum MissionError {
    #[error("mission spec has empty title")]
    EmptyTitle,

    #[error("mission spec has empty objective")]
    EmptyObjective,

    #[error(
        "invalid scope path (must be a non-root relative path without parent traversal): {path:?}"
    )]
    InvalidScopePath { path: PathBuf },

    #[error("invalid mission transition: {from:?} -> {to:?}")]
    InvalidMissionTransition {
        from: MissionState,
        to: MissionState,
    },

    #[error("invalid worker transition: {from:?} -> {to:?}")]
    InvalidWorkerTransition {
        from: WorkerTaskState,
        to: WorkerTaskState,
    },

    #[error("resolve action {action:?} not allowed from state {state:?}")]
    InvalidResolve {
        state: MissionState,
        action: ResolveAction,
    },

    #[error("mission not found: {mission_id}")]
    NotFound { mission_id: MissionId },

    #[error("a mission is already active: {mission_id}")]
    AlreadyActive { mission_id: MissionId },
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ok_spec() -> MissionSpec {
        MissionSpec {
            title: "Add logout endpoint".into(),
            objective: "Add /api/logout, invalidate session.".into(),
            target_ref: "HEAD".into(),
            tests: None,
            supervisor_model: None,
            worker_model: None,
            worker_count: None,
            confirm_plan: None,
            scope_paths: vec![],
        }
    }

    #[test]
    fn spec_rejects_empty_title() {
        let s = MissionSpec {
            title: "  ".into(),
            ..ok_spec()
        };
        assert_eq!(s.validate(), Err(MissionError::EmptyTitle));
    }

    #[test]
    fn spec_rejects_empty_objective() {
        let s = MissionSpec {
            objective: "".into(),
            ..ok_spec()
        };
        assert_eq!(s.validate(), Err(MissionError::EmptyObjective));
    }

    #[test]
    fn spec_accepts_complete_input() {
        assert!(ok_spec().validate().is_ok());
    }

    #[test]
    fn scope_paths_are_normalized_and_deduplicated() {
        let spec = MissionSpec {
            scope_paths: vec![
                "./src//core".into(),
                "src/core".into(),
                "tests/./unit".into(),
            ],
            ..ok_spec()
        }
        .normalized()
        .unwrap();
        assert_eq!(
            spec.scope_paths,
            vec![PathBuf::from("src/core"), PathBuf::from("tests/unit")]
        );
    }

    #[test]
    fn scope_paths_reject_root_absolute_parent_and_windows_forms() {
        for path in [
            ".",
            "./",
            "/tmp",
            "../src",
            "src/../secret",
            "C:\\repo",
            "src\\x",
        ] {
            let spec = MissionSpec {
                scope_paths: vec![path.into()],
                ..ok_spec()
            };
            assert!(
                matches!(spec.validate(), Err(MissionError::InvalidScopePath { .. })),
                "accepted invalid scope {path:?}"
            );
        }
    }

    #[test]
    fn mission_forward_path() {
        use MissionState::*;
        let path = [
            Created,
            Executing,
            Reviewing,
            Executing,
            CompletePendingMerge,
            Merged,
        ];
        for w in path.windows(2) {
            // `MissionState` lost `Copy` when S5 added the
            // payload-bearing `Paused` variant; the slice elements
            // are borrowed in place and explicitly cloned where the
            // by-value API still demands ownership.
            assert!(
                w[0].can_transition_to(w[1].clone()),
                "{:?} -> {:?}",
                w[0],
                w[1]
            );
        }
    }

    #[test]
    fn mission_attention_interrupts_and_resumes() {
        use MissionState::*;
        assert!(Executing.can_transition_to(Attention));
        assert!(Reviewing.can_transition_to(Attention));
        assert!(Attention.can_transition_to(Executing));
        assert!(Attention.can_transition_to(Aborted));
    }

    #[test]
    fn mission_cannot_reenter_execution_without_a_scheduled_supervisor_turn() {
        assert!(!MissionState::CompletePendingMerge.can_transition_to(MissionState::Executing));
    }

    #[test]
    fn mission_terminal_states_are_sinks() {
        use MissionState::*;
        let terminals = [Merged, Discarded, Aborted];
        let any_target = [
            Created,
            Executing,
            Reviewing,
            Attention,
            CompletePendingMerge,
        ];
        for t in &terminals {
            assert!(t.is_terminal());
            for next in &any_target {
                assert!(
                    !t.can_transition_to(next.clone()),
                    "{t:?} should not reach {next:?}"
                );
            }
        }
    }

    #[test]
    fn mission_abort_reachable_from_any_non_terminal() {
        use MissionState::*;
        for s in [
            Created,
            Executing,
            Reviewing,
            Attention,
            CompletePendingMerge,
        ] {
            assert!(s.can_transition_to(Aborted), "abort from {s:?} should work");
        }
    }

    #[test]
    fn mission_transition_returns_error_on_invalid() {
        let err = MissionState::Created
            .transition(MissionState::Merged)
            .expect_err("Created -> Merged should be invalid");
        assert!(matches!(err, MissionError::InvalidMissionTransition { .. }));
    }

    #[test]
    fn worker_forward_path() {
        use WorkerTaskState::*;
        let path = [Spawned, Working, Submitting, UnderReview, Integrated];
        for w in path.windows(2) {
            assert!(w[0].can_transition_to(w[1]), "{:?} -> {:?}", w[0], w[1]);
        }
    }

    #[test]
    fn worker_revise_loops_back_to_working() {
        use WorkerTaskState::*;
        assert!(UnderReview.can_transition_to(Revising));
        assert!(Revising.can_transition_to(Working));
        assert!(Working.can_transition_to(Submitting));
        assert!(Submitting.can_transition_to(UnderReview));
    }

    #[test]
    fn worker_terminal_states_are_sinks() {
        use WorkerTaskState::*;
        let terminals = [Integrated, Discarded, Failed];
        let any_target = [Spawned, Working, Submitting, UnderReview, Revising];
        for t in terminals {
            assert!(t.is_terminal());
            for next in any_target {
                assert!(!t.can_transition_to(next));
            }
        }
    }

    #[test]
    fn worker_failure_reachable_from_any_non_terminal() {
        use WorkerTaskState::*;
        for s in [Spawned, Working, Submitting, UnderReview, Revising] {
            assert!(s.can_transition_to(Failed));
        }
    }

    #[test]
    fn resolve_action_only_from_complete_pending_merge_or_attention() {
        let action = ResolveAction::Merge;
        assert!(action.allowed_from(MissionState::CompletePendingMerge));
        // Attention is also a valid resolve origin so an
        // arbiter-escalated pause can be resolved without an
        // intervening transition.
        assert!(action.allowed_from(MissionState::Attention));
        for s in [
            MissionState::Created,
            MissionState::Executing,
            MissionState::Reviewing,
            MissionState::Merged,
            MissionState::Discarded,
            MissionState::Aborted,
        ] {
            assert!(!action.allowed_from(s.clone()), "should reject from {s:?}");
        }
    }

    #[test]
    fn reserved_extend_variant_carries_directive_but_is_not_actionable() {
        let action = ResolveAction::Extend {
            directive: Some("also add docs".into()),
        };
        match action {
            ResolveAction::Extend { directive } => {
                assert_eq!(directive.as_deref(), Some("also add docs"))
            }
            _ => panic!("expected Extend"),
        }
        assert!(!ResolveAction::Extend { directive: None }
            .allowed_from(MissionState::CompletePendingMerge));
    }

    #[test]
    fn resolve_action_extend_accepts_none_directive() {
        let action = ResolveAction::Extend { directive: None };
        match action {
            ResolveAction::Extend { directive } => assert!(directive.is_none()),
            _ => panic!("expected Extend"),
        }
    }

    #[test]
    fn paused_is_reachable_from_executing_and_reviewing() {
        use MissionState::*;
        let pause = Paused {
            reason: PauseReason::WaitingForQuota {
                vendor: event_schema::Vendor::Claude,
            },
        };
        assert!(Executing.can_transition_to(pause.clone()));
        assert!(Reviewing.can_transition_to(pause));
    }

    #[test]
    fn paused_can_resume_to_executing() {
        use MissionState::*;
        let pause = Paused {
            reason: PauseReason::WaitingForQuota {
                vendor: event_schema::Vendor::Claude,
            },
        };
        assert!(pause.can_transition_to(Executing));
    }

    #[test]
    fn paused_can_abort() {
        use MissionState::*;
        let pause = Paused {
            reason: PauseReason::WaitingForQuota {
                vendor: event_schema::Vendor::Claude,
            },
        };
        assert!(pause.can_transition_to(Aborted));
    }

    #[test]
    fn paused_is_not_terminal() {
        let pause = MissionState::Paused {
            reason: PauseReason::WaitingForQuota {
                vendor: event_schema::Vendor::Claude,
            },
        };
        assert!(!pause.is_terminal());
    }
}
