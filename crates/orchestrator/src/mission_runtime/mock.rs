use super::support::{abort, emit, run_git_in};
use super::{CancelToken, MissionEventBus, MissionRuntimeError};
use crate::memory::MemoryKernel;
use crate::mission::{MissionSpec, MissionState};
use crate::mission_event::{MissionEventKind, TaskDescriptor};
use crate::mission_workspace::MissionWorkspace;
use crate::mock_worker::MockWorkerVariant;
use std::sync::atomic::AtomicU64;
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::watch;

/// Per-step delays for the mock supervisor timeline. The realtime
/// preset matches `msv-spec.md` §4.2 (~2 min end-to-end); the fast
/// preset collapses everything to milliseconds for tests.
#[derive(Debug, Clone, Copy)]
pub struct MockTimingConfig {
    pub decomposition_delay: Duration,
    pub worker_spawn_stagger: Duration,
    pub worker_work_duration: Duration,
    pub progress_tick: Duration,
    pub review_delay: Duration,
    pub test_delay: Duration,
}

impl MockTimingConfig {
    pub fn realtime() -> Self {
        Self {
            decomposition_delay: Duration::from_secs(1),
            worker_spawn_stagger: Duration::from_millis(500),
            worker_work_duration: Duration::from_secs(40),
            progress_tick: Duration::from_secs(5),
            review_delay: Duration::from_secs(2),
            test_delay: Duration::from_secs(2),
        }
    }

    pub fn fast() -> Self {
        Self {
            decomposition_delay: Duration::from_millis(1),
            worker_spawn_stagger: Duration::from_millis(1),
            worker_work_duration: Duration::from_millis(2),
            progress_tick: Duration::from_millis(1),
            review_delay: Duration::from_millis(1),
            test_delay: Duration::from_millis(1),
        }
    }
}

#[allow(clippy::too_many_arguments)]
pub(super) async fn run_mock_mission(
    mission_id: String,
    spec: MissionSpec,
    workspace: MissionWorkspace,
    event_bus: MissionEventBus,
    state_tx: Arc<watch::Sender<MissionState>>,
    cancel: Arc<CancelToken>,
    seq: Arc<AtomicU64>,
    config: MockTimingConfig,
    memory: Option<Arc<MemoryKernel>>,
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

    if cancel.sleep_or_cancel(config.decomposition_delay).await {
        return abort(&event_bus, &mission_id, &seq, &state_tx, "user abort").await;
    }

    let tasks = decompose_mock_tasks(spec.worker_count);
    emit(
        &event_bus,
        &mission_id,
        &seq,
        MissionEventKind::Decomposition {
            tasks: tasks.clone(),
        },
    );

    for (integration_index, task) in tasks.iter().enumerate() {
        let worker_id = crate::ids::worker_id_for_task_index(task.index);
        let _ = &memory; // referenced below per worker; suppress "moved" warnings if added later

        if cancel.sleep_or_cancel(config.worker_spawn_stagger).await {
            return abort(&event_bus, &mission_id, &seq, &state_tx, "user abort").await;
        }

        workspace.create_worker_branch(&worker_id).await?;
        let worktree = workspace.create_worker_worktree(&worker_id).await?;

        // Tier-2A: render the curated memory bundle into the worker's
        // worktree before the worker file is written. Fail-soft —
        // memory failure must never block mission dispatch.
        if let Some(kernel) = &memory {
            let vendor = crate::memory::vendor_for_model(spec.worker_model.as_deref());
            let _ = crate::memory::attach_to_worktree(
                kernel,
                &mission_id,
                &worker_id,
                0,
                vendor,
                &worktree,
            )
            .await;
        }

        emit(
            &event_bus,
            &mission_id,
            &seq,
            MissionEventKind::WorkerSpawned {
                worker_id: worker_id.clone(),
                task_index: task.index,
                task_title: task.title.clone(),
            },
        );

        if cancel.sleep_or_cancel(config.progress_tick).await {
            return abort(&event_bus, &mission_id, &seq, &state_tx, "user abort").await;
        }
        emit(
            &event_bus,
            &mission_id,
            &seq,
            MissionEventKind::WorkerProgress {
                worker_id: worker_id.clone(),
                note: format!("Working on: {}", task.title),
            },
        );

        if cancel.sleep_or_cancel(config.worker_work_duration).await {
            return abort(&event_bus, &mission_id, &seq, &state_tx, "user abort").await;
        }

        // Pre-U3.4 mock path: always run variant 0 (Happy) first pass.
        // The supervisor-driven path in U3.4 chooses the variant from
        // `task.index` and may iterate to a revision pass — for the
        // mock path here, every task gets the Happy first pass so the
        // legacy event stream stays identical.
        let variant = MockWorkerVariant::new(crate::mock_worker::MockWorkerKind::Happy);
        let pass = variant.run_pass(task);
        tokio::fs::write(worktree.join(&pass.file_name), &pass.file_content)
            .await
            .map_err(|e| MissionRuntimeError::Io(e.to_string()))?;
        run_git_in(&worktree, &["add", &pass.file_name]).await?;
        run_git_in(&worktree, &["commit", "-m", &pass.commit_message]).await?;

        emit(
            &event_bus,
            &mission_id,
            &seq,
            MissionEventKind::WorkerResultSubmitted {
                worker_id: worker_id.clone(),
                files: vec![pass.file_name.clone()],
                summary: pass.submission_summary.clone(),
            },
        );

        state_tx.send(MissionState::Reviewing).ok();
        if cancel.sleep_or_cancel(config.review_delay).await {
            return abort(&event_bus, &mission_id, &seq, &state_tx, "user abort").await;
        }
        emit(
            &event_bus,
            &mission_id,
            &seq,
            MissionEventKind::ReviewStarted {
                worker_id: worker_id.clone(),
            },
        );

        let integration = match workspace
            .integrate_worker(&worker_id, integration_index as u32, &task.title)
            .await?
        {
            crate::mission_workspace::MergeOutcome::Success(i) => i,
            crate::mission_workspace::MergeOutcome::Conflict(c) => {
                // Parity with mission_loop.rs: surface a proper
                // ArbiterDecided{ Reversibility } event with the
                // conflict list as evidence.payload_json. The mock
                // path doesn't run a real audit, so audit_overall is
                // 1.0 (matching the AuditCompleted event the mock
                // emits on the success path below).
                let evidence = crate::arbiter::EscalationEvidence {
                    summary: c.summary(),
                    payload_json: serde_json::to_string(&c.conflicts).ok(),
                };
                let decision = crate::arbiter::ArbiterDecision::Escalate {
                    bound: crate::arbiter::AuthorityBound::Reversibility,
                    evidence: evidence.clone(),
                    suggested_user_action: crate::arbiter::SuggestedUserAction::ResolveMission,
                };
                emit(
                    &event_bus,
                    &mission_id,
                    &seq,
                    MissionEventKind::ArbiterDecided {
                        worker_id: worker_id.clone(),
                        decision_json: serde_json::to_string(&decision)
                            .unwrap_or_else(|_| "{}".to_string()),
                        audit_overall: 1.0,
                        bound: Some(crate::arbiter::AuthorityBound::Reversibility),
                    },
                );
                emit(
                    &event_bus,
                    &mission_id,
                    &seq,
                    MissionEventKind::WorkerProgress {
                        worker_id: worker_id.clone(),
                        note: format!("escalated (Reversibility): {}", evidence.summary),
                    },
                );
                state_tx.send(MissionState::Attention).ok();
                return Ok(());
            }
        };

        emit(
            &event_bus,
            &mission_id,
            &seq,
            MissionEventKind::Integrated {
                worker_id: worker_id.clone(),
                integration_sha: integration.integration_sha,
                snapshot_tag: integration.snapshot_tag,
            },
        );

        state_tx.send(MissionState::Executing).ok();
        if cancel.sleep_or_cancel(config.test_delay).await {
            return abort(&event_bus, &mission_id, &seq, &state_tx, "user abort").await;
        }
        emit(
            &event_bus,
            &mission_id,
            &seq,
            MissionEventKind::TestResult {
                passed: true,
                summary: "mock tests pass".into(),
            },
        );
        // Forward-compat: emit AuditCompleted alongside the legacy TestResult
        // so the mock path matches the real mission_loop's event stream.
        // S2 integration tests that read audit data from a mock-backed mission
        // will have a typed score to inspect rather than falling through to the
        // default handler.
        let audit_report = crate::audit::AuditReport {
            overall: 1.0,
            ..Default::default()
        };
        let payload_json = serde_json::to_string(&audit_report).unwrap_or_else(|_| "{}".into());
        emit(
            &event_bus,
            &mission_id,
            &seq,
            MissionEventKind::AuditCompleted {
                tier: "smoke".into(),
                overall: 1.0,
                payload_json,
            },
        );
    }

    state_tx.send(MissionState::CompletePendingMerge).ok();
    emit(
        &event_bus,
        &mission_id,
        &seq,
        MissionEventKind::Completed {
            summary: format!("{} tasks integrated", tasks.len()),
            files_changed: tasks.len() as u32,
        },
    );

    Ok(())
}

/// Build the mock supervisor's task decomposition. Respects an
/// explicit `worker_count` from `MissionSpec`; defaults to 3 when
/// the user left "Auto" selected. Clamped to [1, 10] so a stray
/// large number can't spawn 1000 workers. Reuses the original
/// three named titles for the first three slots and falls back
/// to "Task N" beyond that.
pub(super) fn decompose_mock_tasks(worker_count: Option<u32>) -> Vec<TaskDescriptor> {
    const NAMED: [&str; 3] = [
        "Plan integration",
        "Implement changes",
        "Update documentation",
    ];
    let count = worker_count.unwrap_or(3).clamp(1, 10) as usize;
    (0..count)
        .map(|i| TaskDescriptor {
            index: i as u32,
            title: NAMED
                .get(i)
                .map(|s| (*s).to_string())
                .unwrap_or_else(|| format!("Task {}", i + 1)),
            ..Default::default()
        })
        .collect()
}
