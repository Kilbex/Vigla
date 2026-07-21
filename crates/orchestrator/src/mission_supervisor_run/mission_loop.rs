use super::driver::{SupervisorDriver, SupervisorTurnResult};
use super::prompts::{
    format_complete_prompt, format_decompose_prompt, format_decompose_prompt_with_hint,
};
use super::run_task::{run_task, TaskOutcome, TaskRunCtx};
use super::support::{abort, emit, first_intent};
use super::worker_pass::WorkerBackend;
use crate::endurance::{BeatStatus, Clock, EnduranceMonitor, SystemClock};
use crate::memory::MemoryKernel;
use crate::mission::{MissionSpec, MissionState};
use crate::mission_event::{MissionEventKind, TaskDescriptor};
use crate::mission_runtime::{CancelToken, MissionEventBus, MissionRuntimeError, PlanDecision};
use crate::mission_workspace::MissionWorkspace;
use std::sync::atomic::{AtomicU32, AtomicU64, Ordering};
use std::sync::Arc;
use std::time::Duration;
use supervisor_adapter::SupervisorIntent;
use tokio::sync::{mpsc, watch, Mutex};
use tokio::task::JoinSet;

/// Process-level endurance monitor handle, injected into the mission
/// loop so it can emit liveness heartbeats (U10 A4). A `std::sync::Mutex`
/// — not tokio's — because [`EnduranceMonitor::beat`] is a short
/// *synchronous* atomic file write; the lock is taken and released
/// without ever crossing an `.await`. Optional: the host wires the real
/// process-level monitor; missions that run without one (mock/test
/// missions, or before the host wires it) simply don't beat.
pub(crate) type EnduranceHandle = Arc<std::sync::Mutex<EnduranceMonitor<SystemClock>>>;

/// Run a best-effort bookkeeping write against the wired monitor. A
/// beat/fault/recovery write must never turn a healthy mission into a
/// failure, so a poisoned lock or IO error is logged and swallowed. The
/// critical section is synchronous — never `.await` while holding the
/// lock. No-op when no monitor is wired.
fn with_monitor_best_effort(
    endurance: &Option<EnduranceHandle>,
    what: &'static str,
    op: impl FnOnce(&mut EnduranceMonitor) -> Result<(), crate::endurance::EnduranceError>,
) {
    if let Some(handle) = endurance {
        match handle.lock() {
            Ok(mut monitor) => {
                if let Err(e) = op(&mut monitor) {
                    tracing::warn!(
                        target: "vigla::endurance",
                        error = %e,
                        what,
                        "endurance write failed; mission continues"
                    );
                }
            }
            Err(_poisoned) => {
                tracing::warn!(
                    target: "vigla::endurance",
                    what,
                    "endurance monitor mutex poisoned; skipping write"
                );
            }
        }
    }
}

/// Emit one heartbeat beat, best-effort.
fn beat_best_effort(endurance: &Option<EnduranceHandle>, status: BeatStatus) {
    with_monitor_best_effort(endurance, "beat", |m| m.beat(status));
}

/// Book a worker fault (`worker_panic`, `task_error`, `recovery`),
/// best-effort.
fn note_fault_best_effort(endurance: &Option<EnduranceHandle>, kind: &str) {
    with_monitor_best_effort(endurance, "note_fault", |m| m.note_fault(kind));
}

/// Book a fault recovery, best-effort.
fn note_recovery_best_effort(endurance: &Option<EnduranceHandle>, kind: &str) {
    with_monitor_best_effort(endurance, "note_recovery", |m| m.note_recovery(kind));
}

/// Cadence class for the background heartbeat ticker.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Tick {
    /// Work in flight → beat often (finer stall resolution).
    Active,
    /// Idle or quota-paused → back off to save CPU/quota, while staying
    /// well under the crash threshold so an idle fleet is never misread
    /// as crashed.
    Idle,
}

impl Tick {
    fn duration(self, active: Duration, idle: Duration) -> Duration {
        match self {
            Tick::Active => active,
            Tick::Idle => idle,
        }
    }
}

/// Active/idle beat intervals. Both MUST stay under
/// `EnduranceConfig::crash_threshold_ms` (90 s) — otherwise an idle or
/// quota-paused fleet would be misclassified as crashed. The
/// `cadences_stay_under_crash_threshold` test guards this invariant.
const HEARTBEAT_TICK_ACTIVE: Duration = Duration::from_secs(30);
const HEARTBEAT_TICK_IDLE: Duration = Duration::from_secs(60);

/// Pure policy for one ticker cycle. Given the live in-flight worker count
/// and whether the fleet is currently quota-paused, decide the beat to
/// emit and the cadence class.
///
/// This is the sleep-wake adaptation: a quota pause or an empty pool
/// reports `workers_active: 0` (so the monitor reads `Idle`, never a false
/// `Stalled`) and backs the cadence off; otherwise it reports the
/// in-flight count with `progressed: false`, so a genuinely wedged worker
/// — making no real forward progress — accrues toward `Stalled`. Real
/// progress still arrives via the discrete completion beats, which keep an
/// actively-working mission `Healthy`.
fn ticker_plan(inflight: u32, quota_paused: bool) -> (BeatStatus, Tick) {
    if quota_paused || inflight == 0 {
        (
            BeatStatus {
                phase: Some(if quota_paused { "paused:quota" } else { "idle" }.into()),
                workers_active: Some(0),
                progressed: false,
                ..Default::default()
            },
            Tick::Idle,
        )
    } else {
        (
            BeatStatus {
                phase: Some("executing".into()),
                workers_active: Some(inflight),
                progressed: false,
                ..Default::default()
            },
            Tick::Active,
        )
    }
}

/// True if any vendor is *currently* quota-exhausted (reset still in the
/// future). The tracker exposes no single "paused?" query, so fold over
/// the vendors that have tracked state; `is_exhausted` filters out stale
/// entries whose reset has already elapsed.
async fn fleet_quota_paused(
    tracker: &crate::recovery::VendorQuotaTracker,
    now_unix_ms: u64,
) -> bool {
    for vendor in tracker.tracked_vendors().await {
        if tracker.is_exhausted(vendor, now_unix_ms).await {
            return true;
        }
    }
    false
}

/// Spawn the background heartbeat ticker (U10 adaptation slice). Emits a
/// `progressed: false` liveness beat every cadence, adapting between the
/// `active`/`idle` intervals via [`ticker_plan`]. This is what makes
/// `Stalled` observable on the live loop — the discrete beats alone only
/// fire on progress, so a wedged worker would otherwise just go stale and
/// read as `Crashed`. The cadences are injected so tests can drive it with
/// tiny intervals.
fn spawn_heartbeat_ticker(
    handle: EnduranceHandle,
    inflight: Arc<AtomicU32>,
    quota_tracker: Arc<crate::recovery::VendorQuotaTracker>,
    active: Duration,
    idle: Duration,
) -> tokio::task::JoinHandle<()> {
    tokio::spawn(async move {
        let mut next = active;
        loop {
            tokio::time::sleep(next).await;
            let now = SystemClock.now_ms();
            let paused = fleet_quota_paused(&quota_tracker, now).await;
            let (status, tick) = ticker_plan(inflight.load(Ordering::Relaxed), paused);
            beat_best_effort(&Some(handle.clone()), status);
            next = tick.duration(active, idle);
        }
    })
}

/// Aborts a background task when the mission future unwinds on any return
/// path, mirroring `QuotaWakeupHandle`'s drop-aborts-the-task idiom.
struct AbortOnDrop(tokio::task::JoinHandle<()>);
impl Drop for AbortOnDrop {
    fn drop(&mut self) {
        self.0.abort();
    }
}

/// Resets the shared in-flight worker count to zero when a dispatcher
/// returns on ANY path — normal completion or an early Escalated / Aborted
/// / Err drain. Without this the heartbeat ticker could briefly read a
/// stale `inflight > 0` for an already-drained dispatcher and emit a
/// `workers_active > 0` beat for a fleet that is actually idle.
struct ZeroOnDrop<'a>(&'a AtomicU32);
impl Drop for ZeroOnDrop<'_> {
    fn drop(&mut self) {
        self.0.store(0, Ordering::Relaxed);
    }
}

/// Drive a mission through the supervisor:
///
/// 1. **Decompose turn** — supervisor proposes the task list. The
///    decomposition is validated as a DAG (cycles / orphans / dup
///    indices rejected). Optionally pauses for user confirmation
///    when `confirm_plan` is on.
/// 2. **Per-task dispatch** — for each task in topological order,
///    call [`run_task`] which spawns a worker, runs the audit /
///    arbiter loop, and integrates on Accept. D1 (S7 T10) keeps
///    dispatch sequential; D2 (S7 T11) lifts the loop into a
///    `JoinSet` driven by [`crate::task_graph::Scheduler`].
/// 3. **Completion turn** — supervisor declares the mission done;
///    a `Completed` event lands with the summary.
///
/// The supervisor session is held under a mutex so per-pass review
/// turns (S6 rework-kind selection) inside parallel `run_task`
/// futures serialise correctly — a single supervisor session can
/// only handle one in-flight turn at a time.
#[allow(clippy::too_many_arguments)]
pub(crate) async fn run_supervisor_mission(
    mission_id: String,
    spec: MissionSpec,
    workspace: MissionWorkspace,
    event_bus: MissionEventBus,
    state_tx: Arc<watch::Sender<MissionState>>,
    cancel: Arc<CancelToken>,
    seq: Arc<AtomicU64>,
    mut driver: SupervisorDriver,
    worker_backend: WorkerBackend,
    mut plan_decision_rx: mpsc::Receiver<PlanDecision>,
    visible_plan_generation: Arc<AtomicU32>,
    plan_decision_open: Arc<std::sync::atomic::AtomicBool>,
    memory: Option<Arc<MemoryKernel>>,
    skills: Option<Arc<crate::skills::SkillLibrary>>,
    endurance: Option<EnduranceHandle>,
) -> Result<(), MissionRuntimeError> {
    emit(
        &event_bus,
        &mission_id,
        &seq,
        MissionEventKind::Created { spec: spec.clone() },
    );
    state_tx.send(MissionState::Executing).ok();
    emit(
        &event_bus,
        &mission_id,
        &seq,
        MissionEventKind::ExecutionStarted,
    );

    // U10 A4: first liveness beat — the orchestrator is now executing
    // this mission. No-op unless the host wired a process-level monitor.
    beat_best_effort(
        &endurance,
        BeatStatus {
            phase: Some("executing".into()),
            mission_id: Some(Some(mission_id.clone())),
            workers_active: Some(0),
            events_total: Some(seq.load(Ordering::Relaxed)),
            progressed: true,
        },
    );

    if cancel.is_cancelled() {
        return abort(&event_bus, &mission_id, &seq, &state_tx, "user abort").await;
    }

    // ── Turn 1: ask supervisor to decompose ────────────────────────
    let mission_cwd = workspace.supervisor_worktree_path();
    let mut session_id: Option<String> = None;
    let mut plan_generation: u32 = 0;
    let mut latest_hint: Option<String> = None;
    let confirm_plan_on = matches!(spec.confirm_plan, Some(true));

    // The decomposition loop. Normally runs once; if `confirm_plan`
    // is on and the user requests a regenerate, we re-run with the
    // user's hint appended to the prompt.
    let tasks = loop {
        let prompt = match latest_hint.as_deref() {
            None => format_decompose_prompt(&spec),
            Some(hint) => format_decompose_prompt_with_hint(&spec, hint, plan_generation),
        };
        let SupervisorTurnResult {
            outputs,
            session_id: turn_sid,
        } = driver
            .run_turn_cancellable(&prompt, session_id.as_deref(), &mission_cwd, Some(&cancel))
            .await;
        if turn_sid.is_some() {
            session_id = turn_sid;
        }
        // A supervisor decomposition turn completed — book it as progress
        // so a long planning phase still reads as a live, advancing loop.
        beat_best_effort(
            &endurance,
            BeatStatus {
                phase: Some("planning".into()),
                events_total: Some(seq.load(Ordering::Relaxed)),
                progressed: true,
                ..Default::default()
            },
        );
        // Supervisor's decompose-turn reasoning is intentionally not
        // emitted as a worker progress event (no real `worker_id`
        // exists yet, and the frontend store keys progress by
        // worker). The decomposition itself is the user-visible
        // artifact.

        let (mut proposed, plan_overview, plan_tech_stack, plan_envelope_fit) =
            match first_intent(&outputs) {
                // S7 T15: route empty decompositions through validate()
                // so the supervisor flow emits a structured
                // DecompositionRejected event (EmptyDecomposition
                // variant) instead of a vague Aborted("supervisor did
                // not decompose…") fallback. The mission still
                // terminates; the inbox card just gains a typed reason.
                Some(SupervisorIntent::Decompose {
                    tasks,
                    overview,
                    tech_stack,
                    envelope_fit,
                }) => {
                    // Cap at 6 tasks. If we truncate, a kept task that
                    // depends on a dropped index would otherwise reach
                    // validate() as OrphanDependency, aborting the
                    // mission with a misleading "supervisor's fault"
                    // reason for a limit the orchestrator imposed.
                    // Strip ONLY those truncation-induced refs; refs to
                    // indices that were never present (e.g.
                    // depends_on=[99] for a 2-task decomposition)
                    // remain so validate() still rejects genuine
                    // supervisor bugs.
                    const MAX_TASKS: u32 = 6;
                    let kept = (tasks.len() as u32).min(MAX_TASKS);
                    let truncated = tasks.len() as u32 > kept;
                    let proposed_tasks = tasks
                        .iter()
                        .enumerate()
                        .take(MAX_TASKS as usize)
                        .map(|(i, t)| TaskDescriptor {
                            index: i as u32,
                            title: t.title.clone(),
                            description: t.description.clone(),
                            depends_on: if truncated {
                                t.depends_on
                                    .iter()
                                    .copied()
                                    .filter(|&d| !(d >= kept && (d as usize) < tasks.len()))
                                    .collect()
                            } else {
                                t.depends_on.clone()
                            },
                            scope_paths: t.scope_paths.clone(),
                            ..Default::default()
                        })
                        .collect::<Vec<_>>();

                    // QC-3: convert adapter-local envelope/tech-stack
                    // types to the orchestrator wire shape. Field-for-
                    // field maps; the adapter shape stays decoupled
                    // from the orchestrator so the adapter crate
                    // doesn't need a path-dep on vigla-orchestrator.
                    // `first_intent` returns borrowed references, so
                    // we clone before mapping into owned wire types.
                    let tech_stack_oc = tech_stack.as_ref().map(|rows| {
                        rows.iter()
                            .map(|t| crate::mission_event::TechChoice {
                                layer: t.layer.clone(),
                                choice: t.choice.clone(),
                                rationale: t.rationale.clone(),
                                is_new: t.is_new,
                            })
                            .collect::<Vec<_>>()
                    });
                    let envelope_fit_oc = envelope_fit
                        .as_ref()
                        .map(|ef| adapter_envelope_to_orchestrator(ef.clone()));

                    (
                        proposed_tasks,
                        overview.clone(),
                        tech_stack_oc,
                        envelope_fit_oc,
                    )
                }
                Some(_) => {
                    let reason = supervisor_bad_decompose_reason(
                        &outputs,
                        "supervisor did not decompose on the first turn",
                    );
                    return abort(&event_bus, &mission_id, &seq, &state_tx, &reason).await;
                }
                None => {
                    let reason = supervisor_bad_decompose_reason(
                        &outputs,
                        "supervisor turn produced no decomposition",
                    );
                    return abort(&event_bus, &mission_id, &seq, &state_tx, &reason).await;
                }
            };

        for task in &mut proposed {
            task.scope_paths = match crate::mission::normalize_scope_paths(&task.scope_paths) {
                Ok(paths) => paths,
                Err(error) => {
                    emit(
                        &event_bus,
                        &mission_id,
                        &seq,
                        MissionEventKind::DecompositionRejected {
                            reason: error.to_string(),
                        },
                    );
                    return abort(
                        &event_bus,
                        &mission_id,
                        &seq,
                        &state_tx,
                        "decomposition contained an invalid scope path",
                    )
                    .await;
                }
            };
        }

        // ── S7: validate decomposition as a DAG before emitting ────
        match crate::task_graph::validate(&proposed) {
            Ok(_dag) => {
                emit(
                    &event_bus,
                    &mission_id,
                    &seq,
                    MissionEventKind::Decomposition {
                        tasks: proposed.clone(),
                    },
                );
            }
            Err(err) => {
                let reason = serde_json::to_string(&err)
                    .unwrap_or_else(|_| format!("decomposition rejected: {err:?}"));
                emit(
                    &event_bus,
                    &mission_id,
                    &seq,
                    MissionEventKind::DecompositionRejected { reason },
                );
                return abort(
                    &event_bus,
                    &mission_id,
                    &seq,
                    &state_tx,
                    "decomposition failed DAG validation",
                )
                .await;
            }
        }

        // QC-3: envelope-fit gate. The orchestrator pauses for plan
        // review when EITHER the user asked for Review mode
        // (`confirm_plan == Some(true)`) OR the supervisor's
        // envelope_fit names any bound as `Exceeds`. A legacy adapter
        // (envelope_fit == None) short-circuits to QC-2 semantics.
        let envelope_tripped = plan_envelope_fit
            .as_ref()
            .and_then(crate::arbiter::check_plan_envelope)
            .is_some();

        // Default (autonomous) path: no pause, proceed straight to
        // the per-task loop with the freshly-proposed plan. Legacy
        // adapters (no envelope_fit) and within-envelope decomposes
        // in Direct mode both fall through here.
        if !confirm_plan_on && !envelope_tripped {
            break proposed;
        }

        // ── QC-2/QC-3: pause for user plan approval ──────────────
        // Emit the rich PlanProposed payload (overview, tech_stack,
        // envelope_fit) so the FE's MissionPlanPreview can render
        // the mind map and envelope panel regardless of which gate
        // tripped.
        emit(
            &event_bus,
            &mission_id,
            &seq,
            MissionEventKind::PlanProposed {
                tasks: proposed.clone(),
                generation: plan_generation,
                overview: plan_overview.clone(),
                tech_stack: plan_tech_stack.clone(),
                envelope_fit: plan_envelope_fit.clone(),
            },
        );
        visible_plan_generation.store(plan_generation, Ordering::Release);
        state_tx.send(MissionState::PendingPlanApproval).ok();
        plan_decision_open.store(true, Ordering::Release);

        // Wait for the user to confirm or request a regeneration.
        // Cancel is always preemptive.
        let decision = loop {
            let decision = tokio::select! {
                biased;
                _ = cancel.notified() => {
                    return abort(&event_bus, &mission_id, &seq, &state_tx, "user abort").await;
                }
                decision = plan_decision_rx.recv() => {
                    match decision {
                        Some(d) => d,
                        // Sender dropped — runtime is going away. Treat
                        // like a user abort so we unwind cleanly.
                        None => {
                            return abort(
                                &event_bus,
                                &mission_id,
                                &seq,
                                &state_tx,
                                "plan-decision channel closed",
                            )
                            .await;
                        }
                    }
                }
            };
            if decision.generation() == plan_generation {
                break decision;
            }
        };

        match decision {
            PlanDecision::Confirm { .. } => {
                emit(
                    &event_bus,
                    &mission_id,
                    &seq,
                    MissionEventKind::PlanConfirmed {
                        generation: plan_generation,
                    },
                );
                state_tx.send(MissionState::Executing).ok();
                break proposed;
            }
            PlanDecision::Regenerate { hint, .. } => {
                emit(
                    &event_bus,
                    &mission_id,
                    &seq,
                    MissionEventKind::PlanRegenerationRequested {
                        hint: hint.clone(),
                        prior_generation: plan_generation,
                    },
                );
                state_tx.send(MissionState::Executing).ok();
                plan_generation = plan_generation.saturating_add(1);
                latest_hint = hint;
                // Loop back: a new decompose turn runs with the hint.
            }
            PlanDecision::Reject { reason, .. } => {
                emit(
                    &event_bus,
                    &mission_id,
                    &seq,
                    MissionEventKind::PlanRejected {
                        generation: plan_generation,
                        reason: reason.clone(),
                    },
                );
                let abort_reason = match reason.as_deref() {
                    Some(r) if !r.is_empty() => {
                        format!("plan_rejected: {r}")
                    }
                    _ => "plan_rejected".to_string(),
                };
                return abort(&event_bus, &mission_id, &seq, &state_tx, &abort_reason).await;
            }
        }
    };

    // ── Per-task dispatch ────────────────────────────────────────
    //
    // D2 (S7 T11) replaces the sequential for-loop from D1 with a
    // parallel `JoinSet` driven by [`crate::task_graph::Scheduler`]
    // over the validated DAG. Up to
    // `ArbiterPolicy::max_parallel_workers` tasks run concurrently.
    //
    // Shared mutable state lives in `Arc<Mutex<_>>`:
    //   * `driver` / `session_id`  — supervisor session
    //   * `integration_lock`       — git integration boundary
    //     (held by run_task during integrate; integration phase is
    //     serialised even though worker passes ran in parallel)
    //   * `attempts_used_for_mission` — shared rework counter
    let arbiter_policy = crate::arbiter::ArbiterPolicy::default();
    let recovery_policy = crate::recovery::policy::RecoveryPolicy::default();
    let quota_tracker = event_bus.quota_tracker.clone();
    // Spawn the wake-up task for this mission. The handle owns its
    // tokio task; dropping it (when `task_ctx` and all `run_task`
    // clones go away at mission end) aborts the task. 1 s poll is
    // the same cadence used in the failure-recovery e2e tests and
    // is well under any vendor's reset granularity.
    let quota_wakeup = Arc::new(crate::recovery::spawn_quota_wakeup_task(
        Arc::clone(&quota_tracker),
        std::time::Duration::from_secs(1),
    ));
    // U10 adaptation: shared in-flight worker count + the background
    // heartbeat ticker. The ticker emits liveness beats between the
    // discrete progress beats, so a wedged worker becomes observable as
    // Stalled while an idle/quota-paused fleet stays Idle. Spawned only
    // when a monitor is wired; the drop guard aborts it on any mission
    // exit (mirrors the quota-wakeup handle's lifetime).
    //
    // The ticker intentionally stops between missions: the process is
    // silent when no mission runs, so a host launch after a >90s idle/
    // closed gap resumes as an "unclean restart" (one fault injected AND
    // recovered, so the all-day gate's unrecovered count stays 0;
    // `max_beat_gap_ms` is NOT inflated because `launch` rebases
    // `last_beat_at_ms`). Don't "fix" this by keeping the ticker alive
    // across missions — that would mask genuine crashes. The proper
    // refinement is a host-level idle beat + a clean-shutdown marker.
    let inflight = Arc::new(AtomicU32::new(0));
    let _heartbeat_ticker = endurance.as_ref().map(|h| {
        AbortOnDrop(spawn_heartbeat_ticker(
            h.clone(),
            Arc::clone(&inflight),
            Arc::clone(&quota_tracker),
            HEARTBEAT_TICK_ACTIVE,
            HEARTBEAT_TICK_IDLE,
        ))
    });
    let integration_lock = Arc::new(Mutex::new(()));
    let attempts_used_for_mission = Arc::new(Mutex::new(0u8));
    let driver = Arc::new(Mutex::new(driver));
    let session_id = Arc::new(Mutex::new(session_id));
    let max_parallel = arbiter_policy.max_parallel_workers.max(1) as usize;

    let task_ctx = TaskRunCtx {
        mission_id: mission_id.clone(),
        spec: spec.clone(),
        workspace: workspace.clone(),
        event_bus: event_bus.clone(),
        state_tx: Arc::clone(&state_tx),
        cancel: Arc::clone(&cancel),
        seq: Arc::clone(&seq),
        worker_backend,
        memory: memory.clone(),
        skills: skills.clone(),
        arbiter_policy,
        recovery_policy,
        quota_tracker,
        tasks_total: tasks.len(),
        driver: Arc::clone(&driver),
        session_id: Arc::clone(&session_id),
        mission_cwd: mission_cwd.clone(),
        integration_lock,
        attempts_used_for_mission,
        quota_wakeup,
    };

    // Monotonic integration index: handed out at dispatch order,
    // not by `task.index`, because completion order is
    // non-deterministic under parallelism. Each integration gets a
    // unique pre-merge snapshot tag from this counter. The counter
    // persists across the main DAG and the Split-drain rounds so
    // every integration in the mission gets a unique tag suffix.
    let mut next_integration_index: u32 = 0;

    // ── Main DAG ────────────────────────────────────────────────
    // Re-run validate to recover the `Dag`. The decomposition was
    // already validated upstream (an Err short-circuits to abort);
    // validation is a pure microsecond-cheap function over the task
    // list, so re-running it is cheaper than threading the Dag down.
    let dag = crate::task_graph::validate(&tasks)
        .expect("decomposition was already validated at emit time");
    match dispatch_dag(
        &task_ctx,
        &tasks,
        dag,
        max_parallel,
        &mut next_integration_index,
        &endurance,
        &inflight,
    )
    .await?
    {
        DagOutcome::Done => {}
        DagOutcome::Escalated => return Ok(()),
        DagOutcome::Aborted => {
            return abort(&event_bus, &mission_id, &seq, &state_tx, "user abort").await;
        }
    }

    // ── Final turn: declare complete (best-effort) ───────────────
    //
    // S9: the supervisor still runs the completion prompt so its
    // freeform prose is available for the Accept-card summary,
    // but the user-facing terminal signal is the typed
    // CompletionVerdict assembled from the mission's event stream
    // (which already carries every signal the verdict needs —
    // audits, arbiter decisions, recovery activity, touched
    // files — so the per-task `run_task` futures stay decoupled
    // from the verdict path).
    if cancel.is_cancelled() {
        return abort(&event_bus, &mission_id, &seq, &state_tx, "user abort").await;
    }
    let complete_prompt = format_complete_prompt(&tasks);
    // Match the session_id THEN driver lock order used by parallel
    // run_task review turns; the inverted order would deadlock if any
    // task held session_id while waiting on driver.
    let SupervisorTurnResult { outputs, .. } = {
        let sid_guard = session_id.lock().await;
        let mut driver_guard = driver.lock().await;
        driver_guard
            .run_turn_cancellable(
                &complete_prompt,
                sid_guard.as_deref(),
                &mission_cwd,
                Some(&cancel),
            )
            .await
    };
    // The completion turn ran — final supervisor-side progress beat.
    beat_best_effort(
        &endurance,
        BeatStatus {
            phase: Some("completing".into()),
            events_total: Some(seq.load(Ordering::Relaxed)),
            progressed: true,
            ..Default::default()
        },
    );
    let supervisor_prose = match first_intent(&outputs) {
        Some(SupervisorIntent::DeclareComplete { summary }) => summary.clone(),
        _ => format!("{} tasks complete", tasks.len()),
    };

    // ── S9: assemble the completion verdict ─────────────────────
    let snapshot = event_bus.snapshot_kinds();
    let mission_audit_owned = derive_mission_audit(&snapshot);
    let touched_files = derive_touched_files(&snapshot);
    let recovery_summary = derive_recovery_summary(&snapshot);
    let scrubs = derive_scrubs(&snapshot);
    let all_subtasks_accepted = derive_all_subtasks_accepted(&snapshot);
    let events_for_assembler = derive_assembler_events(&snapshot);

    // U10: book recoverable faults. Each recovery-engine intervention was a
    // worker failure the engine handled; reaching completion means it
    // recovered, so book injected+recovered (visible in the report, gate
    // unaffected). Fatal faults that abort the mission are booked at the
    // dispatch arms; quota pauses and arbiter quality-revisions are not
    // recovery events and are excluded.
    for _ in 0..recovery_fault_count(&snapshot) {
        note_fault_best_effort(&endurance, "recovery");
        note_recovery_best_effort(&endurance, "recovery");
    }

    let integrated_test_pass = mission_audit_owned
        .as_ref()
        .and_then(|r| r.test_pass.as_ref());
    let inputs = crate::judgment::AssembleInputs {
        worktree_root: mission_cwd.clone(),
        touched_files: &touched_files,
        all_subtasks_accepted,
        mission_audit: mission_audit_owned.as_ref(),
        integrated_test_pass,
        recovery_history: &recovery_summary,
        events: &events_for_assembler,
        scrubs: &scrubs,
    };
    let mut verdict = crate::judgment::assemble_verdict(&inputs);

    // The supervisor's prose, when present, replaces the default
    // Accept summary so the inbox card mentions the supervisor's
    // own framing (e.g. "shipped logout endpoint + tests").
    if let crate::arbiter::decision::ArbiterDecision::Accept(payload) = &mut verdict.recommendation
    {
        payload.summary = supervisor_prose.clone();
    }

    let payload_json = serde_json::to_string(&verdict).unwrap_or_else(|_| "{}".to_string());
    emit(
        &event_bus,
        &mission_id,
        &seq,
        MissionEventKind::CompletionVerdictRendered { payload_json },
    );

    state_tx.send(MissionState::CompletePendingMerge).ok();
    emit(
        &event_bus,
        &mission_id,
        &seq,
        MissionEventKind::Completed {
            summary: supervisor_prose,
            // The count of UNIQUE files the workers touched (deduplicated
            // from WorkerResultSubmitted), not the task count — the field
            // is surfaced in the UI as "N files changed". `touched_files`
            // is the same value already fed to the completion verdict.
            files_changed: touched_files.len() as u32,
        },
    );

    // Stop the heartbeat ticker BEFORE the terminal beat so a late ticker
    // beat cannot overwrite the final "done" phase. Relying on the
    // end-of-scope `AbortOnDrop` would race the terminal write, since
    // `JoinHandle::abort` is asynchronous.
    drop(_heartbeat_ticker);

    // Terminal beat: mission complete, no workers in flight.
    beat_best_effort(
        &endurance,
        BeatStatus {
            phase: Some("done".into()),
            workers_active: Some(0),
            events_total: Some(seq.load(Ordering::Relaxed)),
            progressed: true,
            ..Default::default()
        },
    );

    Ok(())
}

// ── S9 verdict-input derivation helpers ─────────────────────────
//
// All inputs to `judgment::assemble_verdict` are derived from the
// mission's event stream rather than threaded through `run_task` via
// shared accumulators. This keeps the per-task futures decoupled and
// makes the verdict trivially replay-derivable from a persisted event
// log later (S10's inbox can re-derive verdict from history if a
// future migration drops the live event_bus history).

/// Pull the last full AuditReport off the event stream. Prefers a
/// PostIntegrationAuditCompleted (S4 emits these after the
/// supervisor-branch reintegration); falls back to the last
/// AuditCompleted.
fn derive_mission_audit(snapshot: &[MissionEventKind]) -> Option<crate::audit::AuditReport> {
    let post_integration = snapshot.iter().rev().find_map(|ev| match ev {
        MissionEventKind::PostIntegrationAuditCompleted { payload_json, .. } => {
            Some(payload_json.as_str())
        }
        _ => None,
    });
    let fallback = || {
        snapshot.iter().rev().find_map(|ev| match ev {
            MissionEventKind::AuditCompleted { payload_json, .. } => Some(payload_json.as_str()),
            _ => None,
        })
    };
    let payload = post_integration.or_else(fallback)?;
    serde_json::from_str::<crate::audit::AuditReport>(payload).ok()
}

/// Union of files reported in WorkerResultSubmitted events,
/// deduplicated while preserving first-seen order.
fn derive_touched_files(snapshot: &[MissionEventKind]) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    for ev in snapshot {
        if let MissionEventKind::WorkerResultSubmitted { files, .. } = ev {
            for f in files {
                if !out.iter().any(|s| s == f) {
                    out.push(f.clone());
                }
            }
        }
    }
    out
}

/// Count of recovery-engine interventions in the event stream. Each is a
/// worker failure the recovery engine handled — a *recoverable* fault.
/// Quota pauses and arbiter quality-revisions are not `RecoveryDecided`
/// events, so they are correctly excluded.
fn recovery_fault_count(snapshot: &[MissionEventKind]) -> u64 {
    snapshot
        .iter()
        .filter(|ev| matches!(ev, MissionEventKind::RecoveryDecided { .. }))
        .count() as u64
}

/// Aggregate RecoveryDecided events into a `RecoveryHistorySummary`,
/// keyed by FailureClass wire name. Latest-action-wins per class.
fn derive_recovery_summary(
    snapshot: &[MissionEventKind],
) -> crate::judgment::RecoveryHistorySummary {
    let mut summary = crate::judgment::RecoveryHistorySummary::new();
    for ev in snapshot {
        if let MissionEventKind::RecoveryDecided {
            class_json,
            action_json,
            ..
        } = ev
        {
            let class: Result<crate::recovery::types::FailureClass, _> =
                serde_json::from_str(class_json);
            let action: Result<crate::recovery::types::RecoveryAction, _> =
                serde_json::from_str(action_json);
            if let (Ok(c), Ok(a)) = (class, action) {
                summary.add_class(
                    crate::recovery::history::wire_name_for_class(&c),
                    crate::recovery::history::wire_name_for_action(&a),
                    1,
                );
            }
        }
    }
    summary
}

/// Derive ScrubRecords from ArbiterDecided events whose decision_json
/// holds a Scrub variant. Worker id is parsed to recover task_index
/// via the `mock-N` convention used by run_task — including the
/// `mock-N-rK` form a Reassign rework mints (arbiter::rework_dispatch),
/// whose leading segment still carries the original task index; on
/// miss, defaults to 0.
fn derive_scrubs(snapshot: &[MissionEventKind]) -> Vec<crate::judgment::unresolved::ScrubRecord> {
    let mut out = Vec::new();
    for ev in snapshot {
        if let MissionEventKind::ArbiterDecided {
            worker_id,
            decision_json,
            bound,
            ..
        } = ev
        {
            if bound.is_some() {
                continue;
            }
            let decision: Result<crate::arbiter::decision::ArbiterDecision, _> =
                serde_json::from_str(decision_json);
            if let Ok(crate::arbiter::decision::ArbiterDecision::Scrub { reason, .. }) = decision {
                out.push(crate::judgment::unresolved::ScrubRecord {
                    task_index: worker_id
                        .strip_prefix("mock-")
                        // `mock-2-r1` (Reassign) -> leading "2" is the
                        // 1-based task number; `mock-2` -> "2".
                        .and_then(|s| s.split('-').next())
                        .and_then(|s| s.parse::<u32>().ok())
                        .map(|n| n.saturating_sub(1))
                        .unwrap_or(0),
                    reason: format!("{reason:?}").to_lowercase(),
                });
            }
        }
    }
    out
}

/// True iff every per-task ArbiterDecided ended in Accept — no
/// Escalate (bound = Some), no Scrub. Vacuously true on an empty
/// stream (degenerate zero-task missions).
fn derive_all_subtasks_accepted(snapshot: &[MissionEventKind]) -> bool {
    for ev in snapshot {
        if let MissionEventKind::ArbiterDecided {
            decision_json,
            bound,
            ..
        } = ev
        {
            if bound.is_some() {
                return false;
            }
            if let Ok(d) =
                serde_json::from_str::<crate::arbiter::decision::ArbiterDecision>(decision_json)
            {
                if matches!(d, crate::arbiter::decision::ArbiterDecision::Scrub { .. }) {
                    return false;
                }
            }
        }
    }
    true
}

/// Filter the snapshot down to the event kinds the unresolved-issues
/// collector cares about: open escalations and composer truncations.
fn derive_assembler_events(snapshot: &[MissionEventKind]) -> Vec<MissionEventKind> {
    snapshot
        .iter()
        .filter(|ev| {
            matches!(
                ev,
                MissionEventKind::ArbiterDecided { bound: Some(_), .. }
                    | MissionEventKind::ContextBudgetTruncated { .. }
            )
        })
        .cloned()
        .collect()
}

/// Outcome of dispatching the mission DAG. Distinct from
/// [`TaskOutcome`] because the dispatcher rolls up multiple per-task
/// outcomes into a single result the caller decides on.
enum DagOutcome {
    /// Every task in the DAG reached terminal state without a
    /// mission-halting event.
    Done,
    /// One task escalated; the dispatcher drained the in-flight
    /// tasks and the caller should exit the mission early.
    Escalated,
    /// User cancel observed; the caller should emit `Aborted` and
    /// transition the mission to `Aborted`.
    Aborted,
}

/// Drive a DAG through the parallel `JoinSet` dispatcher. Caps the in-flight pool at
/// `max_parallel`; `next_integration_index` is monotonic across
/// invocations so every integration tag stays unique mission-wide.
async fn dispatch_dag(
    task_ctx: &TaskRunCtx,
    tasks: &[TaskDescriptor],
    dag: crate::task_graph::Dag,
    max_parallel: usize,
    next_integration_index: &mut u32,
    endurance: &Option<EnduranceHandle>,
    inflight: &AtomicU32,
) -> Result<DagOutcome, MissionRuntimeError> {
    let mut scheduler = crate::task_graph::Scheduler::new(dag);
    let mut joinset: JoinSet<(u32, Result<TaskOutcome, MissionRuntimeError>)> = JoinSet::new();
    // Zero the in-flight count on every exit from this dispatcher (normal
    // or an early drain-and-return), so the ticker never reads a stale
    // count for a drained DAG.
    let _reset_inflight = ZeroOnDrop(inflight);

    loop {
        // ── Fill the pool up to the cap ─────────────────────────
        // `scheduler.ready()` enumerates indices with indegree 0
        // that are still Pending. We dispatch them in BTreeMap
        // order (so spawn-order across runs is deterministic for
        // any given DAG), one at a time, until either the pool is
        // full or there's nothing more to dispatch right now.
        while joinset.len() < max_parallel {
            let ready = scheduler.ready();
            if ready.is_empty() {
                break;
            }
            let next_idx = ready[0];
            scheduler.mark_running(next_idx);
            let task = tasks
                .iter()
                .find(|t| t.index == next_idx)
                .expect("scheduler emits indices from the same task list")
                .clone();
            let ctx = task_ctx.clone();
            let integration_index = *next_integration_index;
            *next_integration_index = next_integration_index.saturating_add(1);
            joinset.spawn(async move {
                let outcome = run_task(ctx, task, integration_index).await;
                (next_idx, outcome)
            });
        }
        // Publish the live in-flight count for the heartbeat ticker.
        inflight.store(joinset.len() as u32, Ordering::Relaxed);

        if joinset.is_empty() {
            // Nothing dispatched and nothing in flight. Either the
            // scheduler is done (loop exits on the next is_done()
            // check) or validate() rejected the DAG upstream — that
            // path aborts before we get here.
            break;
        }

        // ── Await the next completion ───────────────────────────
        match joinset.join_next().await {
            Some(Ok((completed_idx, Ok(TaskOutcome::Accepted)))) => {
                scheduler.mark_done(completed_idx);
                // U10 A4: a worker integrated → forward progress. After
                // a completion, `joinset.len()` is the count of tasks
                // still in flight, which is the live workers_active.
                beat_best_effort(
                    endurance,
                    BeatStatus {
                        phase: Some("executing".into()),
                        workers_active: Some(joinset.len() as u32),
                        events_total: Some(task_ctx.seq.load(Ordering::Relaxed)),
                        progressed: true,
                        ..Default::default()
                    },
                );
            }
            Some(Ok((_, Ok(TaskOutcome::Scrubbed | TaskOutcome::Escalated)))) => {
                // Do not expose disposition controls while a peer may still be
                // writing or integrating. First stop dispatching, then drain the
                // already-running set, and only then publish Attention.
                while joinset.join_next().await.is_some() {}
                task_ctx.state_tx.send(MissionState::Attention).ok();
                emit(
                    &task_ctx.event_bus,
                    &task_ctx.mission_id,
                    &task_ctx.seq,
                    MissionEventKind::AttentionReady,
                );
                return Ok(DagOutcome::Escalated);
            }
            Some(Ok((_, Ok(TaskOutcome::Aborted)))) => {
                while joinset.join_next().await.is_some() {}
                return Ok(DagOutcome::Aborted);
            }
            Some(Ok((_, Err(e)))) => {
                // Fatal fault: a worker dispatch errored and the mission
                // aborts — unrecovered, so it fails the all-day gate.
                note_fault_best_effort(endurance, "task_error");
                while joinset.join_next().await.is_some() {}
                return Err(e);
            }
            Some(Err(join_err)) => {
                // tokio task panic — surface as runtime error so
                // the caller's catch-all path emits a clean Aborted.
                // Fatal fault: an unrecovered worker panic.
                note_fault_best_effort(endurance, "worker_panic");
                while joinset.join_next().await.is_some() {}
                return Err(MissionRuntimeError::Io(format!(
                    "task panicked: {join_err}"
                )));
            }
            None => break,
        }

        if scheduler.is_done() {
            break;
        }
    }

    Ok(DagOutcome::Done)
}

fn supervisor_bad_decompose_reason(
    outputs: &[supervisor_adapter::SupervisorOutput],
    fallback: &str,
) -> String {
    if let Some(err) = outputs.iter().find_map(|output| match output {
        supervisor_adapter::SupervisorOutput::Error(err) => Some(err.as_str()),
        _ => None,
    }) {
        return format!(
            "supervisor turn failed before decomposition: {}",
            truncate_for_abort_reason(err)
        );
    }

    if let Some(log) = outputs.iter().rev().find_map(|output| match output {
        supervisor_adapter::SupervisorOutput::Log(log) => Some(log.as_str()),
        _ => None,
    }) {
        return format!(
            "{fallback} (last supervisor log: {})",
            truncate_for_abort_reason(log)
        );
    }

    fallback.to_string()
}

fn truncate_for_abort_reason(value: &str) -> String {
    const MAX: usize = 360;
    let trimmed = value.trim();
    let mut out = String::new();
    for ch in trimmed.chars().take(MAX) {
        out.push(ch);
    }
    if trimmed.chars().count() > MAX {
        out.push_str("...");
    }
    out
}

/// QC-3: map the parser-local [`supervisor_adapter::EnvelopeFit`] to
/// the orchestrator wire type. The shapes are byte-identical; the
/// duplication exists so the adapter crate stays independent of the
/// orchestrator (which depends on the adapter, not the other way
/// around).
fn adapter_envelope_to_orchestrator(
    ef: supervisor_adapter::EnvelopeFit,
) -> crate::mission_event::EnvelopeFit {
    fn map_kind(k: supervisor_adapter::BoundFitKind) -> crate::mission_event::BoundFitKind {
        use crate::mission_event::BoundFitKind as O;
        use supervisor_adapter::BoundFitKind as A;
        match k {
            A::Within => O::Within,
            A::NearLimit => O::NearLimit,
            A::Exceeds => O::Exceeds,
        }
    }
    fn map_bf(bf: supervisor_adapter::BoundFit) -> crate::mission_event::BoundFit {
        crate::mission_event::BoundFit {
            fit: map_kind(bf.fit),
            note: bf.note,
        }
    }
    crate::mission_event::EnvelopeFit {
        scope: map_bf(ef.scope),
        reversibility: map_bf(ef.reversibility),
        risk: map_bf(ef.risk),
        quality: map_bf(ef.quality),
    }
}

#[cfg(test)]
mod scrub_derivation_tests {
    use super::*;
    use crate::arbiter::decision::{ArbiterDecision, ScrubReason};

    fn scrub_event(worker_id: &str) -> MissionEventKind {
        let decision_json = serde_json::to_string(&ArbiterDecision::Scrub {
            reason: ScrubReason::QualityExhausted,
            retained_artifacts: vec![],
            partial_audit: None,
        })
        .unwrap();
        MissionEventKind::ArbiterDecided {
            worker_id: worker_id.to_string(),
            decision_json,
            audit_overall: 0.0,
            bound: None,
        }
    }

    #[test]
    fn scrub_task_index_from_plain_worker_id() {
        // Sanity: the canonical `mock-N` form (ids::worker_id_for_task_index)
        // maps back to the 0-based task index.
        assert_eq!(derive_scrubs(&[scrub_event("mock-1")])[0].task_index, 0);
        assert_eq!(derive_scrubs(&[scrub_event("mock-3")])[0].task_index, 2);
    }

    #[test]
    fn scrub_task_index_from_reassigned_worker_id() {
        // Regression: a Reassign mints `mock-{idx+1}-r{n}`
        // (arbiter::rework_dispatch). Scrubbing that worker must still
        // attribute the scrub to the real task index, not collapse to 0.
        let scrubs = derive_scrubs(&[scrub_event("mock-2-r1")]);
        assert_eq!(scrubs.len(), 1);
        assert_eq!(scrubs[0].task_index, 1);
    }

    #[test]
    fn recovery_fault_count_counts_only_recovery_events() {
        let rec = |w: &str| MissionEventKind::RecoveryDecided {
            worker_id: w.to_string(),
            class_json: "{}".to_string(),
            action_json: "{}".to_string(),
        };
        let events = vec![
            rec("mock-1"),
            MissionEventKind::WorkerProgress {
                worker_id: "mock-1".to_string(),
                note: "not a recovery".to_string(),
            },
            rec("mock-2"),
        ];
        // Only the two RecoveryDecided events are recoverable faults; the
        // WorkerProgress (and anything else) is excluded.
        assert_eq!(recovery_fault_count(&events), 2);
        assert_eq!(recovery_fault_count(&[]), 0);
    }
}

#[cfg(test)]
mod heartbeat_ticker_tests {
    use super::*;

    #[test]
    fn ticker_plan_active_reports_inflight_without_progress() {
        let (status, tick) = ticker_plan(3, false);
        assert_eq!(status.workers_active, Some(3));
        assert!(!status.progressed, "the ticker must never claim progress");
        assert_eq!(status.phase.as_deref(), Some("executing"));
        assert_eq!(tick, Tick::Active);
    }

    #[test]
    fn ticker_plan_empty_pool_is_idle() {
        let (status, tick) = ticker_plan(0, false);
        assert_eq!(status.workers_active, Some(0));
        assert_eq!(status.phase.as_deref(), Some("idle"));
        assert_eq!(tick, Tick::Idle);
    }

    #[test]
    fn ticker_plan_quota_pause_reads_idle_never_stall() {
        // The load-bearing case: a quota-parked worker still holds its
        // JoinSet slot (inflight > 0), but a quota pause must read Idle,
        // never Stalled — the exact confusion this subsystem prevents.
        let (status, tick) = ticker_plan(2, true);
        assert_eq!(
            status.workers_active,
            Some(0),
            "a quota pause must report no active workers"
        );
        assert_eq!(status.phase.as_deref(), Some("paused:quota"));
        assert_eq!(tick, Tick::Idle);
    }

    #[test]
    fn cadences_stay_under_crash_threshold() {
        // If an idle beat gap exceeded the crash threshold, an idle fleet
        // would false-read as Crashed instead of Idle.
        let crash =
            Duration::from_millis(crate::endurance::EnduranceConfig::default().crash_threshold_ms);
        assert!(HEARTBEAT_TICK_ACTIVE < crash);
        assert!(HEARTBEAT_TICK_IDLE < crash);
    }

    #[tokio::test]
    async fn fleet_quota_paused_tracks_current_exhaustion() {
        use event_schema::Vendor;
        let t = crate::recovery::VendorQuotaTracker::in_memory();
        assert!(!fleet_quota_paused(&t, 1_000).await, "nothing exhausted");
        t.mark_exhausted(Vendor::Claude, 1_000, Some(10_000))
            .await
            .unwrap();
        assert!(
            fleet_quota_paused(&t, 2_000).await,
            "Claude exhausted with reset in the future ⇒ paused"
        );
        assert!(
            !fleet_quota_paused(&t, 10_001).await,
            "reset already elapsed ⇒ not paused (stale entry)"
        );
    }

    #[tokio::test(flavor = "current_thread")]
    async fn heartbeat_ticker_emits_liveness_beats() {
        let dir = tempfile::TempDir::new().unwrap();
        let monitor = crate::endurance::EnduranceMonitor::launch(
            dir.path(),
            crate::endurance::SystemClock,
            crate::endurance::EnduranceConfig::default(),
        )
        .unwrap();
        let handle = Arc::new(std::sync::Mutex::new(monitor));
        let inflight = Arc::new(AtomicU32::new(1));
        let tracker = crate::recovery::VendorQuotaTracker::in_memory();

        let before = handle.lock().unwrap().heartbeat().beat_seq;
        // Tiny injected cadence so the test runs in a few ms of real time.
        let tick = Duration::from_millis(5);
        let task = spawn_heartbeat_ticker(handle.clone(), inflight, tracker, tick, tick);

        // Let the ticker run several cycles, then stop it.
        tokio::time::sleep(Duration::from_millis(60)).await;
        task.abort();

        let hb = handle.lock().unwrap();
        assert!(
            hb.heartbeat().beat_seq > before,
            "ticker should emit liveness beats over time (before={before}, after={})",
            hb.heartbeat().beat_seq
        );
        // Those are workers-in-flight, no-progress liveness beats — the
        // state from which a real wedge would accrue toward Stalled.
        assert_eq!(hb.heartbeat().workers_active, 1);
    }

    #[tokio::test(flavor = "current_thread")]
    async fn ticker_reports_idle_when_quota_paused_with_workers_in_flight() {
        // The subsystem's headline guarantee, end-to-end through the live
        // ticker (not just the pure `ticker_plan`): workers ARE in flight
        // — which would read Stalled if mistaken for a wedge — but the
        // fleet is quota-paused, so the beat must report no active workers
        // and the monitor must NOT read Stalled/Crashed.
        use event_schema::Vendor;
        let dir = tempfile::TempDir::new().unwrap();
        let monitor = crate::endurance::EnduranceMonitor::launch(
            dir.path(),
            crate::endurance::SystemClock,
            crate::endurance::EnduranceConfig::default(),
        )
        .unwrap();
        let handle = Arc::new(std::sync::Mutex::new(monitor));
        let inflight = Arc::new(AtomicU32::new(2));
        let tracker = crate::recovery::VendorQuotaTracker::in_memory();
        // Exhausted with a reset far in the future ⇒ currently paused.
        tracker
            .mark_exhausted(Vendor::Claude, 0, Some(u64::MAX))
            .await
            .unwrap();

        let tick = Duration::from_millis(5);
        let task = spawn_heartbeat_ticker(handle.clone(), inflight, tracker, tick, tick);
        tokio::time::sleep(Duration::from_millis(60)).await;
        task.abort();

        let hb = handle.lock().unwrap();
        assert_eq!(
            hb.heartbeat().workers_active,
            0,
            "a quota pause must report no active workers even with the pool full"
        );
        assert!(
            !hb.liveness().needs_attention(),
            "quota pause with workers in flight must not read Stalled/Crashed: {:?}",
            hb.liveness()
        );
    }
}
