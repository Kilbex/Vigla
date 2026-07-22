//! Host-facing services with no Tauri dependency.
//!
//! The desktop host, future Linux/Windows shells, and tests should call
//! these APIs instead of reimplementing business rules in their UI
//! integration layers. The Tauri crate remains responsible for IPC and
//! event forwarding only.

use crate::ids::new_mission_id;
use crate::memory::{MemoryKernel, MemoryRegistry};
use crate::mission_runtime::{MissionRuntimeError, MockTimingConfig};
use crate::mission_supervisor_run::{
    worker_model_selection_is_valid, RealClaudeConfig, ScriptedSupervisor, SupervisorDriver,
    WorkerBackend, L1_CLAUDE_QUOTA_EXHAUSTED_WORKER_MODEL,
};
use crate::mission_workspace::MissionGitError;
use crate::RepositoryError;
use crate::{
    DispositionAction, MergeResolution, MissionEventKind, MissionEventReceiver, MissionRuntime,
    MissionSpec, MissionState, MissionWorkspace, Repository, ResolveAction, Supervisor,
    SupervisorError,
};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;
use thiserror::Error;
use tokio::sync::Mutex;

#[derive(Debug, Error)]
pub enum HostServiceError {
    #[error("working directory is empty")]
    WorkingDirectoryEmpty,

    #[error("working directory does not exist or is unreadable: {0}")]
    WorkingDirectoryUnreadable(String),

    #[error("working directory is not a directory: {0}")]
    WorkingDirectoryNotDirectory(PathBuf),

    #[error("working directory is not inside a Git worktree: {path} ({reason})")]
    WorkingDirectoryNotGitRepository { path: PathBuf, reason: String },

    #[error("prompt is empty")]
    PromptEmpty,

    #[error("a mission is already active (state: {state:?}); abort or resolve it first")]
    ActiveMission { state: MissionState },

    #[error("a mission was started concurrently (state: {state:?}); abort or resolve it first")]
    ConcurrentMission { state: MissionState },

    #[error("no active mission")]
    NoActiveMission,

    #[error(
        "worker model `{model}` is not recognized - use `auto`, a supported vendor (`claude`, `codex`, `antigravity`, `gemini`, `kiro`, or `copilot`), or a comma-separated roster such as `claude:sonnet,codex:gpt-5.5,antigravity:auto`"
    )]
    UnsupportedWorkerModel { model: String },

    #[error(
        "supervisor model `{model}` is not supported - production missions require `claude`; `auto` selects the deterministic mock supervisor"
    )]
    UnsupportedSupervisorModel { model: String },

    #[error("could not determine target branch for mission: {reason}")]
    TargetRefUnavailable { reason: String },

    #[error("failed to run git diff: {0}")]
    GitDiffIo(String),

    #[error("invalid utf8 in git diff: {0}")]
    GitDiffUtf8(String),

    #[error(transparent)]
    Supervisor(#[from] SupervisorError),

    #[error(transparent)]
    MissionRuntime(#[from] MissionRuntimeError),

    #[error(transparent)]
    MissionGit(#[from] MissionGitError),

    #[error(transparent)]
    Repository(#[from] RepositoryError),

    #[error("mission artifact cleanup refused: {0}")]
    MissionCleanupRefused(String),
}

/// Validate a user-supplied working directory path and return a
/// concrete path. This is intentionally filesystem-only and UI-agnostic.
pub fn validate_working_dir(raw: &str) -> Result<PathBuf, HostServiceError> {
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        return Err(HostServiceError::WorkingDirectoryEmpty);
    }
    let path = PathBuf::from(trimmed);
    let meta = std::fs::metadata(&path)
        .map_err(|e| HostServiceError::WorkingDirectoryUnreadable(e.to_string()))?;
    if !meta.is_dir() {
        return Err(HostServiceError::WorkingDirectoryNotDirectory(path));
    }
    Ok(path)
}

/// Resolve any selected path inside a Git worktree to one canonical repository
/// identity. Mission workspaces, memory, skills, retention, history, and revert
/// must all use this exact top-level path rather than independent aliases.
pub fn resolve_git_repo_root(raw: &str) -> Result<PathBuf, HostServiceError> {
    let selected = validate_working_dir(raw)?;
    let selected = std::fs::canonicalize(&selected)
        .map_err(|error| HostServiceError::WorkingDirectoryUnreadable(error.to_string()))?;
    let stdout = MissionWorkspace::run_git_process_bytes_sync_in(
        &selected,
        &["rev-parse", "--show-toplevel"],
    )
    .map_err(|error| HostServiceError::WorkingDirectoryNotGitRepository {
        path: selected.clone(),
        reason: error.to_string(),
    })?;
    let top_level = String::from_utf8(stdout).map_err(|error| {
        HostServiceError::WorkingDirectoryNotGitRepository {
            path: selected.clone(),
            reason: error.to_string(),
        }
    })?;
    std::fs::canonicalize(top_level.trim()).map_err(|error| {
        HostServiceError::WorkingDirectoryNotGitRepository {
            path: selected,
            reason: error.to_string(),
        }
    })
}

/// Remove the Vigla-owned Git artifacts retained by an aborted mission and
/// durably record completion. The outcome is the authorization source and the
/// recorded canonical repository is the only accepted cleanup target.
///
/// Git cleanup is idempotent and runs before the marker is written. If the
/// process stops between those operations, retrying safely finishes the same
/// cleanup without touching the target branch.
pub async fn cleanup_aborted_mission_artifacts(
    repository: &Repository,
    mission_id: &str,
) -> Result<(), HostServiceError> {
    let terminal = repository
        .mission_outcome(mission_id)
        .await?
        .ok_or_else(|| {
            HostServiceError::MissionCleanupRefused(format!(
                "mission {mission_id} has no durable aborted outcome"
            ))
        })?;
    if terminal.state != crate::MissionOutcomeState::Aborted {
        return Err(HostServiceError::MissionCleanupRefused(format!(
            "mission {mission_id} ended as {} rather than aborted",
            terminal.state.as_str()
        )));
    }
    if repository.mission_artifacts_cleaned(mission_id).await? {
        return Ok(());
    }

    let recorded_root = terminal.repo_root.as_deref().ok_or_else(|| {
        HostServiceError::MissionCleanupRefused(format!(
            "mission {mission_id} is a legacy outcome with no recorded repository root"
        ))
    })?;
    let repo_root = resolve_git_repo_root(recorded_root)?;
    if repo_root.to_string_lossy() != recorded_root {
        return Err(HostServiceError::MissionCleanupRefused(format!(
            "mission {mission_id} repository identity changed from {recorded_root:?} to {}",
            repo_root.display()
        )));
    }

    MissionWorkspace::new(repo_root, mission_id.to_owned())?
        .discard()
        .await?;
    repository
        .record_mission_cleanup(mission_id, recorded_root, &crate::ids::rfc3339_now())
        .await?;
    Ok(())
}

pub fn validate_prompt(prompt: &str) -> Result<(), HostServiceError> {
    if prompt.trim().is_empty() {
        return Err(HostServiceError::PromptEmpty);
    }
    Ok(())
}

/// Reconcile terminal-disposition intents left by an interrupted host process.
///
/// Final Git anchors are the proof for a merge; without both valid anchors the
/// abandoned mission is conservatively cleaned and recorded as aborted. A
/// discard is replayed idempotently. Failed rows remain in the journal for the
/// next startup and are reported together.
pub async fn reconcile_disposition_journal(repository: &Repository) -> Result<usize, String> {
    let intents = repository
        .list_disposition_intents()
        .await
        .map_err(|error| format!("read disposition journal: {error}"))?;
    let mut reconciled = 0usize;
    let mut failures = Vec::new();

    for intent in intents {
        let result: Result<(), String> = async {
            let configured_root = PathBuf::from(&intent.repo_root);
            let canonical_root = std::fs::canonicalize(&configured_root).map_err(|error| {
                format!(
                    "repository {} is unavailable: {error}",
                    configured_root.display()
                )
            })?;
            if canonical_root.to_string_lossy() != intent.repo_root {
                return Err(format!(
                    "repository identity changed from {:?} to {}; refusing to guess",
                    intent.repo_root,
                    canonical_root.display()
                ));
            }
            let workspace = MissionWorkspace::new(canonical_root, intent.mission_id.clone())
                .map_err(|error| error.to_string())?;
            let now = crate::ids::rfc3339_now();

            match intent.action {
                DispositionAction::Merge => {
                    if workspace
                        .final_merge_is_applied(&intent.target_ref)
                        .await
                        .map_err(|error| error.to_string())?
                    {
                        repository
                            .record_mission_outcome(
                                &intent.mission_id,
                                &intent.repo_root,
                                &intent.target_ref,
                                crate::MissionOutcomeState::Merged,
                                &now,
                            )
                            .await
                            .map_err(|error| error.to_string())?;
                    } else {
                        workspace
                            .discard()
                            .await
                            .map_err(|error| error.to_string())?;
                        repository
                            .record_mission_outcome(
                                &intent.mission_id,
                                &intent.repo_root,
                                &intent.target_ref,
                                crate::MissionOutcomeState::Aborted,
                                &now,
                            )
                            .await
                            .map_err(|error| error.to_string())?;
                    }
                }
                DispositionAction::Discard => {
                    workspace
                        .discard()
                        .await
                        .map_err(|error| error.to_string())?;
                    repository
                        .record_mission_outcome(
                            &intent.mission_id,
                            &intent.repo_root,
                            &intent.target_ref,
                            crate::MissionOutcomeState::Discarded,
                            &now,
                        )
                        .await
                        .map_err(|error| error.to_string())?;
                }
            }

            // For a proven merge, cleanup is post-commit work. Keep the row if
            // cleanup fails so a future startup retries it.
            if intent.action == DispositionAction::Merge {
                workspace
                    .discard()
                    .await
                    .map_err(|error| error.to_string())?;
            }
            repository
                .clear_disposition_intent(&intent.mission_id)
                .await
                .map_err(|error| error.to_string())?;
            Ok(())
        }
        .await;

        match result {
            Ok(()) => reconciled += 1,
            Err(error) => failures.push(format!("{}: {error}", intent.mission_id)),
        }
    }

    if failures.is_empty() {
        Ok(reconciled)
    } else {
        Err(format!(
            "{} disposition intent(s) remain unresolved: {}",
            failures.len(),
            failures.join("; ")
        ))
    }
}

pub async fn start_claude_worker(
    supervisor: &Arc<Supervisor>,
    prompt: String,
    cwd: &str,
    max_turns: Option<u32>,
) -> Result<String, HostServiceError> {
    validate_prompt(&prompt)?;
    let working_dir = validate_working_dir(cwd)?;
    supervisor
        .spawn_claude(prompt, working_dir, max_turns.unwrap_or(8))
        .await
        .map_err(HostServiceError::from)
}

pub async fn start_codex_worker(
    supervisor: &Arc<Supervisor>,
    prompt: String,
    cwd: &str,
) -> Result<String, HostServiceError> {
    validate_prompt(&prompt)?;
    let working_dir = validate_working_dir(cwd)?;
    supervisor
        .spawn_codex(prompt, working_dir)
        .await
        .map_err(HostServiceError::from)
}

pub async fn start_gemini_worker(
    supervisor: &Arc<Supervisor>,
    prompt: String,
    cwd: &str,
) -> Result<String, HostServiceError> {
    validate_prompt(&prompt)?;
    let working_dir = validate_working_dir(cwd)?;
    supervisor
        .spawn_gemini(prompt, working_dir)
        .await
        .map_err(HostServiceError::from)
}

pub async fn continue_worker(
    supervisor: &Arc<Supervisor>,
    worker_id: &str,
    prompt: &str,
) -> Result<(), HostServiceError> {
    validate_prompt(prompt)?;
    supervisor
        .continue_worker(worker_id, prompt)
        .await
        .map_err(HostServiceError::from)
}

pub async fn get_worker_diff(
    supervisor: &Arc<Supervisor>,
    worker_id: &str,
) -> Result<String, HostServiceError> {
    let worker_info = supervisor.get_worker_info(worker_id).await?;
    let stdout = match MissionWorkspace::run_git_process_bytes_in(
        Path::new(&worker_info.cwd),
        &["diff", "--no-ext-diff", "--unified=3"],
    )
    .await
    {
        Ok(stdout) => stdout,
        Err(MissionGitError::Git { .. }) => return Ok(String::new()),
        Err(error) => return Err(HostServiceError::GitDiffIo(error.to_string())),
    };

    String::from_utf8(stdout).map_err(|e| HostServiceError::GitDiffUtf8(e.to_string()))
}

/// Result of starting a mission. UI hosts forward `events` through
/// their own event transport and return `mission_id` through IPC.
#[derive(Debug)]
pub struct StartedMission {
    pub mission_id: String,
    pub events: MissionEventReceiver,
}

/// One-active-mission controller. It is deliberately independent of
/// Tauri so lifecycle policy can be tested and reused by any host.
#[derive(Default)]
pub struct MissionController {
    runtime: Mutex<Option<MissionRuntime>>,
    /// A2 (Tier-2G): registry of per-repo memory kernels. The
    /// controller resolves the right kernel from the `cwd` passed to
    /// `start_mission`, opening one on first touch. When `None`, the
    /// controller starts missions without memory — exactly the
    /// pre-Tier-2A behaviour.
    memory_registry: Mutex<Option<Arc<MemoryRegistry>>>,
    /// Persistent audit history is recorded from a dedicated mission-event
    /// subscription, independent of any UI host's forwarding loop.
    repository: Mutex<Option<Repository>>,
}

impl MissionController {
    /// A2: install the memory registry. The controller consults the
    /// registry on each `start_mission` to resolve the right kernel
    /// for the mission's working directory. Idempotent — replaces
    /// any previously-installed registry; existing in-flight missions
    /// keep whatever kernel they captured at start time.
    pub async fn install_memory_registry(&self, registry: Arc<MemoryRegistry>) {
        *self.memory_registry.lock().await = Some(registry);
    }

    /// Install the repository used for cross-mission audit history. Headless
    /// callers may omit it; desktop startup installs the same repository used
    /// for canonical worker events.
    pub async fn install_repository(&self, repository: Repository) {
        *self.repository.lock().await = Some(repository);
    }

    /// Snapshot of the currently-installed registry, if any.
    async fn current_registry(&self) -> Option<Arc<MemoryRegistry>> {
        self.memory_registry.lock().await.clone()
    }

    async fn current_repository(&self) -> Option<Repository> {
        self.repository.lock().await.clone()
    }

    /// Build the per-mission skill library for `repo_root`. Fail-soft: load
    /// errors are impossible today (bundled skills are compiled in; user-skill
    /// errors are swallowed inside `open_for_repo`), but the `Option` keeps the
    /// headless/test contract identical to memory (`None` = skills disabled).
    async fn resolve_skills_for_repo(
        repo_root: &std::path::Path,
    ) -> Option<Arc<crate::skills::SkillLibrary>> {
        Some(Arc::new(
            crate::skills::SkillLibrary::open_for_repo(repo_root).await,
        ))
    }

    /// Resolve the per-repo memory kernel for `repo_root`. Returns
    /// `None` when no registry is installed (test paths, headless
    /// orchestrator). On registry-installed paths a per-repo kernel
    /// failure is logged and `None` is returned — the release-gate
    /// contract that memory never blocks dispatch.
    async fn resolve_memory_for_repo(
        &self,
        repo_root: &std::path::Path,
    ) -> Option<Arc<MemoryKernel>> {
        let registry = self.current_registry().await?;
        match registry.get_or_open(repo_root).await {
            Ok(k) => Some(k),
            Err(e) => {
                tracing::warn!(
                    "vigla: memory kernel unavailable for {}: {e}",
                    repo_root.display()
                );
                None
            }
        }
    }

    pub async fn start_mission(
        &self,
        spec: MissionSpec,
        cwd: &str,
    ) -> Result<StartedMission, HostServiceError> {
        let repo_root = resolve_git_repo_root(cwd)?;
        let spec = resolve_mission_target_ref(spec, &repo_root).await?;
        let target_ref = spec.target_ref.clone();
        let repo_root_for_history = repo_root.to_string_lossy().into_owned();

        // Hold the slot from the active-state check through workspace startup.
        // Startup creates durable branches and worktrees before it can return a
        // runtime, so releasing the slot in between lets two callers create
        // artifacts and forces the loser to abandon an unregistered mission.
        let mut runtime_slot = self.runtime.lock().await;
        if let Some(existing) = runtime_slot.as_ref() {
            let state = existing.state();
            if !is_terminal(&state) {
                return Err(HostServiceError::ActiveMission { state });
            }
        }

        let mission_id = new_mission_id(&spec.title);
        let workspace = MissionWorkspace::new(repo_root.clone(), mission_id.clone())?;
        let memory = self.resolve_memory_for_repo(&repo_root).await;
        let skills = Self::resolve_skills_for_repo(&repo_root).await;
        let runtime = select_and_start_mission_with_memory(spec, workspace, memory, skills).await?;

        let history_repository = self.current_repository().await;
        let compaction_repository = history_repository.clone();
        if let Some(repository) = &history_repository {
            runtime.install_disposition_store(repository.clone()).await;
        }

        let events = runtime.subscribe();
        if let Some(repository) = history_repository {
            let audit_events = runtime.subscribe();
            crate::spawn_supervised("mission history recorder", async move {
                record_mission_history(audit_events, repository, repo_root_for_history, target_ref)
                    .await;
            });
        }
        *runtime_slot = Some(runtime);
        drop(runtime_slot);
        if let Some(repository) = compaction_repository {
            crate::mission_workspace::retention::spawn_repo_compaction_if_due(
                repo_root,
                repository,
                crate::mission_workspace::retention::RetentionPolicy::default(),
            );
        }
        Ok(StartedMission { mission_id, events })
    }

    pub async fn abort_mission(&self) -> Result<(), HostServiceError> {
        let runtime = self.active_runtime().await?;
        runtime.abort().await.map_err(HostServiceError::from)
    }

    pub async fn resolve_mission(&self, action: ResolveAction) -> Result<(), HostServiceError> {
        let runtime = self.active_runtime().await?;
        runtime
            .resolve(action)
            .await
            .map_err(HostServiceError::from)
    }

    pub async fn confirm_plan(&self, generation: u32) -> Result<(), HostServiceError> {
        let runtime = self.active_runtime().await?;
        runtime
            .confirm_plan(generation)
            .await
            .map_err(HostServiceError::from)
    }

    pub async fn regenerate_plan(
        &self,
        generation: u32,
        hint: Option<String>,
    ) -> Result<(), HostServiceError> {
        let runtime = self.active_runtime().await?;
        runtime
            .regenerate_plan(generation, hint)
            .await
            .map_err(HostServiceError::from)
    }

    /// QC-3: forward `MissionRuntime::reject_plan` through the host
    /// services layer for the Tauri command. The runtime emits
    /// `PlanRejected` followed by `Aborted`; both flow back to the FE
    /// over the existing `mission://events` channel.
    pub async fn reject_plan(
        &self,
        generation: u32,
        reason: Option<String>,
    ) -> Result<(), HostServiceError> {
        let runtime = self.active_runtime().await?;
        runtime
            .reject_plan(generation, reason)
            .await
            .map_err(HostServiceError::from)
    }

    async fn active_runtime(&self) -> Result<MissionRuntime, HostServiceError> {
        let guard = self.runtime.lock().await;
        guard.clone().ok_or(HostServiceError::NoActiveMission)
    }

    /// Look up the live state for `mission_id`. Returns `None` when
    /// no runtime is currently registered for that id — typical for
    /// already-merged / aborted missions whose runtime handle was released.
    /// The host cross-checks this state before a merged-mission rollback.
    pub async fn mission_state(&self, mission_id: &str) -> Option<MissionState> {
        let guard = self.runtime.lock().await;
        guard
            .as_ref()
            .filter(|rt| rt.mission_id() == mission_id)
            .map(|rt| rt.state())
    }
}

/// Persist audits and the eventual terminal disposition from a dedicated
/// subscription. A slow or absent UI therefore cannot make History empty or
/// misclassify a discarded/aborted mission as merged.
async fn record_mission_history(
    mut events: MissionEventReceiver,
    repository: Repository,
    repo_root: String,
    target_ref: String,
) {
    use tokio::sync::broadcast::error::RecvError;

    let mut latest: Option<(String, crate::audit::AuditReport, String)> = None;
    let mut mission_level_written = false;

    loop {
        let event = match events.recv().await {
            Ok(event) => event,
            Err(RecvError::Lagged(skipped)) => {
                tracing::warn!(
                    "vigla: audit history recorder lagged and skipped {skipped} mission events"
                );
                continue;
            }
            Err(RecvError::Closed) => break,
        };

        match &event.kind {
            MissionEventKind::AuditCompleted {
                tier, payload_json, ..
            } => match serde_json::from_str::<crate::audit::AuditReport>(payload_json) {
                Ok(report) => latest = Some((tier.clone(), report, event.ts.clone())),
                Err(error) => tracing::warn!(
                    "vigla: ignored malformed audit payload for mission {}: {error}",
                    event.mission_id
                ),
            },
            MissionEventKind::PostIntegrationAuditCompleted {
                worker_id,
                tier,
                payload_json,
                ..
            } => match serde_json::from_str::<crate::audit::AuditReport>(payload_json) {
                Ok(report) => {
                    if let Err(error) = repository
                        .record_audit_at(
                            &event.mission_id,
                            Some(worker_id),
                            tier,
                            &report,
                            &event.ts,
                        )
                        .await
                    {
                        tracing::warn!(
                            "vigla: failed to persist worker audit for mission {}: {error}",
                            event.mission_id
                        );
                    }
                    latest = Some((tier.clone(), report, event.ts.clone()));
                }
                Err(error) => tracing::warn!(
                    "vigla: ignored malformed post-integration audit for mission {}: {error}",
                    event.mission_id
                ),
            },
            MissionEventKind::Completed { .. } => {
                mission_level_written =
                    persist_latest_mission_audit(&repository, &event.mission_id, latest.as_ref())
                        .await;
            }
            MissionEventKind::Aborted { .. } => {
                persist_mission_outcome(
                    &repository,
                    &event.mission_id,
                    &repo_root,
                    &target_ref,
                    crate::MissionOutcomeState::Aborted,
                    &event.ts,
                )
                .await;
                if !mission_level_written {
                    persist_latest_mission_audit(&repository, &event.mission_id, latest.as_ref())
                        .await;
                }
                break;
            }
            MissionEventKind::MergeResolved { resolution }
                if matches!(
                    resolution,
                    MergeResolution::Merged | MergeResolution::Discarded
                ) =>
            {
                let state = match resolution {
                    MergeResolution::Merged => crate::MissionOutcomeState::Merged,
                    MergeResolution::Discarded => crate::MissionOutcomeState::Discarded,
                    MergeResolution::Extended { .. } => unreachable!("guarded above"),
                };
                persist_mission_outcome(
                    &repository,
                    &event.mission_id,
                    &repo_root,
                    &target_ref,
                    state,
                    &event.ts,
                )
                .await;
                if !mission_level_written {
                    persist_latest_mission_audit(&repository, &event.mission_id, latest.as_ref())
                        .await;
                }
                break;
            }
            _ => {}
        }
    }
}

async fn persist_mission_outcome(
    repository: &Repository,
    mission_id: &str,
    repo_root: &str,
    target_ref: &str,
    state: crate::MissionOutcomeState,
    event_ts: &str,
) {
    if let Err(error) = repository
        .record_mission_outcome(mission_id, repo_root, target_ref, state, event_ts)
        .await
    {
        tracing::warn!(
            "vigla: failed to persist {} outcome for mission {mission_id}: {error}",
            state.as_str()
        );
    }
}

async fn persist_latest_mission_audit(
    repository: &Repository,
    mission_id: &str,
    latest: Option<&(String, crate::audit::AuditReport, String)>,
) -> bool {
    let Some((tier, report, audit_ts)) = latest else {
        return false;
    };
    match repository
        .record_audit_at(mission_id, None, tier, report, audit_ts)
        .await
    {
        Ok(()) => true,
        Err(error) => {
            tracing::warn!(
                "vigla: failed to persist mission-level audit for mission {mission_id}: {error}"
            );
            false
        }
    }
}

pub async fn select_and_start_mission(
    spec: MissionSpec,
    workspace: MissionWorkspace,
) -> Result<MissionRuntime, HostServiceError> {
    select_and_start_mission_with_memory(spec, workspace, None, None).await
}

/// As [`select_and_start_mission`] but threads a memory kernel into
/// the runtime so worker dispatch composes a curated bundle into the
/// worktree and resolution emits a barrier.
pub async fn select_and_start_mission_with_memory(
    spec: MissionSpec,
    workspace: MissionWorkspace,
    memory: Option<Arc<MemoryKernel>>,
    skills: Option<Arc<crate::skills::SkillLibrary>>,
) -> Result<MissionRuntime, HostServiceError> {
    let force_mock = std::env::var("VIGLA_FORCE_MOCK_SUPERVISOR")
        .ok()
        .map(|v| v == "1")
        .unwrap_or(false);
    let l1_quota_mock = l1_quota_mock_enabled();
    let model = spec.supervisor_model.as_deref().unwrap_or("auto");

    if l1_quota_mock
        && spec
            .worker_model
            .as_deref()
            .is_some_and(is_l1_quota_worker_model)
    {
        if model != "claude" {
            return Err(HostServiceError::UnsupportedSupervisorModel {
                model: model.to_owned(),
            });
        }
        let driver = SupervisorDriver::Scripted(l1_quota_scripted_supervisor());
        return MissionRuntime::start_supervised_with_memory(
            spec,
            workspace,
            driver,
            WorkerBackend::L1ClaudeQuotaExhausted,
            memory,
            skills,
        )
        .await
        .map_err(HostServiceError::from);
    }

    match (model, force_mock) {
        ("auto", _) | ("claude", true) => MissionRuntime::start_with_memory(
            spec,
            workspace,
            MockTimingConfig::realtime(),
            memory,
            skills,
        )
        .await
        .map_err(HostServiceError::from),
        ("claude", false) => {
            let worker_backend = worker_backend_for_model(spec.worker_model.as_deref())?;
            let driver = SupervisorDriver::RealClaude(RealClaudeConfig {
                binary: "claude".into(),
                model: None,
                turn_timeout: Duration::from_secs(120),
            });
            MissionRuntime::start_supervised_with_memory(
                spec,
                workspace,
                driver,
                worker_backend,
                memory,
                skills,
            )
            .await
            .map_err(HostServiceError::from)
        }
        (other, _) => Err(HostServiceError::UnsupportedSupervisorModel {
            model: other.to_owned(),
        }),
    }
}

pub fn worker_backend_for_model(model: Option<&str>) -> Result<WorkerBackend, HostServiceError> {
    worker_backend_for_model_inner(model, l1_quota_mock_enabled())
}

fn worker_backend_for_model_inner(
    model: Option<&str>,
    l1_quota_mock: bool,
) -> Result<WorkerBackend, HostServiceError> {
    match model.unwrap_or("auto").trim() {
        "auto" | "Auto" => Ok(WorkerBackend::AutoReal),
        other if l1_quota_mock && is_l1_quota_worker_model(other) => {
            Ok(WorkerBackend::L1ClaudeQuotaExhausted)
        }
        other if worker_model_selection_is_valid(other) => {
            let spec = MissionSpec {
                title: String::new(),
                objective: String::new(),
                target_ref: String::new(),
                tests: None,
                supervisor_model: None,
                worker_model: Some(other.to_owned()),
                worker_count: None,
                confirm_plan: None,
                scope_paths: Vec::new(),
            };
            Ok(crate::mission_supervisor_run::select_worker_backend(&spec))
        }
        other => Err(HostServiceError::UnsupportedWorkerModel {
            model: other.to_owned(),
        }),
    }
}

fn is_l1_quota_worker_model(model: &str) -> bool {
    model.trim() == L1_CLAUDE_QUOTA_EXHAUSTED_WORKER_MODEL
}

fn l1_quota_mock_enabled() -> bool {
    std::env::var("VIGLA_L1_QUOTA_MOCK")
        .ok()
        .is_some_and(|v| v == "1")
}

fn l1_quota_scripted_supervisor() -> ScriptedSupervisor {
    use supervisor_adapter::{SupervisorIntent, SupervisorOutput, SupervisorTaskDescriptor};

    ScriptedSupervisor::new(vec![vec![SupervisorOutput::Intent(
        SupervisorIntent::Decompose {
            tasks: vec![SupervisorTaskDescriptor {
                title: "Trigger deterministic Claude quota pause".into(),
                description: Some(
                    "L1 row-4 smoke task; the worker backend emits a Claude quota signal once."
                        .into(),
                ),
                ..Default::default()
            }],
            overview: Some("Exercise the quota pause and countdown surface.".into()),
            tech_stack: None,
            envelope_fit: None,
        },
    )]])
}

async fn resolve_mission_target_ref(
    mut spec: MissionSpec,
    repo_root: &Path,
) -> Result<MissionSpec, HostServiceError> {
    let requested = spec.target_ref.trim();
    if requested.is_empty() {
        spec.target_ref = current_local_branch(repo_root).await?;
        return Ok(spec);
    }

    // Backward compatibility for existing UI builds that still sent
    // "main" as an implicit default: if the repo has no main ref,
    // use the checked-out local branch instead.
    if requested == "main" && !git_commit_exists(repo_root, requested).await? {
        spec.target_ref = current_local_branch(repo_root).await?;
    } else if requested != spec.target_ref {
        spec.target_ref = requested.to_string();
    }
    Ok(spec)
}

async fn current_local_branch(repo_root: &Path) -> Result<String, HostServiceError> {
    let branch = match MissionWorkspace::run_git_process_in(
        repo_root,
        &["symbolic-ref", "--quiet", "--short", "HEAD"],
    )
    .await
    {
        Ok(branch) => branch,
        Err(MissionGitError::Git { stderr, .. }) => {
            let reason = if stderr.is_empty() {
                "the repository is in detached HEAD; check out a local branch first".into()
            } else {
                stderr
            };
            return Err(HostServiceError::TargetRefUnavailable { reason });
        }
        Err(error) => {
            return Err(HostServiceError::TargetRefUnavailable {
                reason: error.to_string(),
            });
        }
    };
    if branch.is_empty() {
        return Err(HostServiceError::TargetRefUnavailable {
            reason: "git did not report a current branch".into(),
        });
    }
    Ok(branch)
}

async fn git_commit_exists(repo_root: &Path, refname: &str) -> Result<bool, HostServiceError> {
    let commitish = format!("{refname}^{{commit}}");
    match MissionWorkspace::run_git_process_in(
        repo_root,
        &["rev-parse", "--verify", "--quiet", &commitish],
    )
    .await
    {
        Ok(_) => Ok(true),
        Err(MissionGitError::Git { .. }) => Ok(false),
        Err(error) => Err(HostServiceError::TargetRefUnavailable {
            reason: error.to_string(),
        }),
    }
}

fn is_terminal(state: &MissionState) -> bool {
    matches!(
        state,
        MissionState::Merged | MissionState::Discarded | MissionState::Aborted
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::mission_event::MissionEvent;
    use crate::mission_runtime::MissionEventBus;
    use crate::vendor_profile::WorkerVendor;
    use std::process::Command as SyncCommand;
    use tempfile::TempDir;

    #[test]
    fn validate_working_dir_rejects_missing_path() {
        let err = validate_working_dir("/this/path/does/not/exist").unwrap_err();
        assert!(err.to_string().contains("does not exist"), "msg = {err}");
    }

    #[test]
    fn validate_working_dir_rejects_non_directory() {
        let temp = TempDir::new().unwrap();
        let file = temp.path().join("not-dir");
        std::fs::write(&file, b"x").unwrap();

        let err = validate_working_dir(file.to_str().unwrap()).unwrap_err();

        assert!(err.to_string().contains("not a directory"), "msg = {err}");
    }

    #[test]
    fn validate_working_dir_accepts_existing_directory() {
        let temp = TempDir::new().unwrap();
        let path = validate_working_dir(temp.path().to_str().unwrap()).unwrap();
        assert_eq!(path, temp.path());
    }

    #[test]
    fn resolve_git_repo_root_rejects_a_non_git_directory() {
        let temp = TempDir::new().unwrap();
        let error = resolve_git_repo_root(temp.path().to_str().unwrap()).unwrap_err();
        assert!(matches!(
            error,
            HostServiceError::WorkingDirectoryNotGitRepository { .. }
        ));
    }

    #[test]
    fn resolve_git_repo_root_canonicalizes_a_selected_subdirectory() {
        let (_temp, root) = make_sandbox_repo();
        let selected = root.join("src/nested");
        std::fs::create_dir_all(&selected).unwrap();

        let resolved = resolve_git_repo_root(selected.to_str().unwrap()).unwrap();

        assert_eq!(resolved, std::fs::canonicalize(root).unwrap());
    }

    #[cfg(unix)]
    #[test]
    fn resolve_git_repo_root_collapses_symlink_aliases() {
        let (_temp, root) = make_sandbox_repo();
        let aliases = TempDir::new().unwrap();
        let alias = aliases.path().join("repo-alias");
        std::os::unix::fs::symlink(&root, &alias).unwrap();

        let resolved = resolve_git_repo_root(alias.to_str().unwrap()).unwrap();

        assert_eq!(resolved, std::fs::canonicalize(root).unwrap());
    }

    #[test]
    fn validate_prompt_rejects_empty_text() {
        let err = validate_prompt(" \n\t").unwrap_err();
        assert!(matches!(err, HostServiceError::PromptEmpty));
    }

    #[test]
    fn worker_backend_for_model_preserves_routing_policy() {
        assert!(matches!(
            worker_backend_for_model(None).unwrap(),
            WorkerBackend::AutoReal
        ));
        assert!(matches!(
            worker_backend_for_model(Some("auto")).unwrap(),
            WorkerBackend::AutoReal
        ));
        assert!(matches!(
            worker_backend_for_model(Some("claude")).unwrap(),
            WorkerBackend::RealCli(WorkerVendor::Claude)
        ));
        assert!(matches!(
            worker_backend_for_model(Some("codex")).unwrap(),
            WorkerBackend::RealCli(WorkerVendor::Codex)
        ));
        assert!(matches!(
            worker_backend_for_model(Some("gemini")).unwrap(),
            WorkerBackend::RealCli(WorkerVendor::Gemini)
        ));
        assert!(matches!(
            worker_backend_for_model(Some("claude,codex,gemini")).unwrap(),
            WorkerBackend::Roster(_)
        ));
        assert!(matches!(
            worker_backend_for_model(Some("claude:sonnet,codex:gpt-5.5,gemini")).unwrap(),
            WorkerBackend::Roster(_)
        ));
        assert!(matches!(
            worker_backend_for_model(Some("claude:haiku")).unwrap(),
            WorkerBackend::RealCli(WorkerVendor::Claude)
        ));
        assert!(matches!(
            worker_backend_for_model_inner(
                Some(crate::mission_supervisor_run::L1_CLAUDE_QUOTA_EXHAUSTED_WORKER_MODEL),
                false,
            )
            .unwrap_err(),
            HostServiceError::UnsupportedWorkerModel { .. }
        ));
        assert!(matches!(
            worker_backend_for_model_inner(
                Some(crate::mission_supervisor_run::L1_CLAUDE_QUOTA_EXHAUSTED_WORKER_MODEL),
                true,
            )
            .unwrap(),
            WorkerBackend::L1ClaudeQuotaExhausted
        ));
        assert!(matches!(
            worker_backend_for_model(Some("opencode")).unwrap_err(),
            HostServiceError::UnsupportedWorkerModel { .. }
        ));
    }

    #[tokio::test]
    async fn mission_controller_rejects_second_active_mission() {
        let (_temp, root) = make_sandbox_repo();
        let controller = MissionController::default();

        let first = controller
            .start_mission(ok_spec("first"), root.to_str().unwrap())
            .await
            .expect("first mission starts");
        let second = controller
            .start_mission(ok_spec("second"), root.to_str().unwrap())
            .await
            .unwrap_err();

        assert!(matches!(second, HostServiceError::ActiveMission { .. }));
        controller
            .abort_mission()
            .await
            .expect("abort first mission");
        drop(first);
    }

    #[tokio::test]
    async fn concurrent_mission_start_reserves_before_creating_git_artifacts() {
        let (_temp, root) = make_sandbox_repo();
        let controller = Arc::new(MissionController::default());
        let first_controller = Arc::clone(&controller);
        let second_controller = Arc::clone(&controller);

        let (first, second) = tokio::join!(
            first_controller.start_mission(ok_spec("concurrent-first"), root.to_str().unwrap()),
            second_controller.start_mission(ok_spec("concurrent-second"), root.to_str().unwrap()),
        );

        assert_eq!(
            usize::from(first.is_ok()) + usize::from(second.is_ok()),
            1,
            "exactly one concurrent caller may reserve the mission slot"
        );
        let branches = SyncCommand::new("git")
            .args([
                "for-each-ref",
                "--format=%(refname:short)",
                "refs/heads/vigla/",
            ])
            .current_dir(&root)
            .output()
            .unwrap();
        let branch_listing = String::from_utf8_lossy(&branches.stdout);
        let supervisor_branches = branch_listing
            .lines()
            .filter(|branch| branch.ends_with("/supervisor"))
            .collect::<Vec<_>>();
        assert_eq!(
            supervisor_branches.len(),
            1,
            "a rejected concurrent start must not leave an orphan branch: {supervisor_branches:?}"
        );
        let worktree_root = root.join(".vigla/worktrees");
        let mission_dirs = std::fs::read_dir(worktree_root)
            .unwrap()
            .collect::<Result<Vec<_>, _>>()
            .unwrap();
        assert_eq!(
            mission_dirs.len(),
            1,
            "a rejected concurrent start must not leave an orphan worktree"
        );

        controller.abort_mission().await.unwrap();
    }

    #[tokio::test]
    async fn audit_event_recorder_populates_cross_mission_history() {
        let repository = Repository::open_in_memory().await.unwrap();
        let event_bus = MissionEventBus::new(16);
        let events = event_bus.subscribe();
        let report = crate::audit::AuditReport {
            overall: 0.82,
            ..Default::default()
        };
        let payload_json = serde_json::to_string(&report).unwrap();
        let mission_id = "mission-history-regression";

        event_bus.emit(MissionEvent {
            mission_id: mission_id.into(),
            seq: 1,
            ts: "2026-07-21T12:00:00.000Z".into(),
            kind: MissionEventKind::PostIntegrationAuditCompleted {
                worker_id: "worker-1".into(),
                tier: "standard".into(),
                overall: report.overall,
                payload_json,
            },
        });
        event_bus.emit(MissionEvent {
            mission_id: mission_id.into(),
            seq: 2,
            ts: "2026-07-21T12:00:01.000Z".into(),
            kind: MissionEventKind::Completed {
                summary: "ready for review".into(),
                files_changed: 1,
            },
        });
        event_bus.emit(MissionEvent {
            mission_id: mission_id.into(),
            seq: 3,
            ts: "2026-07-21T12:00:02.000Z".into(),
            kind: MissionEventKind::MergeResolved {
                resolution: MergeResolution::Discarded,
            },
        });

        record_mission_history(
            events,
            repository.clone(),
            "/repo/history".into(),
            "main".into(),
        )
        .await;

        let history = repository.list_recent_missions(10).await.unwrap();
        assert_eq!(history.len(), 1);
        assert_eq!(history[0].mission_id, mission_id);
        assert_eq!(history[0].tier, "standard");
        assert!((history[0].audit_overall - report.overall).abs() < 1e-9);

        let audits = crate::audit::persist::list_audits_for_mission(&repository.pool(), mission_id)
            .await
            .unwrap();
        assert_eq!(audits.len(), 2, "worker and mission summary are retained");
        assert!(audits
            .iter()
            .any(|row| row.worker_id.as_deref() == Some("worker-1")));
        assert!(audits.iter().any(|row| row.worker_id.is_none()));

        let outcome = repository
            .mission_outcome(mission_id)
            .await
            .unwrap()
            .expect("terminal disposition persisted");
        assert_eq!(outcome.target_ref, "main");
        assert_eq!(outcome.state, crate::MissionOutcomeState::Discarded);
    }

    /// C2 wiring: `mission_state` must return `None` for an unknown
    /// mission id (so the host's revert path does not refuse missions
    /// whose runtimes have already terminated and been released) and
    /// `Some(state)` for the active mission. The host's
    /// `revert_mission` command relies on this contract to refuse a
    /// still-live Paused mission while still allowing reverts on
    /// terminal corpses.
    #[tokio::test]
    async fn mission_state_lookup_matches_active_runtime() {
        let (_temp, root) = make_sandbox_repo();
        let controller = MissionController::default();

        // No active mission yet → any id returns None.
        assert!(controller.mission_state("nonexistent").await.is_none());

        let started = controller
            .start_mission(ok_spec("for-state-lookup"), root.to_str().unwrap())
            .await
            .expect("mission starts");

        // Right id → Some(state). Don't pin the exact variant: the
        // mock mission's exact state at this moment depends on timing.
        // What matters is that the lookup hits the active runtime.
        assert!(
            controller
                .mission_state(&started.mission_id)
                .await
                .is_some(),
            "active mission must be looked up by id"
        );

        // Wrong id → None even though a runtime is registered.
        assert!(
            controller
                .mission_state("mid-not-the-active-one")
                .await
                .is_none(),
            "mission_state must scope to the supplied id"
        );

        controller.abort_mission().await.expect("abort");
    }

    #[tokio::test]
    async fn empty_target_ref_defaults_to_current_local_branch() {
        let (_temp, root) = make_sandbox_repo_on("master");
        let spec = MissionSpec {
            target_ref: "".into(),
            ..ok_spec("default-target")
        };

        let resolved = resolve_mission_target_ref(spec, &root).await.unwrap();

        assert_eq!(resolved.target_ref, "master");
    }

    #[tokio::test]
    async fn missing_implicit_main_defaults_to_current_local_branch() {
        let (_temp, root) = make_sandbox_repo_on("master");
        let spec = MissionSpec {
            target_ref: "main".into(),
            ..ok_spec("missing-main")
        };

        let resolved = resolve_mission_target_ref(spec, &root).await.unwrap();

        assert_eq!(resolved.target_ref, "master");
    }

    #[tokio::test]
    async fn existing_main_target_ref_is_preserved() {
        let (_temp, root) = make_sandbox_repo();
        let spec = ok_spec("existing-main");

        let resolved = resolve_mission_target_ref(spec, &root).await.unwrap();

        assert_eq!(resolved.target_ref, "main");
    }

    #[tokio::test]
    async fn startup_reconciliation_recovers_git_applied_merge() {
        let (_temp, root) = make_sandbox_repo();
        let root = std::fs::canonicalize(root).unwrap();
        let mission_id = "reconcile-merged-0001";
        let workspace = MissionWorkspace::new(root.clone(), mission_id.into()).unwrap();
        workspace.create_supervisor_branch("main").await.unwrap();
        let supervisor = workspace.create_supervisor_worktree().await.unwrap();
        std::fs::write(supervisor.join("mission.txt"), "merged\n").unwrap();
        let commit = SyncCommand::new("git")
            .args(["add", "mission.txt"])
            .current_dir(&supervisor)
            .status()
            .unwrap();
        assert!(commit.success());
        let commit = SyncCommand::new("git")
            .args(["commit", "-m", "mission work"])
            .current_dir(&supervisor)
            .status()
            .unwrap();
        assert!(commit.success());

        let repository = Repository::open_in_memory().await.unwrap();
        repository
            .record_disposition_intent(
                mission_id,
                root.to_str().unwrap(),
                "main",
                DispositionAction::Merge,
                "2026-07-21T12:00:00Z",
            )
            .await
            .unwrap();
        workspace.final_merge("main").await.unwrap();

        assert_eq!(reconcile_disposition_journal(&repository).await.unwrap(), 1);
        let outcome = repository
            .mission_outcome(mission_id)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(outcome.state, crate::MissionOutcomeState::Merged);
        assert!(repository
            .list_disposition_intents()
            .await
            .unwrap()
            .is_empty());
        assert!(root.join("mission.txt").is_file());
    }

    #[tokio::test]
    async fn startup_reconciliation_aborts_unapplied_merge_intent() {
        let (_temp, root) = make_sandbox_repo();
        let root = std::fs::canonicalize(root).unwrap();
        let mission_id = "reconcile-unapplied-0001";
        let workspace = MissionWorkspace::new(root.clone(), mission_id.into()).unwrap();
        workspace.create_supervisor_branch("main").await.unwrap();
        workspace.create_supervisor_worktree().await.unwrap();
        let repository = Repository::open_in_memory().await.unwrap();
        repository
            .record_disposition_intent(
                mission_id,
                root.to_str().unwrap(),
                "main",
                DispositionAction::Merge,
                "2026-07-21T12:00:00Z",
            )
            .await
            .unwrap();

        assert_eq!(reconcile_disposition_journal(&repository).await.unwrap(), 1);
        assert_eq!(
            repository
                .mission_outcome(mission_id)
                .await
                .unwrap()
                .unwrap()
                .state,
            crate::MissionOutcomeState::Aborted
        );
        assert!(repository
            .list_disposition_intents()
            .await
            .unwrap()
            .is_empty());
    }

    fn ok_spec(title: &str) -> MissionSpec {
        MissionSpec {
            title: title.into(),
            objective: "exercise host service controller".into(),
            target_ref: "main".into(),
            tests: None,
            supervisor_model: None,
            worker_model: None,
            worker_count: Some(1),
            confirm_plan: None,
            scope_paths: vec![],
        }
    }

    fn make_sandbox_repo() -> (TempDir, PathBuf) {
        make_sandbox_repo_on("main")
    }

    fn make_sandbox_repo_on(branch: &str) -> (TempDir, PathBuf) {
        let temp = TempDir::new().expect("tempdir");
        let path = temp.path().to_path_buf();
        let run = |args: &[&str]| {
            let out = SyncCommand::new("git")
                .args(args)
                .current_dir(&path)
                .output()
                .expect("git command");
            assert!(
                out.status.success(),
                "git {args:?} failed: {}",
                String::from_utf8_lossy(&out.stderr)
            );
        };
        run(&["init", "--initial-branch", branch]);
        run(&["config", "user.email", "test@example.com"]);
        run(&["config", "user.name", "Test"]);
        run(&["config", "commit.gpgsign", "false"]);
        std::fs::write(path.join("README.md"), "test\n").unwrap();
        run(&["add", "README.md"]);
        run(&["commit", "-m", "initial"]);
        (temp, path)
    }
}
