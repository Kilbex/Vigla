//! Mission runtime: drives one mission end-to-end.
//!
//! Owns the lifecycle of a single mission from `Created` to a terminal
//! state, emitting mission events on a broadcast channel and waiting for
//! [`MissionRuntime::resolve`] / [`MissionRuntime::abort`] at
//! `CompletePendingMerge`. Two execution paths share this surface:
//!
//!   * the **supervised** path ([`MissionRuntime::start_supervised`] and
//!     its `_with`/`_with_memory` variants) — the live system: a real (or
//!     scripted) supervisor decomposes the mission and drives
//!     [`crate::mission_supervisor_run::run_supervisor_mission`], which
//!     dispatches workers over a `JoinSet`, runs the audit/arbiter loop,
//!     integrates on Accept, and beats the process-level endurance
//!     heartbeat (injected here via [`crate::endurance::shared_monitor`]);
//!   * the **mock** path ([`MissionRuntime::start`]) — a scripted Tokio
//!     timeline used by demos and tests.
//!
//! The module's surface (event shape, lifecycle methods) is the contract
//! both paths honor.

mod boundary;
mod event_bus;
mod mock;
mod support;

pub use boundary::WorkerRole;
pub use event_bus::MissionEventReceiver;
pub use mock::MockTimingConfig;

pub(crate) use event_bus::MissionEventBus;

use mock::run_mock_mission;
use support::{emit, finalize_failure};

use crate::memory::MemoryKernel;
use crate::mission::{MissionError, MissionSpec, MissionState, ResolveAction};
use crate::mission_event::{MergeResolution, MissionEventKind};
use crate::mission_workspace::{MissionGitError, MissionWorkspace};
use crate::{DispositionAction, MissionOutcomeState, Repository};
use std::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, Ordering};
use std::sync::Arc;
use std::time::Duration;
use thiserror::Error;
use tokio::sync::{mpsc, watch, Mutex};

#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum MissionRuntimeError {
    #[error("spec invalid: {0}")]
    Spec(MissionError),

    #[error("git: {0}")]
    Git(MissionGitError),

    #[error("io: {0}")]
    Io(String),

    #[error("persistence: {0}")]
    Persistence(String),

    #[error("resolve not allowed from state {state:?}")]
    ResolveNotAllowed { state: MissionState },

    #[error("mission extension is unavailable until supervisor re-entry is implemented")]
    ExtensionUnsupported,

    /// QC-2: `confirm_plan` / `regenerate_plan` may only be called
    /// while the mission is paused at `PendingPlanApproval`.
    #[error("plan decision not allowed from state {state:?}")]
    PlanDecisionNotAllowed { state: MissionState },

    #[error("plan decision targets generation {submitted}, but the current plan is generation {current}")]
    StalePlanDecision { submitted: u32, current: u32 },

    #[error("a plan decision was already submitted for generation {generation}")]
    PlanDecisionAlreadySubmitted { generation: u32 },

    #[error("runtime has already terminated")]
    AlreadyTerminated,
}

/// QC-2: how the user disposed of a proposed plan. Sent from the
/// runtime API (`confirm_plan` / `regenerate_plan` / `reject_plan`)
/// into the supervisor task via an mpsc channel.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PlanDecision {
    Confirm {
        generation: u32,
    },
    Regenerate {
        generation: u32,
        hint: Option<String>,
    },
    /// QC-3: user rejected the proposed plan; mission aborts.
    /// `reason` is the optional free-form text from the FE
    /// reject form.
    Reject {
        generation: u32,
        reason: Option<String>,
    },
}

impl PlanDecision {
    pub(crate) fn generation(&self) -> u32 {
        match self {
            Self::Confirm { generation }
            | Self::Regenerate { generation, .. }
            | Self::Reject { generation, .. } => *generation,
        }
    }
}

impl From<MissionGitError> for MissionRuntimeError {
    fn from(value: MissionGitError) -> Self {
        Self::Git(value)
    }
}

/// Cooperative cancellation: an interruptible sleep + a queryable
/// "are we cancelled" flag. Used to make abort responsive without
/// threading `select!` through every await point.
#[derive(Debug)]
pub struct CancelToken {
    cancelled: AtomicBool,
    changed: watch::Sender<bool>,
}

impl CancelToken {
    pub fn new() -> Arc<Self> {
        let (changed, _) = watch::channel(false);
        Arc::new(Self {
            cancelled: AtomicBool::new(false),
            changed,
        })
    }

    pub fn cancel(&self) {
        self.cancelled.store(true, Ordering::SeqCst);
        self.changed.send_replace(true);
    }

    pub fn is_cancelled(&self) -> bool {
        self.cancelled.load(Ordering::SeqCst)
    }

    /// Sleep `dur`, returning `true` if cancellation interrupted the
    /// sleep or was already set on entry.
    pub async fn sleep_or_cancel(&self, dur: Duration) -> bool {
        let mut changed = self.changed.subscribe();
        tokio::select! {
            _ = wait_for_cancelled(&mut changed) => true,
            _ = tokio::time::sleep(dur) => self.is_cancelled(),
        }
    }

    /// Await cancellation. Resolves as soon as `cancel()` has been
    /// called (or returns immediately if it already has). Use this in
    /// `tokio::select!` arms that need to preempt long-running awaits
    /// (e.g. waiting for a user plan decision) on abort.
    pub async fn notified(&self) {
        let mut changed = self.changed.subscribe();
        wait_for_cancelled(&mut changed).await;
    }
}

async fn wait_for_cancelled(changed: &mut watch::Receiver<bool>) {
    loop {
        if *changed.borrow_and_update() {
            return;
        }
        if changed.changed().await.is_err() {
            return;
        }
    }
}

/// Handle to a running mock mission. Construct with [`Self::start`];
/// observe via [`Self::subscribe`] and [`Self::state`]; finish with
/// [`Self::resolve`] (Merge/Discard) or [`Self::abort`].
#[derive(Debug, Clone)]
pub struct MissionRuntime {
    mission_id: String,
    spec: MissionSpec,
    workspace: MissionWorkspace,
    event_bus: MissionEventBus,
    state_tx: Arc<watch::Sender<MissionState>>,
    state_rx: watch::Receiver<MissionState>,
    cancel: Arc<CancelToken>,
    seq: Arc<AtomicU64>,
    /// Serializes `resolve` so two concurrent callers cannot both
    /// enter `final_merge` (which writes to a shared temp worktree
    /// path and `update-ref`s the user's target branch). The second
    /// caller waits, then sees state moved past
    /// `CompletePendingMerge` and is rejected with
    /// `ResolveNotAllowed`. On inner Err the lock releases and the
    /// state stays at `CompletePendingMerge`, so the user can retry.
    resolve_lock: Arc<Mutex<()>>,
    /// QC-2: `confirm_plan` / `regenerate_plan` IPC commands push
    /// `PlanDecision`s into the supervisor task here. The mock-backed
    /// `start` path also holds a sender but its task never reads from
    /// the receiver (mock missions don't pause for plan approval), so
    /// the channel is effectively a no-op for that path.
    plan_decision_tx: mpsc::Sender<PlanDecision>,
    /// Generation currently visible on the plan-review surface.
    plan_generation: Arc<AtomicU32>,
    /// Exactly one decision may claim each visible generation. It stays closed
    /// while a regenerated plan is being produced.
    plan_decision_open: Arc<AtomicBool>,
    /// Tier-2A: optional memory kernel installed by the controller.
    /// Workers get a curated memory bundle rendered into their
    /// worktree right after creation; mission accept/scrub fires a
    /// barrier into the kernel. When `None`, the mission runs
    /// exactly as before — memory failure must never block dispatch.
    memory: Option<Arc<MemoryKernel>>,
    /// Installed by the host before the runtime becomes visible. Tests and
    /// embedders may omit it, but production dispositions use it as a
    /// write-ahead journal and synchronous source of truth.
    disposition_store: Arc<Mutex<Option<DispositionStore>>>,
}

#[derive(Debug, Clone)]
struct DispositionStore {
    repository: Repository,
    repo_root: String,
}

impl MissionRuntime {
    pub async fn start(
        spec: MissionSpec,
        workspace: MissionWorkspace,
        config: MockTimingConfig,
    ) -> Result<Self, MissionRuntimeError> {
        Self::start_with_memory(spec, workspace, config, None, None).await
    }

    /// Same as [`Self::start`] but accepts an optional [`MemoryKernel`]
    /// that the mission lifecycle will use for bundle rendering before
    /// each worker dispatch and barrier emission on resolve. When
    /// `memory` is `None`, the mission behaves exactly as a pre-Tier-2A
    /// run — no memory writes, no barriers, no codex side effects.
    pub async fn start_with_memory(
        spec: MissionSpec,
        workspace: MissionWorkspace,
        config: MockTimingConfig,
        memory: Option<Arc<MemoryKernel>>,
        _skills: Option<Arc<crate::skills::SkillLibrary>>,
    ) -> Result<Self, MissionRuntimeError> {
        let spec = spec.normalized().map_err(MissionRuntimeError::Spec)?;

        // Front-load the I/O so callers see git failures synchronously
        // rather than as the first event.
        workspace.create_supervisor_branch(&spec.target_ref).await?;
        workspace.create_supervisor_worktree().await?;

        let mission_id = workspace.mission_id().to_string();
        let event_bus = MissionEventBus::new(256);
        let (state_tx_raw, state_rx) = watch::channel(MissionState::Created);
        let state_tx = Arc::new(state_tx_raw);
        let cancel = CancelToken::new();
        let seq = Arc::new(AtomicU64::new(0));
        // Mock missions never pause at PendingPlanApproval, so the
        // receiver here is never read. Capacity 4 mirrors the
        // supervised path's channel.
        let (plan_decision_tx, _plan_decision_rx) = mpsc::channel::<PlanDecision>(4);
        let plan_generation = Arc::new(AtomicU32::new(0));
        let plan_decision_open = Arc::new(AtomicBool::new(false));

        let task_state_tx = state_tx.clone();
        let task_event_bus = event_bus.clone();
        let task_cancel = cancel.clone();
        let task_seq = seq.clone();
        let task_ws = workspace.clone();
        let task_spec = spec.clone();
        let task_mid = mission_id.clone();
        let task_memory = memory.clone();

        // Spare clones for the error-finalization path so an Err
        // returned by `run_mock_mission` still drives the state
        // machine to Aborted instead of leaving it stuck (which would
        // hang `await_complete_or_terminal` and `resolve` forever).
        let err_state_tx = task_state_tx.clone();
        let err_event_bus = task_event_bus.clone();
        let err_seq = task_seq.clone();
        let err_mid = task_mid.clone();

        crate::spawn_supervised("mission_runtime::mock_mission", async move {
            if let Err(e) = run_mock_mission(
                task_mid,
                task_spec,
                task_ws,
                task_event_bus,
                task_state_tx,
                task_cancel,
                task_seq,
                config,
                task_memory,
            )
            .await
            {
                finalize_failure(&err_state_tx, &err_event_bus, &err_seq, &err_mid, e).await;
            }
        });

        Ok(Self {
            mission_id,
            spec,
            workspace,
            event_bus,
            state_tx,
            state_rx,
            cancel,
            seq,
            resolve_lock: Arc::new(Mutex::new(())),
            plan_decision_tx,
            plan_generation,
            plan_decision_open,
            memory,
            disposition_store: Arc::new(Mutex::new(None)),
        })
    }

    /// Start a mission whose supervisor is a real (or scripted)
    /// driver per [`SupervisorDriver`]. The worker backend is derived
    /// from `spec.worker_model` automatically — see
    /// [`crate::mission_supervisor_run::select_worker_backend`].
    /// The runtime's `abort`, `resolve`, `subscribe`, `state`, and
    /// `mission_id` behave identically to the mock-backed path; only
    /// the inner timeline differs.
    pub async fn start_supervised(
        spec: MissionSpec,
        workspace: MissionWorkspace,
        driver: crate::mission_supervisor_run::SupervisorDriver,
    ) -> Result<Self, MissionRuntimeError> {
        let worker_backend = crate::mission_supervisor_run::select_worker_backend(&spec);
        Self::start_supervised_with(spec, workspace, driver, worker_backend).await
    }

    /// Same as [`Self::start_supervised`] but takes the worker backend
    /// explicitly. Used by tests that want a Mock backend regardless
    /// of what `spec.worker_model` would otherwise select, and by the
    /// host IPC when it has already validated the model string.
    pub async fn start_supervised_with(
        spec: MissionSpec,
        workspace: MissionWorkspace,
        driver: crate::mission_supervisor_run::SupervisorDriver,
        worker_backend: crate::mission_supervisor_run::WorkerBackend,
    ) -> Result<Self, MissionRuntimeError> {
        Self::start_supervised_with_memory(spec, workspace, driver, worker_backend, None, None)
            .await
    }

    /// As [`Self::start_supervised_with`] plus an optional memory
    /// kernel. The supervised path renders curated memory into each
    /// worker worktree before dispatch, routes worker proposal
    /// intents through the kernel, and fires the resolve barrier
    /// (accept/scrub → `on_mission_barrier`).
    pub async fn start_supervised_with_memory(
        spec: MissionSpec,
        workspace: MissionWorkspace,
        driver: crate::mission_supervisor_run::SupervisorDriver,
        worker_backend: crate::mission_supervisor_run::WorkerBackend,
        memory: Option<Arc<MemoryKernel>>,
        skills: Option<Arc<crate::skills::SkillLibrary>>,
    ) -> Result<Self, MissionRuntimeError> {
        let spec = spec.normalized().map_err(MissionRuntimeError::Spec)?;
        workspace.create_supervisor_branch(&spec.target_ref).await?;
        workspace.create_supervisor_worktree().await?;

        let mission_id = workspace.mission_id().to_string();
        let event_bus = MissionEventBus::new(256);
        let (state_tx_raw, state_rx) = watch::channel(MissionState::Created);
        let state_tx = Arc::new(state_tx_raw);
        let cancel = CancelToken::new();
        let seq = Arc::new(AtomicU64::new(0));
        // QC-2: plan-decision channel. Capacity 4 absorbs a stray
        // double-click without blocking the IPC handler; the
        // supervisor task drains at most one decision per pause.
        let (plan_decision_tx, plan_decision_rx) = mpsc::channel::<PlanDecision>(4);
        let plan_generation = Arc::new(AtomicU32::new(0));
        let plan_decision_open = Arc::new(AtomicBool::new(false));

        let task_state_tx = state_tx.clone();
        let task_event_bus = event_bus.clone();
        let task_cancel = cancel.clone();
        let task_seq = seq.clone();
        let task_ws = workspace.clone();
        let task_spec = spec.clone();
        let task_mid = mission_id.clone();
        let task_memory = memory.clone();
        let task_skills = skills.clone();
        let task_plan_generation = plan_generation.clone();
        let task_plan_decision_open = plan_decision_open.clone();

        // See the matching block in `start` for why we keep clones for
        // the failure-finalization path.
        let err_state_tx = task_state_tx.clone();
        let err_event_bus = task_event_bus.clone();
        let err_seq = task_seq.clone();
        let err_mid = task_mid.clone();

        crate::spawn_supervised("mission_runtime::supervisor_mission", async move {
            if let Err(e) = crate::mission_supervisor_run::run_supervisor_mission(
                task_mid,
                task_spec,
                task_ws,
                task_event_bus,
                task_state_tx,
                task_cancel,
                task_seq,
                driver,
                worker_backend,
                plan_decision_rx,
                task_plan_generation,
                task_plan_decision_open,
                task_memory,
                task_skills,
                // U10 A6: inject the process-level endurance monitor the
                // host installed at startup. `None` in tests / when the
                // host installed none — missions then simply don't beat.
                crate::endurance::shared_monitor(),
            )
            .await
            {
                finalize_failure(&err_state_tx, &err_event_bus, &err_seq, &err_mid, e).await;
            }
        });

        Ok(Self {
            mission_id,
            spec,
            workspace,
            event_bus,
            state_tx,
            state_rx,
            cancel,
            seq,
            resolve_lock: Arc::new(Mutex::new(())),
            plan_decision_tx,
            plan_generation,
            plan_decision_open,
            memory,
            disposition_store: Arc::new(Mutex::new(None)),
        })
    }

    /// Install durable disposition persistence before exposing this runtime to
    /// callers. The canonical repository root comes from the workspace, never
    /// from mutable process state or a later UI argument.
    pub async fn install_disposition_store(&self, repository: Repository) {
        *self.disposition_store.lock().await = Some(DispositionStore {
            repository,
            repo_root: self.workspace.repo_root().to_string_lossy().into_owned(),
        });
    }

    async fn current_disposition_store(&self) -> Option<DispositionStore> {
        self.disposition_store.lock().await.clone()
    }

    async fn persist_terminal_outcome(
        &self,
        store: Option<&DispositionStore>,
        state: MissionOutcomeState,
        updated_at: &str,
    ) -> Result<(), MissionRuntimeError> {
        let Some(store) = store else {
            return Ok(());
        };
        store
            .repository
            .record_mission_outcome(
                &self.mission_id,
                &store.repo_root,
                &self.spec.target_ref,
                state,
                updated_at,
            )
            .await
            .map_err(|error| MissionRuntimeError::Persistence(error.to_string()))
    }

    pub fn mission_id(&self) -> &str {
        &self.mission_id
    }

    pub fn subscribe(&self) -> MissionEventReceiver {
        self.event_bus.subscribe()
    }

    pub fn state(&self) -> MissionState {
        // `MissionState` is no longer `Copy` (S5 added a payload-bearing
        // `Paused` variant); clone out of the watch::Ref guard.
        self.state_rx.borrow().clone()
    }

    /// Wait until the mission reaches `CompletePendingMerge` or any
    /// terminal state. Returns the observed state.
    pub async fn await_complete_or_terminal(&self) -> MissionState {
        let mut rx = self.state_rx.clone();
        loop {
            let s = rx.borrow_and_update().clone();
            if matches!(
                s,
                MissionState::CompletePendingMerge
                    // Arbiter escalations park the mission at
                    // Attention waiting for the user's resolve choice;
                    // unblock awaiters here too so they can call
                    // `resolve()` from the Attention pause.
                    | MissionState::Attention
                    | MissionState::Merged
                    | MissionState::Discarded
                    | MissionState::Aborted
            ) {
                return s;
            }
            if rx.changed().await.is_err() {
                return rx.borrow().clone();
            }
        }
    }

    /// Stop the mission immediately. Worker tasks unwind at their
    /// next yield point; the supervisor task emits `mission.aborted`
    /// and sets state to `Aborted`. Branches and worktrees are
    /// preserved per proposal v2 §3.5.
    pub async fn abort(&self) -> Result<(), MissionRuntimeError> {
        // Serialize with resolve() so a concurrent abort + resolve can't
        // both write competing terminal states to the watch channel (e.g.
        // Aborted landing after a merge already completed). resolve() holds
        // this lock across its git ops + state write, so taking it here
        // before sampling state and issuing cancel removes the race (F-8).
        // The Aborted state is written by the cancelled task, which never
        // takes resolve_lock, so holding it through the wait can't deadlock.
        let _resolve_guard = self.resolve_lock.lock().await;
        let current = self.state();
        if matches!(
            current,
            MissionState::Merged | MissionState::Discarded | MissionState::Aborted
        ) {
            return Err(MissionRuntimeError::AlreadyTerminated);
        }
        self.cancel.cancel();
        // These are parked decision states: the execution future has already
        // returned, so no background task remains to observe cancellation.
        // Abort owns the terminal transition directly in this case.
        if matches!(
            current,
            MissionState::CompletePendingMerge | MissionState::Attention
        ) {
            let store = self.current_disposition_store().await;
            let now = crate::ids::rfc3339_now();
            self.persist_terminal_outcome(store.as_ref(), MissionOutcomeState::Aborted, &now)
                .await?;
            self.state_tx.send(MissionState::Aborted).ok();
            emit(
                &self.event_bus,
                &self.mission_id,
                &self.seq,
                MissionEventKind::Aborted {
                    reason: "user abort".into(),
                },
            );
            return Ok(());
        }
        let mut rx = self.state_rx.clone();
        loop {
            let s = rx.borrow_and_update().clone();
            if matches!(s, MissionState::Aborted) {
                let store = self.current_disposition_store().await;
                let now = crate::ids::rfc3339_now();
                return self
                    .persist_terminal_outcome(store.as_ref(), MissionOutcomeState::Aborted, &now)
                    .await;
            }
            if matches!(s, MissionState::Merged | MissionState::Discarded) {
                return Ok(());
            }
            if rx.changed().await.is_err() {
                return Ok(());
            }
        }
    }

    /// QC-2: confirm the supervisor's proposed plan. Only valid when
    /// the mission is paused at `PendingPlanApproval`; signals the
    /// supervisor task to proceed to the per-task loop.
    pub async fn confirm_plan(&self, generation: u32) -> Result<(), MissionRuntimeError> {
        self.submit_plan_decision(PlanDecision::Confirm { generation })
            .await
    }

    /// QC-2: ask the supervisor for a new decomposition. Only valid
    /// when the mission is paused at `PendingPlanApproval`. `hint` is
    /// the user's optional feedback to append to the next decompose
    /// prompt; `None` means "try again without feedback."
    pub async fn regenerate_plan(
        &self,
        generation: u32,
        hint: Option<String>,
    ) -> Result<(), MissionRuntimeError> {
        self.submit_plan_decision(PlanDecision::Regenerate { generation, hint })
            .await
    }

    /// QC-3: reject the proposed plan and abort the mission. Only
    /// valid while the mission is paused at `PendingPlanApproval`;
    /// signals the supervisor loop via the existing
    /// `plan_decision_tx` channel. The loop emits
    /// `MissionEventKind::PlanRejected` followed by `Aborted`.
    pub async fn reject_plan(
        &self,
        generation: u32,
        reason: Option<String>,
    ) -> Result<(), MissionRuntimeError> {
        self.submit_plan_decision(PlanDecision::Reject { generation, reason })
            .await
    }

    async fn submit_plan_decision(
        &self,
        decision: PlanDecision,
    ) -> Result<(), MissionRuntimeError> {
        let s = self.state();
        if s != MissionState::PendingPlanApproval {
            return Err(MissionRuntimeError::PlanDecisionNotAllowed { state: s });
        }
        let submitted = decision.generation();
        let current = self.plan_generation.load(Ordering::Acquire);
        if submitted != current {
            return Err(MissionRuntimeError::StalePlanDecision { submitted, current });
        }
        self.plan_decision_open
            .compare_exchange(true, false, Ordering::AcqRel, Ordering::Acquire)
            .map_err(|_| MissionRuntimeError::PlanDecisionAlreadySubmitted {
                generation: current,
            })?;
        if self.plan_decision_tx.send(decision).await.is_err() {
            self.plan_decision_open.store(true, Ordering::Release);
            return Err(MissionRuntimeError::AlreadyTerminated);
        }
        Ok(())
    }

    /// Apply the user's final disposition. Blocks until the mission
    /// reaches `CompletePendingMerge`; refuses if the mission has
    /// already terminated.
    pub async fn resolve(&self, action: ResolveAction) -> Result<(), MissionRuntimeError> {
        let should_merge = match action {
            ResolveAction::Merge => true,
            ResolveAction::Discard => false,
            // The wire variant is retained for replay compatibility, but
            // reporting success without scheduling another supervisor turn
            // strands the mission in Executing. Fail before taking the lock or
            // mutating state until a real re-entry path exists.
            ResolveAction::Extend { .. } => return Err(MissionRuntimeError::ExtensionUnsupported),
        };

        // Serialize concurrent resolves so two callers cannot both
        // enter `final_merge`. The second caller waits here, then
        // sees state has moved past `CompletePendingMerge` and exits
        // via the terminal-state branch below.
        let _resolve_guard = self.resolve_lock.lock().await;
        let mut rx = self.state_rx.clone();
        loop {
            let s = rx.borrow_and_update().clone();
            match s {
                // `Attention` is a valid resolve origin so an
                // arbiter-escalated pause can be cleared by merging the
                // accepted partial work or discarding the mission.
                MissionState::CompletePendingMerge | MissionState::Attention => break,
                MissionState::Aborted | MissionState::Merged | MissionState::Discarded => {
                    return Err(MissionRuntimeError::ResolveNotAllowed { state: s });
                }
                _ => {
                    if rx.changed().await.is_err() {
                        return Err(MissionRuntimeError::AlreadyTerminated);
                    }
                }
            }
        }

        let store = self.current_disposition_store().await;
        let disposition_ts = crate::ids::rfc3339_now();
        if let Some(store) = &store {
            store
                .repository
                .record_disposition_intent(
                    &self.mission_id,
                    &store.repo_root,
                    &self.spec.target_ref,
                    if should_merge {
                        DispositionAction::Merge
                    } else {
                        DispositionAction::Discard
                    },
                    &disposition_ts,
                )
                .await
                .map_err(|error| MissionRuntimeError::Persistence(error.to_string()))?;
        }

        let (next_state, resolution, barrier) = if should_merge {
            if !self
                .workspace
                .final_merge_is_applied(&self.spec.target_ref)
                .await?
            {
                self.workspace.final_merge(&self.spec.target_ref).await?;
            }
            // The outcome must be durable before success is observable. A
            // retry after a DB failure recognizes the final anchors above and
            // only retries persistence instead of merging twice.
            self.persist_terminal_outcome(
                store.as_ref(),
                MissionOutcomeState::Merged,
                &disposition_ts,
            )
            .await?;
            let cleanup = self.workspace.discard().await;
            if let Err(error) = &cleanup {
                tracing::error!(
                    "vigla: merged mission {} is durable but cleanup remains pending: {error}",
                    self.mission_id
                );
            }
            if cleanup.is_ok() {
                if let Some(store) = &store {
                    if let Err(error) = store
                        .repository
                        .clear_disposition_intent(&self.mission_id)
                        .await
                    {
                        tracing::warn!(
                            "vigla: merged mission {} left a reconciliation journal row: {error}",
                            self.mission_id
                        );
                    }
                }
            }
            (
                MissionState::Merged,
                MergeResolution::Merged,
                event_schema::memory::BarrierKind::Accept,
            )
        } else {
            self.workspace.discard().await?;
            self.persist_terminal_outcome(
                store.as_ref(),
                MissionOutcomeState::Discarded,
                &disposition_ts,
            )
            .await?;
            if let Some(store) = &store {
                if let Err(error) = store
                    .repository
                    .clear_disposition_intent(&self.mission_id)
                    .await
                {
                    tracing::warn!(
                        "vigla: discarded mission {} left a reconciliation journal row: {error}",
                        self.mission_id
                    );
                }
            }
            (
                MissionState::Discarded,
                MergeResolution::Discarded,
                event_schema::memory::BarrierKind::Scrub,
            )
        };

        self.state_tx.send(next_state).ok();
        emit(
            &self.event_bus,
            &self.mission_id,
            &self.seq,
            MissionEventKind::MergeResolved { resolution },
        );

        // Tier-2A: memory barrier after the state transition. Fail-soft
        // — any error from the kernel is logged but never propagated;
        // the mission has already committed to the resolution and the
        // user must not be blocked by memory consolidation.
        if let Some(kernel) = &self.memory {
            if let Err(e) = kernel.on_mission_barrier(&self.mission_id, barrier).await {
                tracing::error!(
                    "vigla: memory barrier failed for mission {} ({:?}): {e}",
                    self.mission_id,
                    barrier
                );
            }
        }

        Ok(())
    }
}

#[cfg(test)]
mod tests;
