//! S7 / U8 acceptance test: a 4-task decomposition with a
//! `depends_on` chain runs through the parallel scheduler and emits
//! events in a topologically-consistent order.
//!
//! Uses the [`ScriptedSupervisor`] driver + mock worker backend so
//! the test is hermetic — no real CLI invocations; the temp-repo
//! workspace is the only filesystem surface.
//!
//! ## What this test pins
//!
//! 1. **Spawn topology** — task 0 (root) spawns before tasks 1 (mid-a)
//!    and 2 (mid-b); 1 and 2 both spawn before task 3 (join).
//! 2. **Integration topology** — every task integrates after every
//!    one of its `depends_on` predecessors integrates. Integration is
//!    serialised under `TaskRunCtx::integration_lock`; even when two
//!    workers complete their pass in parallel, their `Integrated`
//!    events fan out in lock-respecting order, and the order still
//!    respects topology because the join can't start until both mids
//!    complete.
//! 3. **Parallel window** — task 1 and task 2 share the same
//!    indegree-0 ready set after task 0 completes. The scheduler
//!    dispatches them concurrently; the second spawn lands before
//!    either integrates. Without parallelism this would collapse to a
//!    sequential `spawn(1) → integrated(1) → spawn(2) → integrated(2)`
//!    order.
//!
//! Plus a second test that feeds a 2-task cycle and confirms the
//! mission emits `DecompositionRejected` and reaches `Aborted` —
//! the upstream guard that prevents the JoinSet from ever dispatching
//! a cyclic graph.

use orchestrator::mission::{MissionSpec, MissionState};
use orchestrator::mission_event::MissionEventKind;
use orchestrator::mission_runtime::MissionRuntime;
use orchestrator::mission_supervisor_run::{ScriptedSupervisor, SupervisorDriver, WorkerBackend};
use orchestrator::mission_workspace::MissionWorkspace;
use std::process::Command as SyncCommand;
use std::time::{Duration, Instant};
use supervisor_adapter::{SupervisorIntent, SupervisorOutput, SupervisorTaskDescriptor};
use tempfile::TempDir;

/// Init a tempdir with `git init --initial-branch=main` plus a single
/// initial commit on `README.md`. The MissionWorkspace requires a
/// clean repo to create supervisor / worker branches off of.
///
/// Returns the TempDir guard (drop = cleanup) and not just the path
/// so the caller controls lifetime.
fn init_temp_repo() -> TempDir {
    let dir = tempfile::tempdir().expect("tempdir");
    let path = dir.path().to_path_buf();
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
    run(&["config", "user.email", "test@vigla.local"]);
    run(&["config", "user.name", "vigla-test"]);
    run(&["config", "commit.gpgsign", "false"]);
    std::fs::write(path.join("README.md"), "hello\n").expect("write README");
    run(&["add", "README.md"]);
    run(&["commit", "-m", "initial"]);
    dir
}

fn td(_index: u32, title: &str, deps: Vec<u32>) -> SupervisorTaskDescriptor {
    SupervisorTaskDescriptor {
        title: title.into(),
        description: None,
        depends_on: deps,
        scope_paths: Vec::new(),
    }
}

fn ok_spec(title: &str, mission_objective: &str) -> MissionSpec {
    // No Cargo.toml in the fixture → audit's project detector returns
    // ProjectType::None → test_pass / lint subscores abstain → only
    // scope + security run. Empty scope_paths means everything is in
    // scope, so scope's perfect score blends to overall = 1.0 (well
    // above the default 0.7 floor). This keeps the arbiter on the
    // Accept path for every task — the parallel scheduler is the unit
    // under test, not the rework loop.
    MissionSpec {
        title: title.into(),
        objective: mission_objective.into(),
        target_ref: "main".into(),
        tests: None,
        supervisor_model: Some("claude".into()),
        worker_model: None,
        worker_count: Some(4),
        confirm_plan: None,
        scope_paths: Vec::new(),
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn parallel_mission_with_depends_on_chain_respects_topology() {
    let repo = init_temp_repo();
    let mission_id = "mid-parallel-diamond-0001";

    // Decomposition: root → 2 mid → join. Cap (4) is large enough
    // that all ready tasks dispatch immediately when their predecessors
    // complete.
    let tasks = vec![
        td(0, "root", vec![]),
        td(1, "mid-a", vec![0]),
        td(2, "mid-b", vec![0]),
        td(3, "join", vec![1, 2]),
    ];

    // Scripted supervisor turns:
    //   1. Decompose (4 tasks).
    //   2. DeclareComplete (mission summary).
    // Audit passes for every mock submission (no project detected →
    // scope-only blend = 1.0), so the per-pass review turn never
    // fires, and the supervisor only sees these two turns.
    let turns: Vec<Vec<SupervisorOutput>> = vec![
        vec![SupervisorOutput::Intent(SupervisorIntent::Decompose {
            tasks,
            overview: None,
            tech_stack: None,
            envelope_fit: None,
        })],
        vec![SupervisorOutput::Intent(
            SupervisorIntent::DeclareComplete {
                summary: "4-task diamond complete".into(),
            },
        )],
    ];
    let driver = SupervisorDriver::Scripted(ScriptedSupervisor::new(turns));

    let workspace =
        MissionWorkspace::new(repo.path().to_path_buf(), mission_id.into()).expect("workspace");

    let runtime = MissionRuntime::start_supervised_with(
        ok_spec(
            "parallel diamond",
            "test the parallel scheduler with a diamond DAG",
        ),
        workspace,
        driver,
        WorkerBackend::Mock,
    )
    .await
    .expect("start_supervised");

    // ── Drain events until terminal or timeout ─────────────────────
    let mut rx = runtime.subscribe();
    let mut events: Vec<MissionEventKind> = Vec::new();
    let deadline = Instant::now() + Duration::from_secs(30);
    loop {
        let remaining = deadline
            .checked_duration_since(Instant::now())
            .unwrap_or_default();
        if remaining.is_zero() {
            panic!("mission did not terminate within deadline; events so far: {events:?}");
        }
        match tokio::time::timeout(remaining, rx.recv()).await {
            Ok(Ok(e)) => {
                let terminal = matches!(
                    e.kind,
                    MissionEventKind::Completed { .. } | MissionEventKind::Aborted { .. }
                );
                events.push(e.kind);
                if terminal {
                    break;
                }
            }
            Ok(Err(_)) => break, // sender dropped
            Err(_) => break,     // timeout
        }
    }
    // NOTE: Do NOT call `runtime.abort()` here. `abort()` waits for
    // the mission to transition to `Aborted`, but a normally-completed
    // mission sits at `CompletePendingMerge` and never reaches
    // `Aborted` — abort would block indefinitely. We rely on `runtime`
    // being dropped at end-of-scope (which is non-blocking).

    let final_state = runtime.state();
    assert!(
        matches!(final_state, MissionState::CompletePendingMerge),
        "mission should reach CompletePendingMerge; got {final_state:?}; events={events:?}"
    );

    // ── Collect ordering signal: WorkerSpawned + Integrated by index
    let pos_spawn: std::collections::BTreeMap<u32, usize> = events
        .iter()
        .enumerate()
        .filter_map(|(i, e)| match e {
            MissionEventKind::WorkerSpawned { task_index, .. } => Some((*task_index, i)),
            _ => None,
        })
        .collect();
    let pos_integrated: std::collections::BTreeMap<u32, usize> = events
        .iter()
        .enumerate()
        .filter_map(|(i, e)| match e {
            MissionEventKind::Integrated { worker_id, .. } => {
                // worker_id format from run_task: "mock-{task.index + 1}".
                let n: u32 = worker_id.trim_start_matches("mock-").parse().ok()?;
                Some((n.saturating_sub(1), i))
            }
            _ => None,
        })
        .collect();

    assert_eq!(
        pos_spawn.len(),
        4,
        "all 4 tasks must spawn; got {pos_spawn:?}"
    );
    assert_eq!(
        pos_integrated.len(),
        4,
        "all 4 tasks must integrate; got {pos_integrated:?}"
    );

    let spawn = |idx: u32| pos_spawn[&idx];
    let done = |idx: u32| pos_integrated[&idx];

    // (a) Spawn topology — root before mids before join.
    assert!(spawn(0) < spawn(1), "spawn(0) < spawn(1)");
    assert!(spawn(0) < spawn(2), "spawn(0) < spawn(2)");
    assert!(spawn(0) < spawn(3), "spawn(0) < spawn(3)");
    assert!(spawn(1) < spawn(3), "spawn(1) < spawn(3)");
    assert!(spawn(2) < spawn(3), "spawn(2) < spawn(3)");

    // (b) Predecessor must INTEGRATE before successor spawns.
    assert!(done(0) < spawn(1), "integrated(0) precedes spawn(1)");
    assert!(done(0) < spawn(2), "integrated(0) precedes spawn(2)");
    assert!(done(1) < spawn(3), "integrated(1) precedes spawn(3)");
    assert!(done(2) < spawn(3), "integrated(2) precedes spawn(3)");

    // (c) Integration order respects topology.
    assert!(done(0) < done(1), "integrated(0) < integrated(1)");
    assert!(done(0) < done(2), "integrated(0) < integrated(2)");
    assert!(done(1) < done(3), "integrated(1) < integrated(3)");
    assert!(done(2) < done(3), "integrated(2) < integrated(3)");

    // (d) Parallel window — mid-a and mid-b dispatch in the same
    // ready set. The later WorkerSpawned of the two lands before
    // either integrates. Without parallelism, the sequential pattern
    // would be `spawn(1) < done(1) < spawn(2) < done(2)`, making
    // `max(spawn(1), spawn(2))` strictly greater than `min(done(1),
    // done(2))`. The JoinSet collapses this to overlapping windows.
    let later_spawn = std::cmp::max(spawn(1), spawn(2));
    let earlier_int = std::cmp::min(done(1), done(2));
    assert!(
        later_spawn < earlier_int,
        "mid-a and mid-b must spawn inside a parallel window: \
         later_spawn={later_spawn}, earlier_int={earlier_int}; events={events:?}"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn single_task_mission_completes_via_new_dispatcher() {
    // Regression guard for the 1-task common path under the new
    // `JoinSet`-based dispatcher. The diamond test verifies the
    // parallel topology; this verifies the dispatcher still collapses
    // correctly when only one task is ever ready at a time.
    let repo = init_temp_repo();
    let mission_id = "mid-single-0001";
    let tasks = vec![td(0, "only task", vec![])];
    let turns: Vec<Vec<SupervisorOutput>> = vec![
        vec![SupervisorOutput::Intent(SupervisorIntent::Decompose {
            tasks,
            overview: None,
            tech_stack: None,
            envelope_fit: None,
        })],
        vec![SupervisorOutput::Intent(
            SupervisorIntent::DeclareComplete {
                summary: "1 task complete".into(),
            },
        )],
    ];
    let driver = SupervisorDriver::Scripted(ScriptedSupervisor::new(turns));
    let workspace =
        MissionWorkspace::new(repo.path().to_path_buf(), mission_id.into()).expect("workspace");
    let runtime = MissionRuntime::start_supervised_with(
        ok_spec("single", "single-task sanity"),
        workspace,
        driver,
        WorkerBackend::Mock,
    )
    .await
    .expect("start_supervised");

    let mut rx = runtime.subscribe();
    let deadline = Instant::now() + Duration::from_secs(15);
    let mut seen_completed = false;
    while Instant::now() < deadline && !seen_completed {
        let remaining = deadline
            .checked_duration_since(Instant::now())
            .unwrap_or_default();
        match tokio::time::timeout(remaining, rx.recv()).await {
            Ok(Ok(e)) => {
                if matches!(e.kind, MissionEventKind::Completed { .. }) {
                    seen_completed = true;
                }
            }
            _ => break,
        }
    }
    assert!(seen_completed, "single-task mission must reach Completed");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn cyclic_decomposition_aborts_mission() {
    let repo = init_temp_repo();
    let mission_id = "mid-cyclic-0001";

    // 2-task back-edge cycle: 0 → 1 → 0. `task_graph::validate`
    // rejects this with `GraphError::Cycle { involved: [0, 1] }`
    // before any worker spawns; the mission aborts with a
    // `DecompositionRejected` event in its timeline.
    let tasks = vec![td(0, "a", vec![1]), td(1, "b", vec![0])];
    let turns: Vec<Vec<SupervisorOutput>> = vec![vec![SupervisorOutput::Intent(
        SupervisorIntent::Decompose {
            tasks,
            overview: None,
            tech_stack: None,
            envelope_fit: None,
        },
    )]];
    let driver = SupervisorDriver::Scripted(ScriptedSupervisor::new(turns));

    let workspace =
        MissionWorkspace::new(repo.path().to_path_buf(), mission_id.into()).expect("workspace");
    let runtime = MissionRuntime::start_supervised_with(
        ok_spec("cyclic", "trigger DAG rejection"),
        workspace,
        driver,
        WorkerBackend::Mock,
    )
    .await
    .expect("start_supervised");

    let mut rx = runtime.subscribe();
    let deadline = Instant::now() + Duration::from_secs(8);
    let mut seen_rejected = false;
    let mut seen_aborted = false;
    while Instant::now() < deadline && !(seen_rejected && seen_aborted) {
        let remaining = deadline
            .checked_duration_since(Instant::now())
            .unwrap_or_default();
        if remaining.is_zero() {
            break;
        }
        match tokio::time::timeout(remaining, rx.recv()).await {
            Ok(Ok(e)) => {
                if matches!(e.kind, MissionEventKind::DecompositionRejected { .. }) {
                    seen_rejected = true;
                }
                if matches!(e.kind, MissionEventKind::Aborted { .. }) {
                    seen_aborted = true;
                }
            }
            _ => break,
        }
    }
    assert!(seen_rejected, "DecompositionRejected event must fire");
    assert!(seen_aborted, "mission must emit an Aborted event");
    assert_eq!(runtime.state(), MissionState::Aborted);
}
