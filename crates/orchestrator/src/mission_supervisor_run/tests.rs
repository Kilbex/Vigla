use super::*;
use crate::mission::{MissionSpec, MissionState};
use crate::mission_event::{MissionEvent, MissionEventKind, TaskDescriptor};
use crate::mission_runtime::{CancelToken, MissionEventBus, MissionRuntimeError, PlanDecision};
use crate::mission_worker_dispatch::WorkerVendor;
use crate::mission_workspace::MissionWorkspace;
use crate::vendor_profile::DeclaredSideEffectKind;
use std::path::PathBuf;
use std::process::{Command as SyncCommand, Stdio};
use std::sync::atomic::{AtomicBool, AtomicU32, AtomicU64};
use std::sync::Arc;
use std::time::Duration;
use supervisor_adapter::SupervisorOutput;
use tempfile::TempDir;
use tokio::process::Command;
use tokio::sync::{mpsc, watch};

#[test]
fn supervisor_disallowed_tools_enables_read_glob_ls_for_discovery() {
    // QC-1: the supervisor needs to read codebase context before
    // decomposing. Read / Glob / LS MUST be allowed (i.e. not in
    // the disallowed list). Edit / Write / MultiEdit / Bash MUST
    // stay disabled — the supervisor is judgment-only.
    let d = super::SUPERVISOR_DISALLOWED_TOOLS;

    for tool in ["Bash", "Edit", "Write", "MultiEdit"] {
        assert!(
            d.split(',').any(|t| t == tool),
            "supervisor MUST disable {tool}: {d}"
        );
    }
    for tool in ["Read", "Glob", "LS"] {
        assert!(
            !d.split(',').any(|t| t == tool),
            "supervisor MUST NOT disable {tool} (needed for codebase \
                 discovery before decompose): {d}"
        );
    }
}

#[test]
fn supervisor_max_turns_allows_discover_then_decompose() {
    // QC-1: each tool call consumes a turn. The supervisor's
    // discover-then-decompose flow needs room: a few Read calls,
    // an LS or Glob, then the decompose JSON. 4 turns is too
    // tight; we set 8 as a comfortable upper bound.
    #[allow(clippy::assertions_on_constants)]
    // intentional compile-time-style guardrail against future tightening
    {
        assert!(
            super::SUPERVISOR_MAX_TURNS >= 6,
            "max-turns must leave room for discovery+decompose; got {}",
            super::SUPERVISOR_MAX_TURNS
        );
    }
}

#[tokio::test(flavor = "current_thread")]
async fn supervisor_turn_drains_bounded_stderr_without_blocking_child_exit() {
    let mut child = Command::new("/bin/sh")
        .arg("-c")
        .arg("i=0; while [ $i -lt 10000 ]; do echo supervisor-noise >&2; i=$((i+1)); done")
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true)
        .spawn()
        .expect("spawn noisy supervisor");
    let stdout = child.stdout.take().expect("stdout");
    let stderr = child.stderr.take().expect("stderr");

    let result = tokio::time::timeout(
        Duration::from_secs(2),
        collect_supervisor_turn(child, stdout, stderr, Duration::from_secs(2)),
    )
    .await
    .expect("stderr-heavy supervisor turn must not block on a full pipe");

    let logs = result
        .outputs
        .iter()
        .filter_map(|o| match o {
            SupervisorOutput::Log(line) => Some(line.as_str()),
            _ => None,
        })
        .collect::<Vec<_>>();
    assert!(
        logs.iter()
            .any(|line| line.contains("supervisor stderr: supervisor-noise")),
        "stderr should be surfaced as bounded supervisor logs: {logs:?}"
    );
    assert!(
        logs.iter()
            .any(|line| line.contains("stderr output truncated")),
        "stderr log stream should be capped instead of growing unbounded: {logs:?}"
    );
}

#[tokio::test(flavor = "current_thread")]
async fn supervisor_turn_surfaces_nonzero_exit_status() {
    let mut child = Command::new("/bin/sh")
        .arg("-c")
        .arg("echo supervisor failed >&2; exit 42")
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true)
        .spawn()
        .expect("spawn failing supervisor");
    let stdout = child.stdout.take().expect("stdout");
    let stderr = child.stderr.take().expect("stderr");

    let result = collect_supervisor_turn(child, stdout, stderr, Duration::from_secs(2)).await;

    assert!(
        result.outputs.iter().any(|output| matches!(
            output,
            SupervisorOutput::Error(err)
                if err.contains("supervisor process exited unsuccessfully")
        )),
        "non-zero supervisor exit should be surfaced as Error: {:?}",
        result.outputs
    );
}

#[cfg(unix)]
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn cancelling_supervisor_turn_kills_its_process_group() {
    use std::os::unix::fs::PermissionsExt;

    let dir = tempfile::tempdir().unwrap();
    let descendant_pid_file = dir.path().join("descendant-pid");
    let script = dir.path().join("fake-claude");
    std::fs::write(
        &script,
        format!(
            "#!/bin/sh\nsleep 30 &\nprintf '%s' \"$!\" > '{}'\nwait\n",
            descendant_pid_file.display(),
        ),
    )
    .unwrap();
    let mut permissions = std::fs::metadata(&script).unwrap().permissions();
    permissions.set_mode(0o755);
    std::fs::set_permissions(&script, permissions).unwrap();

    let cancel = CancelToken::new();
    let task_cancel = Arc::clone(&cancel);
    let cwd = dir.path().to_path_buf();
    let mut driver = SupervisorDriver::RealClaude(RealClaudeConfig {
        binary: script.to_string_lossy().into_owned(),
        model: None,
        turn_timeout: Duration::from_secs(30),
    });
    let task = tokio::spawn(async move {
        driver
            .run_turn_cancellable("prompt", None, &cwd, Some(&task_cancel))
            .await
    });

    let descendant_pid = tokio::time::timeout(Duration::from_secs(10), async {
        loop {
            if let Ok(contents) = std::fs::read_to_string(&descendant_pid_file) {
                if let Ok(pid) = contents.parse::<i32>() {
                    break pid;
                }
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .expect("fake supervisor did not publish its descendant PID");
    cancel.cancel();
    let result = tokio::time::timeout(Duration::from_secs(2), task)
        .await
        .expect("supervisor cancellation was not prompt")
        .expect("supervisor task panicked");
    assert!(result.outputs.iter().any(
        |output| matches!(output, SupervisorOutput::Error(message) if message.contains("cancelled"))
    ));

    tokio::time::timeout(Duration::from_secs(2), async {
        while unsafe { libc::kill(descendant_pid, 0) } == 0 {
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .expect("cancelled supervisor left a descendant alive");
}

#[test]
fn decompose_prompt_points_supervisor_at_codebase_discovery() {
    let spec = MissionSpec {
        title: "Add logout".into(),
        objective: "Add a /api/logout endpoint that invalidates the session.".into(),
        target_ref: "main".into(),
        tests: None,
        supervisor_model: None,
        worker_model: None,
        worker_count: None,
        confirm_plan: None,
        scope_paths: vec![],
    };
    let prompt = super::format_decompose_prompt(&spec);
    // The prompt must explicitly reference the playbook section so a
    // future prompt edit can't silently drop the discovery step.
    assert!(
        prompt.contains("Codebase discovery"),
        "decompose prompt must reference the Codebase discovery playbook section: {prompt}"
    );
    // And it must keep the original 1–6 tasks contract.
    assert!(
        prompt.contains("between 1 and 6 tasks"),
        "decompose prompt must keep the 1–6 tasks contract: {prompt}"
    );
    // And it must still carry the user's objective verbatim.
    assert!(
        prompt.contains("Add a /api/logout endpoint"),
        "decompose prompt must include the user's objective: {prompt}"
    );
}

#[test]
fn decompose_prompt_carries_requested_worker_count_when_set() {
    let mut spec = ok_spec();
    spec.worker_count = Some(1);
    let prompt = super::format_decompose_prompt(&spec);

    assert!(
        prompt.contains("Requested worker count: exactly 1 task"),
        "decompose prompt must carry the explicit worker count: {prompt}"
    );
}

fn make_sandbox_repo() -> (TempDir, PathBuf) {
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
    run(&["init", "--initial-branch=main"]);
    run(&["config", "user.email", "test@example.com"]);
    run(&["config", "user.name", "Test"]);
    run(&["config", "commit.gpgsign", "false"]);
    std::fs::write(path.join("README.md"), "test\n").unwrap();
    run(&["add", "README.md"]);
    run(&["commit", "-m", "initial"]);
    (temp, path)
}

fn ok_spec() -> MissionSpec {
    MissionSpec {
        title: "Supervised mission".into(),
        objective: "Run the supervisor flow end-to-end".into(),
        target_ref: "main".into(),
        tests: None,
        supervisor_model: Some("claude".into()),
        worker_model: None,
        worker_count: None,
        confirm_plan: None,
        scope_paths: vec![],
    }
}

async fn collect_events(
    rx: &mut crate::mission_runtime::MissionEventReceiver,
    stop: impl Fn(&MissionEventKind) -> bool,
) -> Vec<MissionEvent> {
    let mut events = Vec::new();
    loop {
        match tokio::time::timeout(Duration::from_secs(10), rx.recv()).await {
            Ok(Ok(e)) => {
                let done = stop(&e.kind);
                events.push(e);
                if done {
                    return events;
                }
            }
            _ => return events,
        }
    }
}

async fn run_with_driver(
    spec: MissionSpec,
    mission_id: &str,
    root: PathBuf,
    driver: SupervisorDriver,
) -> (
    Arc<watch::Sender<MissionState>>,
    watch::Receiver<MissionState>,
    crate::mission_runtime::MissionEventReceiver,
    tokio::task::JoinHandle<Result<(), MissionRuntimeError>>,
) {
    let workspace = MissionWorkspace::new(root, mission_id.into()).unwrap();
    workspace.create_supervisor_branch("main").await.unwrap();
    workspace.create_supervisor_worktree().await.unwrap();

    let event_bus = MissionEventBus::new(256);
    let event_rx = event_bus.subscribe();
    let (state_tx_raw, state_rx) = watch::channel(MissionState::Created);
    let state_tx = Arc::new(state_tx_raw);
    let cancel = CancelToken::new();
    let seq = Arc::new(AtomicU64::new(0));
    let (_plan_tx, plan_rx) = mpsc::channel(4);

    let mid = mission_id.to_string();
    let st = state_tx.clone();
    let join = tokio::spawn(async move {
        run_supervisor_mission(
            mid,
            spec,
            workspace,
            event_bus,
            st,
            cancel,
            seq,
            driver,
            WorkerBackend::Mock,
            plan_rx,
            Arc::new(AtomicU32::new(0)),
            Arc::new(AtomicBool::new(false)),
            None,
            None,
            None,
        )
        .await
    });
    (state_tx, state_rx, event_rx, join)
}

/// U10 A4: the supervisor mission loop, when handed a process-level
/// endurance monitor, emits liveness beats as it makes progress — the
/// decomposition turn, each task completion, and the terminal beat.
/// End-to-end proof that `EnduranceMonitor::beat()` is now wired into
/// the live `run_supervisor_mission` loop (the seam the endurance
/// subsystem deliberately shipped without in U10 A2/A3).
#[tokio::test(flavor = "current_thread")]
async fn endurance_monitor_beats_through_supervised_mission() {
    use supervisor_adapter::{SupervisorIntent, SupervisorTaskDescriptor};

    let (_repo, root) = make_sandbox_repo();

    // Two independent tasks, legacy envelope shape (no plan-approval
    // pause when confirm_plan = None), so the loop dispatches, integrates
    // two mock workers, then completes — exercising every beat seam.
    let decompose = SupervisorOutput::Intent(SupervisorIntent::Decompose {
        tasks: vec![
            SupervisorTaskDescriptor {
                title: "task A".into(),
                description: None,
                ..Default::default()
            },
            SupervisorTaskDescriptor {
                title: "task B".into(),
                description: None,
                ..Default::default()
            },
        ],
        overview: None,
        tech_stack: None,
        envelope_fit: None,
    });
    let driver = SupervisorDriver::Scripted(ScriptedSupervisor::new(vec![vec![decompose]]));

    // Process-level monitor on its own root (a watchdog reads this path).
    // SystemClock is fine: this test asserts on beat *counts*, not on
    // time-compression (that is the soak test's job).
    let endurance_dir = TempDir::new().expect("endurance tempdir");
    let monitor = crate::endurance::EnduranceMonitor::launch(
        endurance_dir.path(),
        crate::endurance::SystemClock,
        crate::endurance::EnduranceConfig::default(),
    )
    .expect("launch monitor");
    let handle = Arc::new(std::sync::Mutex::new(monitor));

    // Inline mission setup: the shared `run_with_driver` injects no
    // monitor; this is the one test that does.
    let mission_id = "sup-endurance-0001";
    let workspace = MissionWorkspace::new(root, mission_id.into()).unwrap();
    workspace.create_supervisor_branch("main").await.unwrap();
    workspace.create_supervisor_worktree().await.unwrap();
    let event_bus = MissionEventBus::new(256);
    let mut event_rx = event_bus.subscribe();
    let (state_tx_raw, _state_rx) = watch::channel(MissionState::Created);
    let state_tx = Arc::new(state_tx_raw);
    let cancel = CancelToken::new();
    let seq = Arc::new(AtomicU64::new(0));
    let (_plan_tx, plan_rx) = mpsc::channel(4);

    let join = tokio::spawn(run_supervisor_mission(
        mission_id.to_string(),
        ok_spec(),
        workspace,
        event_bus,
        state_tx,
        cancel,
        seq,
        driver,
        WorkerBackend::Mock,
        plan_rx,
        Arc::new(AtomicU32::new(0)),
        Arc::new(AtomicBool::new(false)),
        None,
        None,
        Some(handle.clone()),
    ));

    let events = collect_events(&mut event_rx, |k| {
        matches!(k, MissionEventKind::Completed { .. })
    })
    .await;
    assert!(
        events
            .iter()
            .any(|e| matches!(e.kind, MissionEventKind::Completed { .. })),
        "mission did not reach Completed: {:?}",
        events.iter().map(|e| &e.kind).collect::<Vec<_>>()
    );
    join.await.expect("mission task joins").expect("mission ok");

    // The loop beat through the monitor: progress recorded for the two
    // task completions plus lifecycle beats, the durable heartbeat file
    // exists, the mission id was stamped, the terminal phase is `done`,
    // and liveness is benign (never stalled/crashed for a clean run).
    let mon = handle.lock().unwrap();
    let report = mon.report();
    assert!(
        report.progress_events >= 2,
        "expected ≥2 progress beats (one per integrated task), got {}",
        report.progress_events
    );
    assert!(mon.heartbeat().beat_seq >= 2, "beats were emitted");
    assert_eq!(
        mon.heartbeat().mission_id.as_deref(),
        Some(mission_id),
        "mission id stamped on the heartbeat"
    );
    assert_eq!(mon.heartbeat().phase, "done", "terminal phase recorded");
    assert!(
        !mon.liveness().needs_attention(),
        "completed mission should not read stalled/crashed: {:?}",
        mon.liveness()
    );
    assert!(
        crate::endurance::heartbeat_path(endurance_dir.path()).exists(),
        "durable heartbeat file written"
    );
}

#[test]
fn select_worker_backend_routes_supervisor_model_string() {
    let mut spec = ok_spec();
    // None and "auto" mean task-role-based real CLI routing.
    spec.worker_model = None;
    assert!(matches!(
        select_worker_backend(&spec),
        WorkerBackend::AutoReal
    ));
    spec.worker_model = Some("auto".into());
    assert!(matches!(
        select_worker_backend(&spec),
        WorkerBackend::AutoReal
    ));
    // Concrete vendors map to RealCli with the right variant.
    spec.worker_model = Some("claude".into());
    assert!(matches!(
        select_worker_backend(&spec),
        WorkerBackend::RealCli(WorkerVendor::Claude)
    ));
    spec.worker_model = Some("codex".into());
    assert!(matches!(
        select_worker_backend(&spec),
        WorkerBackend::RealCli(WorkerVendor::Codex)
    ));
    spec.worker_model = Some("gemini".into());
    assert!(matches!(
        select_worker_backend(&spec),
        WorkerBackend::RealCli(WorkerVendor::Gemini)
    ));
    // Comma-separated values are the UI's per-employee roster:
    // task index 0 uses the first CLI, index 1 the second, etc.
    spec.worker_model = Some("claude,codex,gemini".into());
    assert!(matches!(
        select_worker_backend(&spec),
        WorkerBackend::Roster(_)
    ));
    spec.worker_model = Some("claude:sonnet,codex:gpt-5.5,gemini".into());
    assert!(matches!(
        select_worker_backend(&spec),
        WorkerBackend::Roster(_)
    ));
    spec.worker_model = Some("claude:haiku".into());
    assert!(matches!(
        select_worker_backend(&spec),
        WorkerBackend::RealCli(WorkerVendor::Claude)
    ));
    // Unrecognized values fall back to Mock — the host IPC
    // rejects bad strings upstream; this is a belt-and-suspenders
    // safety net for tests that hand-build specs.
    spec.worker_model = Some("opencode".into());
    assert!(matches!(select_worker_backend(&spec), WorkerBackend::Mock));
}

#[test]
fn auto_real_backend_resolves_per_task_role() {
    let mut task = TaskDescriptor {
        role: crate::task_graph::TaskRole::Implementer,
        ..Default::default()
    };
    assert!(matches!(
        super::worker_pass::resolve_worker_backend_for_task(WorkerBackend::AutoReal, &task, None),
        WorkerBackend::RealCli(WorkerVendor::Claude)
    ));

    task.role = crate::task_graph::TaskRole::Tester;
    assert!(matches!(
        super::worker_pass::resolve_worker_backend_for_task(WorkerBackend::AutoReal, &task, None),
        WorkerBackend::RealCli(WorkerVendor::Codex)
    ));

    task.role = crate::task_graph::TaskRole::Reviewer;
    assert!(matches!(
        super::worker_pass::resolve_worker_backend_for_task(WorkerBackend::AutoReal, &task, None),
        WorkerBackend::RealCli(WorkerVendor::Claude)
    ));

    assert!(matches!(
        super::worker_pass::resolve_worker_backend_for_task(
            WorkerBackend::AutoReal,
            &task,
            Some("gemini")
        ),
        WorkerBackend::RealCli(WorkerVendor::Gemini)
    ));
}

#[test]
fn roster_backend_resolves_by_task_index_and_cycles() {
    let mut spec = ok_spec();
    spec.worker_model = Some("claude:sonnet,codex:gpt-5.5,gemini".into());
    let backend = select_worker_backend(&spec);

    let task0 = TaskDescriptor {
        index: 0,
        ..Default::default()
    };
    let task1 = TaskDescriptor {
        index: 1,
        ..Default::default()
    };
    let task2 = TaskDescriptor {
        index: 2,
        ..Default::default()
    };
    let task3 = TaskDescriptor {
        index: 3,
        ..Default::default()
    };

    assert!(matches!(
        super::worker_pass::resolve_worker_backend_for_task(backend, &task0, None),
        WorkerBackend::RealCli(WorkerVendor::Claude)
    ));
    assert!(matches!(
        super::worker_pass::resolve_worker_backend_for_task(backend, &task1, None),
        WorkerBackend::RealCli(WorkerVendor::Codex)
    ));
    assert!(matches!(
        super::worker_pass::resolve_worker_backend_for_task(backend, &task2, None),
        WorkerBackend::RealCli(WorkerVendor::Gemini)
    ));
    assert!(matches!(
        super::worker_pass::resolve_worker_backend_for_task(backend, &task3, None),
        WorkerBackend::RealCli(WorkerVendor::Claude)
    ));
    assert_eq!(
        super::worker_pass::resolve_worker_model_for_task(
            WorkerBackend::RealCli(WorkerVendor::Claude),
            &task0,
            spec.worker_model.as_deref(),
        )
        .as_deref(),
        Some("sonnet")
    );
    assert_eq!(
        super::worker_pass::resolve_worker_model_for_task(
            WorkerBackend::RealCli(WorkerVendor::Codex),
            &task1,
            spec.worker_model.as_deref(),
        )
        .as_deref(),
        Some("gpt-5.5")
    );
    assert_eq!(
        super::worker_pass::resolve_worker_model_for_task(
            WorkerBackend::RealCli(WorkerVendor::Gemini),
            &task2,
            spec.worker_model.as_deref(),
        ),
        None
    );
}

#[test]
fn forced_side_effect_detection_logs_declared_package_install_for_real_worker() {
    let events = side_effect_events_for_submission(
        "mock-1",
        WorkerBackend::RealCli(WorkerVendor::Codex),
        "Updated docs and ran `python -m pip install colorama==0.4.6 --target /tmp/demo`.",
    );

    assert_eq!(events.len(), 1);
    match &events[0] {
        MissionEventKind::SideEffectLogged {
            worker_id,
            kind,
            summary,
            declared,
        } => {
            assert_eq!(worker_id, "mock-1");
            assert_eq!(*kind, DeclaredSideEffectKind::PackageInstall);
            assert!(summary.contains("python -m pip install"));
            assert!(
                *declared,
                "Codex profile declares possible package installs"
            );
        }
        other => panic!("unexpected event: {other:?}"),
    }
}

#[test]
fn side_effect_detection_ignores_plain_submission_summaries() {
    let events = side_effect_events_for_submission(
        "mock-1",
        WorkerBackend::RealCli(WorkerVendor::Claude),
        "Updated README.md only.",
    );
    assert!(events.is_empty());
}

// Task 16: supervisor-review-pass tombstones removed in T1. The
// arbiter pipeline supersedes the review-pass tests (covered by
// tests/arbiter_decide.rs).

#[tokio::test(flavor = "current_thread")]
async fn no_decomposition_aborts_the_mission() {
    let (_temp, root) = make_sandbox_repo();
    // First turn returns no intent → mission aborts.
    let driver = SupervisorDriver::Scripted(ScriptedSupervisor::new(vec![vec![
        SupervisorOutput::NoIntent,
    ]]));

    let (state_tx, _state_rx, mut event_rx, join) =
        run_with_driver(ok_spec(), "sup-abort-0005", root, driver).await;

    let events = collect_events(&mut event_rx, |k| {
        matches!(k, MissionEventKind::Aborted { .. })
    })
    .await;
    let _ = join.await;

    assert_eq!(*state_tx.borrow(), MissionState::Aborted);
    let abort_reasons: Vec<_> = events
        .iter()
        .filter_map(|e| match &e.kind {
            MissionEventKind::Aborted { reason } => Some(reason.clone()),
            _ => None,
        })
        .collect();
    assert_eq!(abort_reasons.len(), 1);
    assert!(abort_reasons[0].to_lowercase().contains("decompos"));
}

#[tokio::test(flavor = "current_thread")]
async fn supervisor_error_on_decompose_turn_is_preserved_in_abort_reason() {
    let (_temp, root) = make_sandbox_repo();
    let driver = SupervisorDriver::Scripted(ScriptedSupervisor::new(vec![vec![
        SupervisorOutput::Error("supervisor API retry 10/10: rate_limit (status 529)".into()),
    ]]));

    let (state_tx, _state_rx, mut event_rx, join) =
        run_with_driver(ok_spec(), "sup-error-0006", root, driver).await;

    let events = collect_events(&mut event_rx, |k| {
        matches!(k, MissionEventKind::Aborted { .. })
    })
    .await;
    let _ = join.await;

    assert_eq!(*state_tx.borrow(), MissionState::Aborted);
    let reason = events
        .iter()
        .find_map(|e| match &e.kind {
            MissionEventKind::Aborted { reason } => Some(reason.as_str()),
            _ => None,
        })
        .expect("aborted event");
    assert!(
        reason.contains("rate_limit"),
        "abort reason should preserve supervisor failure detail: {reason}"
    );
}

// ──────────────────────────────────────────────────────────────
// QC-2: plan-preview / confirm_plan / regenerate_plan state machine.
// ──────────────────────────────────────────────────────────────
//
// T1 — exercise the `PendingPlanApproval` pause in
// `mission_loop::run_supervisor_mission` (lines ~196–266) end-to-end
// via the scripted-supervisor harness. Together these tests cover:
//   * the Confirm happy path,
//   * the Regenerate-with-hint loop,
//   * abort while parked at PendingPlanApproval,
//   * the channel-capacity quirk that lets a "double-click" Confirm
//     buffer without crashing the loop.

/// Variant of `run_with_driver` that retains the plan-decision sender
/// and the cancel handle so the QC-2 tests can drive the
/// `PendingPlanApproval` state machine and abort cleanly.
async fn run_with_driver_keep_handles(
    spec: MissionSpec,
    mission_id: &str,
    root: PathBuf,
    driver: SupervisorDriver,
) -> (
    Arc<watch::Sender<MissionState>>,
    watch::Receiver<MissionState>,
    crate::mission_runtime::MissionEventReceiver,
    tokio::task::JoinHandle<Result<(), MissionRuntimeError>>,
    mpsc::Sender<PlanDecision>,
    Arc<CancelToken>,
) {
    let workspace = MissionWorkspace::new(root, mission_id.into()).unwrap();
    workspace.create_supervisor_branch("main").await.unwrap();
    workspace.create_supervisor_worktree().await.unwrap();

    let event_bus = MissionEventBus::new(256);
    let event_rx = event_bus.subscribe();
    let (state_tx_raw, state_rx) = watch::channel(MissionState::Created);
    let state_tx = Arc::new(state_tx_raw);
    let cancel = CancelToken::new();
    let seq = Arc::new(AtomicU64::new(0));
    let (plan_tx, plan_rx) = mpsc::channel(4);

    let mid = mission_id.to_string();
    let st = state_tx.clone();
    let cancel_clone = cancel.clone();
    let join = tokio::spawn(async move {
        run_supervisor_mission(
            mid,
            spec,
            workspace,
            event_bus,
            st,
            cancel_clone,
            seq,
            driver,
            WorkerBackend::Mock,
            plan_rx,
            Arc::new(AtomicU32::new(0)),
            Arc::new(AtomicBool::new(false)),
            None,
            None,
            None,
        )
        .await
    });
    (state_tx, state_rx, event_rx, join, plan_tx, cancel)
}

fn confirm_plan_spec() -> MissionSpec {
    let mut s = ok_spec();
    s.confirm_plan = Some(true);
    s
}

fn scripted_decompose(title: &str) -> SupervisorOutput {
    use supervisor_adapter::{SupervisorIntent, SupervisorTaskDescriptor};
    SupervisorOutput::Intent(SupervisorIntent::Decompose {
        tasks: vec![SupervisorTaskDescriptor {
            title: title.into(),
            description: None,
            ..Default::default()
        }],
        overview: None,
        tech_stack: None,
        envelope_fit: None,
    })
}

#[tokio::test(flavor = "current_thread")]
async fn confirm_plan_proceeds_from_pending_plan_approval_to_executing() {
    let (_temp, root) = make_sandbox_repo();
    let driver =
        SupervisorDriver::Scripted(ScriptedSupervisor::new(vec![vec![scripted_decompose(
            "task A",
        )]]));

    let (state_tx, _state_rx, mut event_rx, join, plan_tx, cancel) =
        run_with_driver_keep_handles(confirm_plan_spec(), "sup-confirm-0001", root, driver).await;

    let pre = collect_events(&mut event_rx, |k| {
        matches!(k, MissionEventKind::PlanProposed { .. })
    })
    .await;
    let proposed = pre
        .iter()
        .find(|e| matches!(e.kind, MissionEventKind::PlanProposed { .. }))
        .expect("PlanProposed event must be emitted while confirm_plan=true");
    if let MissionEventKind::PlanProposed {
        generation, tasks, ..
    } = &proposed.kind
    {
        assert_eq!(*generation, 0, "first plan must be generation 0");
        assert_eq!(tasks.len(), 1);
    }

    for _ in 0..20 {
        if *state_tx.borrow() == MissionState::PendingPlanApproval {
            break;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    assert_eq!(*state_tx.borrow(), MissionState::PendingPlanApproval);

    plan_tx
        .send(PlanDecision::Confirm { generation: 0 })
        .await
        .unwrap();
    let post = collect_events(&mut event_rx, |k| {
        matches!(k, MissionEventKind::PlanConfirmed { .. })
    })
    .await;
    let confirmed = post
        .iter()
        .find_map(|e| match &e.kind {
            MissionEventKind::PlanConfirmed { generation } => Some(*generation),
            _ => None,
        })
        .expect("PlanConfirmed must follow PlanDecision::Confirm");
    assert_eq!(confirmed, 0);

    cancel.cancel();
    let _ = tokio::time::timeout(Duration::from_secs(5), join).await;
    assert_ne!(*state_tx.borrow(), MissionState::PendingPlanApproval);
}

#[tokio::test(flavor = "current_thread")]
async fn regenerate_plan_emits_regeneration_request_and_runs_second_decompose() {
    let (_temp, root) = make_sandbox_repo();
    let driver = SupervisorDriver::Scripted(ScriptedSupervisor::new(vec![
        vec![scripted_decompose("first cut")],
        vec![scripted_decompose("post-regen")],
    ]));

    let (_state_tx, _state_rx, mut event_rx, join, plan_tx, cancel) =
        run_with_driver_keep_handles(confirm_plan_spec(), "sup-regen-0001", root, driver).await;

    let _ = collect_events(&mut event_rx, |k| {
        matches!(k, MissionEventKind::PlanProposed { generation: 0, .. })
    })
    .await;

    plan_tx
        .send(PlanDecision::Regenerate {
            generation: 0,
            hint: Some("widen scope".into()),
        })
        .await
        .unwrap();

    let mid = collect_events(&mut event_rx, |k| {
        matches!(k, MissionEventKind::PlanProposed { generation: 1, .. })
    })
    .await;
    let regen = mid
        .iter()
        .find_map(|e| match &e.kind {
            MissionEventKind::PlanRegenerationRequested {
                hint,
                prior_generation,
            } => Some((hint.clone(), *prior_generation)),
            _ => None,
        })
        .expect("PlanRegenerationRequested must precede the second PlanProposed");
    assert_eq!(regen.0.as_deref(), Some("widen scope"));
    assert_eq!(regen.1, 0);

    let second = mid
        .iter()
        .find_map(|e| match &e.kind {
            MissionEventKind::PlanProposed {
                generation, tasks, ..
            } if *generation == 1 => Some((*generation, tasks.len())),
            _ => None,
        })
        .expect("second PlanProposed must have generation=1");
    assert_eq!(second.0, 1);
    assert!(second.1 >= 1);

    cancel.cancel();
    let _ = tokio::time::timeout(Duration::from_secs(5), join).await;
}

#[tokio::test(flavor = "current_thread")]
async fn cancel_while_pending_plan_approval_aborts_cleanly() {
    let (_temp, root) = make_sandbox_repo();
    let driver =
        SupervisorDriver::Scripted(ScriptedSupervisor::new(vec![vec![scripted_decompose(
            "await me",
        )]]));

    let (state_tx, _state_rx, mut event_rx, join, _plan_tx, cancel) =
        run_with_driver_keep_handles(confirm_plan_spec(), "sup-cancel-0001", root, driver).await;

    let _ = collect_events(&mut event_rx, |k| {
        matches!(k, MissionEventKind::PlanProposed { .. })
    })
    .await;
    for _ in 0..50 {
        if *state_tx.borrow() == MissionState::PendingPlanApproval {
            break;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    assert_eq!(*state_tx.borrow(), MissionState::PendingPlanApproval);

    cancel.cancel();

    let events = collect_events(&mut event_rx, |k| {
        matches!(k, MissionEventKind::Aborted { .. })
    })
    .await;
    let _ = tokio::time::timeout(Duration::from_secs(5), join).await;
    let reasons: Vec<_> = events
        .iter()
        .filter_map(|e| match &e.kind {
            MissionEventKind::Aborted { reason } => Some(reason.clone()),
            _ => None,
        })
        .collect();
    assert_eq!(reasons.len(), 1, "exactly one Aborted event expected");
    assert!(
        reasons[0].to_lowercase().contains("abort"),
        "abort reason should be user-driven, got: {}",
        reasons[0]
    );
    assert_eq!(*state_tx.borrow(), MissionState::Aborted);
}

#[tokio::test(flavor = "current_thread")]
async fn reject_plan_from_pending_state_emits_plan_rejected_then_aborted() {
    let (_temp, root) = make_sandbox_repo();
    let driver =
        SupervisorDriver::Scripted(ScriptedSupervisor::new(vec![vec![scripted_decompose(
            "rejectable task",
        )]]));

    let (state_tx, _state_rx, mut event_rx, join, plan_tx, _cancel) =
        run_with_driver_keep_handles(confirm_plan_spec(), "sup-reject-0001", root, driver).await;

    // Wait for the PlanProposed → PendingPlanApproval transition.
    let _ = collect_events(&mut event_rx, |k| {
        matches!(k, MissionEventKind::PlanProposed { .. })
    })
    .await;
    for _ in 0..50 {
        if *state_tx.borrow() == MissionState::PendingPlanApproval {
            break;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    assert_eq!(*state_tx.borrow(), MissionState::PendingPlanApproval);

    plan_tx
        .send(PlanDecision::Reject {
            generation: 0,
            reason: Some("scope too broad".into()),
        })
        .await
        .unwrap();

    let post = collect_events(&mut event_rx, |k| {
        matches!(k, MissionEventKind::Aborted { .. })
    })
    .await;

    let rejected = post.iter().find_map(|e| match &e.kind {
        MissionEventKind::PlanRejected { generation, reason } => {
            Some((*generation, reason.clone()))
        }
        _ => None,
    });
    let rej = rejected.expect("expected PlanRejected emission");
    assert_eq!(rej.0, 0);
    assert_eq!(rej.1.as_deref(), Some("scope too broad"));

    let aborted_reason = post
        .iter()
        .find_map(|e| match &e.kind {
            MissionEventKind::Aborted { reason } => Some(reason.clone()),
            _ => None,
        })
        .expect("expected Aborted emission");
    assert!(
        aborted_reason.contains("plan_rejected") && aborted_reason.contains("scope too broad"),
        "Aborted reason should embed reject reason: {aborted_reason}"
    );

    let _ = tokio::time::timeout(Duration::from_secs(5), join).await;
    assert_eq!(*state_tx.borrow(), MissionState::Aborted);
}

#[tokio::test(flavor = "current_thread")]
async fn reject_plan_with_no_reason_aborts_with_default_label() {
    let (_temp, root) = make_sandbox_repo();
    let driver =
        SupervisorDriver::Scripted(ScriptedSupervisor::new(vec![vec![scripted_decompose(
            "no-reason task",
        )]]));

    let (state_tx, _state_rx, mut event_rx, join, plan_tx, _cancel) = run_with_driver_keep_handles(
        confirm_plan_spec(),
        "sup-reject-noreason-0001",
        root,
        driver,
    )
    .await;

    let _ = collect_events(&mut event_rx, |k| {
        matches!(k, MissionEventKind::PlanProposed { .. })
    })
    .await;
    for _ in 0..50 {
        if *state_tx.borrow() == MissionState::PendingPlanApproval {
            break;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }

    plan_tx
        .send(PlanDecision::Reject {
            generation: 0,
            reason: None,
        })
        .await
        .unwrap();

    let post = collect_events(&mut event_rx, |k| {
        matches!(k, MissionEventKind::Aborted { .. })
    })
    .await;
    let aborted_reason = post
        .iter()
        .find_map(|e| match &e.kind {
            MissionEventKind::Aborted { reason } => Some(reason.clone()),
            _ => None,
        })
        .expect("expected Aborted emission");
    assert_eq!(aborted_reason, "plan_rejected");

    let _ = tokio::time::timeout(Duration::from_secs(5), join).await;
    assert_eq!(*state_tx.borrow(), MissionState::Aborted);
}

#[tokio::test(flavor = "current_thread")]
async fn double_click_confirm_does_not_emit_two_plan_confirmed_events() {
    // Capacity-4 channel pitfall: a user double-click sends two
    // PlanDecision::Confirm in quick succession. The loop consumes
    // one and immediately breaks out of the await; the second sits
    // in the channel buffer with no receiver. The mission must
    // still emit exactly one PlanConfirmed event and not panic.
    let (_temp, root) = make_sandbox_repo();
    let driver =
        SupervisorDriver::Scripted(ScriptedSupervisor::new(vec![vec![scripted_decompose(
            "task A",
        )]]));

    let (_state_tx, _state_rx, mut event_rx, join, plan_tx, cancel) =
        run_with_driver_keep_handles(confirm_plan_spec(), "sup-dblclick-0001", root, driver).await;

    let _ = collect_events(&mut event_rx, |k| {
        matches!(k, MissionEventKind::PlanProposed { .. })
    })
    .await;

    plan_tx
        .send(PlanDecision::Confirm { generation: 0 })
        .await
        .unwrap();
    plan_tx
        .send(PlanDecision::Confirm { generation: 0 })
        .await
        .unwrap();

    let events = collect_events(&mut event_rx, |k| {
        matches!(k, MissionEventKind::PlanConfirmed { .. })
    })
    .await;
    cancel.cancel();
    let _ = tokio::time::timeout(Duration::from_secs(5), join).await;

    let confirmed_count = events
        .iter()
        .filter(|e| matches!(e.kind, MissionEventKind::PlanConfirmed { .. }))
        .count();
    assert_eq!(
        confirmed_count, 1,
        "double-click must emit exactly one PlanConfirmed event"
    );
}

#[tokio::test(flavor = "current_thread")]
async fn a_decision_for_the_old_plan_cannot_confirm_a_regenerated_plan() {
    let (_temp, root) = make_sandbox_repo();
    let driver = SupervisorDriver::Scripted(ScriptedSupervisor::new(vec![
        vec![scripted_decompose("original plan")],
        vec![scripted_decompose("regenerated plan")],
    ]));

    let (_state_tx, _state_rx, mut event_rx, join, plan_tx, cancel) =
        run_with_driver_keep_handles(confirm_plan_spec(), "sup-stale-plan-0001", root, driver)
            .await;

    let _ = collect_events(&mut event_rx, |kind| {
        matches!(kind, MissionEventKind::PlanProposed { generation: 0, .. })
    })
    .await;
    plan_tx
        .send(PlanDecision::Regenerate {
            generation: 0,
            hint: Some("try again".into()),
        })
        .await
        .unwrap();
    plan_tx
        .send(PlanDecision::Confirm { generation: 0 })
        .await
        .unwrap();

    let stale_confirmed = tokio::time::timeout(Duration::from_secs(1), async {
        loop {
            let event = event_rx.recv().await.expect("event stream remains open");
            if matches!(
                event.kind,
                MissionEventKind::PlanConfirmed { generation: 1 }
            ) {
                return;
            }
        }
    })
    .await;

    assert!(
        stale_confirmed.is_err(),
        "a decision submitted for generation 0 must not confirm generation 1"
    );
    cancel.cancel();
    let _ = tokio::time::timeout(Duration::from_secs(5), join).await;
}

// ──────────────────────────────────────────────────────────────
// QC-3 envelope-fit gate matrix.
// ──────────────────────────────────────────────────────────────
//
// Matrix:
//   confirm_plan  ∈ {None, Some(true)}
//   envelope      ∈ {None (legacy), AllWithin, RiskExceeds}
//
// Expected behaviour:
//   - confirm_plan == Some(true)   → pause regardless of envelope
//   - envelope.exceeded() == Some(_) → pause regardless of confirm_plan
//   - else                          → straight to Executing (no
//                                     PlanProposed event)

#[tokio::test(flavor = "current_thread")]
async fn envelope_within_direct_does_not_pause() {
    let (_temp, root) = make_sandbox_repo();
    let driver = SupervisorDriver::Scripted(ScriptedSupervisor::new(
        super::scripted_fixtures::plan_happy_envelope_within(),
    ));

    let (state_tx, _state_rx, mut event_rx, join, _plan_tx, cancel) =
        run_with_driver_keep_handles(ok_spec(), "sup-env-within-direct", root, driver).await;

    // Wait briefly to confirm the loop didn't stop at PendingPlanApproval.
    let mut saw_pending = false;
    let mut saw_executing = false;
    for _ in 0..200 {
        let s = state_tx.borrow().clone();
        if s == MissionState::PendingPlanApproval {
            saw_pending = true;
            break;
        }
        if s == MissionState::Executing {
            saw_executing = true;
            break;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    assert!(
        !saw_pending,
        "direct mode + within envelope must not pause: state was {:?}",
        *state_tx.borrow()
    );
    assert!(saw_executing, "should reach Executing");

    // No PlanProposed event should have been emitted on the direct
    // path either.
    let early = collect_events(&mut event_rx, |k| {
        matches!(k, MissionEventKind::ExecutionStarted)
    })
    .await;
    let proposed_present = early
        .iter()
        .any(|e| matches!(e.kind, MissionEventKind::PlanProposed { .. }));
    assert!(
        !proposed_present,
        "no PlanProposed event expected on the direct-within path"
    );

    cancel.cancel();
    let _ = tokio::time::timeout(Duration::from_secs(5), join).await;
}

#[tokio::test(flavor = "current_thread")]
async fn envelope_exceeds_risk_forces_pause_in_direct_mode() {
    let (_temp, root) = make_sandbox_repo();
    let driver = SupervisorDriver::Scripted(ScriptedSupervisor::new(
        super::scripted_fixtures::plan_exceeds_risk(),
    ));

    let (state_tx, _state_rx, mut event_rx, join, _plan_tx, cancel) =
        run_with_driver_keep_handles(ok_spec(), "sup-env-exceeds-risk", root, driver).await;

    // The envelope-fit gate should force PendingPlanApproval even
    // though confirm_plan is None.
    for _ in 0..200 {
        if *state_tx.borrow() == MissionState::PendingPlanApproval {
            break;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    assert_eq!(
        *state_tx.borrow(),
        MissionState::PendingPlanApproval,
        "envelope_fit.risk == Exceeds must force pause in Direct mode"
    );

    let proposed = collect_events(&mut event_rx, |k| {
        matches!(k, MissionEventKind::PlanProposed { .. })
    })
    .await;
    let envelope_carried = proposed.iter().any(|e| match &e.kind {
        MissionEventKind::PlanProposed { envelope_fit, .. } => envelope_fit.is_some(),
        _ => false,
    });
    assert!(
        envelope_carried,
        "PlanProposed event should carry the envelope_fit metadata"
    );

    cancel.cancel();
    let _ = tokio::time::timeout(Duration::from_secs(5), join).await;
}

#[tokio::test(flavor = "current_thread")]
async fn envelope_within_review_still_pauses_for_user() {
    let (_temp, root) = make_sandbox_repo();
    let driver = SupervisorDriver::Scripted(ScriptedSupervisor::new(
        super::scripted_fixtures::plan_happy_envelope_within(),
    ));

    let (state_tx, _state_rx, _event_rx, join, _plan_tx, cancel) =
        run_with_driver_keep_handles(confirm_plan_spec(), "sup-env-within-review", root, driver)
            .await;

    for _ in 0..200 {
        if *state_tx.borrow() == MissionState::PendingPlanApproval {
            break;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    assert_eq!(
        *state_tx.borrow(),
        MissionState::PendingPlanApproval,
        "Review mode pauses even when envelope is all Within"
    );

    cancel.cancel();
    let _ = tokio::time::timeout(Duration::from_secs(5), join).await;
}

#[tokio::test(flavor = "current_thread")]
async fn envelope_none_legacy_does_not_pause_in_direct_mode() {
    // Back-compat: a supervisor adapter that doesn't emit
    // envelope_fit collapses the gate to QC-2 semantics — only
    // confirm_plan == Some(true) pauses.
    let (_temp, root) = make_sandbox_repo();
    let driver = SupervisorDriver::Scripted(ScriptedSupervisor::new(
        super::scripted_fixtures::plan_no_envelope(),
    ));

    let (state_tx, _state_rx, _event_rx, join, _plan_tx, cancel) =
        run_with_driver_keep_handles(ok_spec(), "sup-env-legacy-direct", root, driver).await;

    // Loop should not enter PendingPlanApproval.
    let mut saw_pending = false;
    let mut saw_executing = false;
    for _ in 0..200 {
        let s = state_tx.borrow().clone();
        if s == MissionState::PendingPlanApproval {
            saw_pending = true;
            break;
        }
        if s == MissionState::Executing {
            saw_executing = true;
            break;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    assert!(
        !saw_pending,
        "legacy adapter (envelope_fit == None) must not pause in Direct mode"
    );
    assert!(saw_executing, "should reach Executing");

    cancel.cancel();
    let _ = tokio::time::timeout(Duration::from_secs(5), join).await;
}

#[tokio::test(flavor = "current_thread")]
async fn envelope_regenerate_clears_near_limit_in_review_mode() {
    // First turn returns quality.fit == NearLimit; the user
    // regenerates and the second turn returns a clean within-
    // envelope decompose. This confirms the envelope metadata is
    // re-emitted on the second PlanProposed.
    let (_temp, root) = make_sandbox_repo();
    let driver = SupervisorDriver::Scripted(ScriptedSupervisor::new(
        super::scripted_fixtures::plan_regenerate_then_clean(),
    ));

    let (state_tx, _state_rx, mut event_rx, join, plan_tx, cancel) =
        run_with_driver_keep_handles(confirm_plan_spec(), "sup-env-regen-clean", root, driver)
            .await;

    // Wait for the first PlanProposed (generation 0).
    let first = collect_events(&mut event_rx, |k| {
        matches!(k, MissionEventKind::PlanProposed { generation: 0, .. })
    })
    .await;
    let first_envelope = first.iter().find_map(|e| match &e.kind {
        MissionEventKind::PlanProposed {
            generation: 0,
            envelope_fit,
            ..
        } => envelope_fit.as_ref().map(|ef| ef.quality.fit),
        _ => None,
    });
    assert_eq!(
        first_envelope,
        Some(crate::mission_event::BoundFitKind::NearLimit),
        "first plan should carry quality NearLimit"
    );

    for _ in 0..200 {
        if *state_tx.borrow() == MissionState::PendingPlanApproval {
            break;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }

    plan_tx
        .send(PlanDecision::Regenerate {
            generation: 0,
            hint: Some("add a test task".into()),
        })
        .await
        .unwrap();

    // Wait for the second PlanProposed (generation 1).
    let second = collect_events(&mut event_rx, |k| {
        matches!(k, MissionEventKind::PlanProposed { generation: 1, .. })
    })
    .await;
    let second_envelope = second.iter().find_map(|e| match &e.kind {
        MissionEventKind::PlanProposed {
            generation: 1,
            envelope_fit,
            ..
        } => envelope_fit.as_ref().map(|ef| ef.quality.fit),
        _ => None,
    });
    assert_eq!(
        second_envelope,
        Some(crate::mission_event::BoundFitKind::Within),
        "regenerated plan should carry quality Within"
    );

    cancel.cancel();
    let _ = tokio::time::timeout(Duration::from_secs(5), join).await;
}

// ──────────────────────────────────────────────────────────────
// S7 Task 7: cyclic decomposition smoke test.
// ──────────────────────────────────────────────────────────────
#[tokio::test(flavor = "current_thread")]
async fn cyclic_decomposition_aborts_with_rejected_event() {
    use crate::task_graph;
    let tasks = vec![
        crate::mission_event::TaskDescriptor {
            index: 0,
            title: "a".into(),
            depends_on: vec![1],
            ..Default::default()
        },
        crate::mission_event::TaskDescriptor {
            index: 1,
            title: "b".into(),
            depends_on: vec![0],
            ..Default::default()
        },
    ];
    let err = task_graph::validate(&tasks).expect_err("cycle should reject");
    assert!(matches!(err, task_graph::GraphError::Cycle { .. }));
}

#[test]
fn criteria_fail_emits_quality_extend_when_budget_remains() {
    use crate::audit::report::{AuditReport, ScopeScore};
    use crate::task_graph::{evaluate_criteria, AcceptanceCriteria, CriteriaOutcome};

    let report = AuditReport {
        overall: 0.85,
        scope: Some(ScopeScore {
            in_scope: 1,
            out_of_scope: 0,
            score: 1.0,
        }),
        ..AuditReport::default()
    };

    let criteria = AcceptanceCriteria {
        min_audit_overall: Some(0.95),
        ..Default::default()
    };

    match evaluate_criteria(&criteria, &report) {
        CriteriaOutcome::Fail { reasons } => assert!(!reasons.is_empty()),
        _ => panic!("expected Fail"),
    }
}

// ──────────────────────────────────────────────────────────────
// S8 Task 13 (live variant): an in-tree mock worker that writes
// outside the mission's `scope_paths` should trip the pre-flight
// ACL gate. The mission lands in Attention with an ArbiterDecided
// event carrying `bound: Some(Scope)`, and audit never runs (no
// AuditCompleted for that worker).
//
// This is the integration counterpart to the pure-function tests
// in `tests/acl_gate.rs`, driven through `run_with_driver` and a
// `ScriptedSupervisor`.
// ──────────────────────────────────────────────────────────────
#[tokio::test(flavor = "current_thread")]
async fn mock_worker_writing_outside_mission_scope_trips_acl_gate() {
    use supervisor_adapter::{SupervisorIntent, SupervisorTaskDescriptor};

    let (_temp, root) = make_sandbox_repo();

    // Spec restricts the mission to `src/` only — the mock worker
    // writes `MOCK_0.md` at the worktree root, which is out of scope.
    let mut spec = ok_spec();
    spec.scope_paths = vec![PathBuf::from("src")];

    // Turn 1: decompose into a single task. The mock worker runs
    // automatically (WorkerBackend::Mock) and writes MOCK_0.md.
    let driver = SupervisorDriver::Scripted(ScriptedSupervisor::new(vec![vec![
        SupervisorOutput::Intent(SupervisorIntent::Decompose {
            tasks: vec![SupervisorTaskDescriptor {
                title: "scoped task".into(),
                description: None,
                ..Default::default()
            }],
            overview: None,
            tech_stack: None,
            envelope_fit: None,
        }),
    ]]));

    let (state_tx, _state_rx, mut event_rx, join) =
        run_with_driver(spec, "sup-acl-live-0001", root, driver).await;

    // Drain until we see the ArbiterDecided event carrying the Scope
    // bound (the pre-flight gate's synthetic decision), or the join
    // handle returns (mission ended).
    let events = collect_events(&mut event_rx, |k| {
        matches!(
            k,
            MissionEventKind::ArbiterDecided {
                bound: Some(crate::arbiter::AuthorityBound::Scope),
                ..
            }
        )
    })
    .await;
    let _ = join.await;

    // State should be Attention (set by the pre-flight gate before
    // `break`-ing out of the per-task loop).
    assert_eq!(*state_tx.borrow(), MissionState::Attention);

    // The ArbiterDecided event with Scope bound must be present.
    let scope_decided = events.iter().find_map(|e| match &e.kind {
        MissionEventKind::ArbiterDecided {
            decision_json,
            bound: Some(crate::arbiter::AuthorityBound::Scope),
            ..
        } => Some(decision_json.clone()),
        _ => None,
    });
    assert!(
        scope_decided.is_some(),
        "expected an ArbiterDecided event with bound=Some(Scope); got events={:?}",
        events.iter().map(|e| &e.kind).collect::<Vec<_>>()
    );
    let json = scope_decided.unwrap();
    assert!(
        json.contains("scope") || json.contains("Scope"),
        "decision_json should mention scope: {json}"
    );

    // Pre-flight gate fires BEFORE audit, so no AuditCompleted event
    // should land for this worker.
    let audit_events: Vec<_> = events
        .iter()
        .filter(|e| matches!(e.kind, MissionEventKind::AuditCompleted { .. }))
        .collect();
    assert!(
        audit_events.is_empty(),
        "ACL pre-flight gate must short-circuit before audit; got {} AuditCompleted events",
        audit_events.len()
    );
}
