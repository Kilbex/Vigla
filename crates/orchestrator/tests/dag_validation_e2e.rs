//! End-to-end DAG validation through the supervisor decomposition flow.
//!
//! For each of the four invalid-DAG shapes — Cycle, OrphanDependency,
//! DuplicateIndex, EmptyDecomposition — drive a real `MissionRuntime`
//! with a `ScriptedSupervisor` that emits the invalid task vector on
//! its first turn and assert that:
//!
//!   1. A `MissionEventKind::DecompositionRejected` event fires whose
//!      JSON `reason` carries the matching `GraphError` discriminant.
//!   2. The mission reaches `MissionState::Aborted`.
//!   3. No `MissionEventKind::WorkerSpawned` event fires — rejection
//!      halts dispatch before any worker is started.
//!
//! These four cases mirror the unit-level coverage in
//! `orchestrator/src/task_graph/validate.rs`; the integration here
//! pins the wire-up between `task_graph::validate` and the supervisor
//! mission loop (see `mission_supervisor_run::mission_loop`, the
//! "S7: validate decomposition as a DAG" block).

use orchestrator::mission::MissionId;
use orchestrator::mission_event::MissionEventKind;
use orchestrator::mission_runtime::MissionRuntime;
use orchestrator::mission_supervisor_run::{ScriptedSupervisor, SupervisorDriver, WorkerBackend};
use orchestrator::mission_workspace::MissionWorkspace;
use orchestrator::task_graph::GraphError;
use orchestrator::{MissionEvent, MissionSpec, MissionState};
use std::path::PathBuf;
use std::process::Command;
use std::time::Duration;
use supervisor_adapter::{SupervisorIntent, SupervisorOutput, SupervisorTaskDescriptor};
use tempfile::TempDir;

/// Spin up a tempdir with `git init` + an initial commit on `main`.
/// `MissionWorkspace` requires a clean repo to branch off of.
fn make_sandbox_repo() -> (TempDir, PathBuf) {
    let temp = TempDir::new().expect("tempdir");
    let path = temp.path().to_path_buf();
    let run = |args: &[&str]| {
        let out = Command::new("git")
            .args(args)
            .current_dir(&path)
            .output()
            .expect("git");
        assert!(
            out.status.success(),
            "git {args:?} stderr={}",
            String::from_utf8_lossy(&out.stderr)
        );
    };
    run(&["init", "--initial-branch=main"]);
    run(&["config", "user.email", "dag-e2e@vigla.local"]);
    run(&["config", "user.name", "vigla-dag-e2e"]);
    run(&["config", "commit.gpgsign", "false"]);
    std::fs::write(path.join("README.md"), "dag-e2e sandbox\n").unwrap();
    run(&["add", "README.md"]);
    run(&["commit", "-m", "initial"]);
    (temp, path)
}

fn spec_for_case(label: &str) -> MissionSpec {
    MissionSpec {
        title: format!("dag-e2e {label}"),
        objective: format!("trigger DAG rejection via the {label} shape"),
        target_ref: "main".into(),
        tests: None,
        supervisor_model: None,
        worker_model: None,
        worker_count: None,
        confirm_plan: None,
        scope_paths: vec![],
    }
}

/// Build a single-turn scripted supervisor that emits one
/// `Decompose` intent carrying the supplied task vector.
fn scripted_decomp(tasks: Vec<SupervisorTaskDescriptor>) -> SupervisorDriver {
    SupervisorDriver::Scripted(ScriptedSupervisor::new(vec![vec![
        SupervisorOutput::Intent(SupervisorIntent::Decompose {
            tasks,
            overview: None,
            tech_stack: None,
            envelope_fit: None,
        }),
    ]]))
}

/// Drive a scripted-supervisor mission to terminal state, returning
/// every emitted mission event. Times out after 8 seconds so a
/// regression that fails to terminate doesn't hang the harness.
async fn run_and_drain(
    mission_id: &str,
    spec: MissionSpec,
    driver: SupervisorDriver,
) -> (Vec<MissionEvent>, MissionState) {
    let (_temp, root) = make_sandbox_repo();
    let workspace =
        MissionWorkspace::new(root, MissionId::from(mission_id.to_string())).expect("workspace");
    let runtime =
        MissionRuntime::start_supervised_with(spec, workspace, driver, WorkerBackend::Mock)
            .await
            .expect("start_supervised_with");

    let mut rx = runtime.subscribe();
    let mut events: Vec<MissionEvent> = Vec::new();
    let deadline = tokio::time::Instant::now() + Duration::from_secs(8);

    loop {
        if tokio::time::Instant::now() >= deadline {
            panic!("dag-validation mission did not terminate within 8s; events={events:?}");
        }
        let remaining = deadline - tokio::time::Instant::now();
        match tokio::time::timeout(remaining, rx.recv()).await {
            Ok(Ok(ev)) => {
                let aborted = matches!(ev.kind, MissionEventKind::Aborted { .. });
                events.push(ev);
                if aborted {
                    // Drain any trailing events the supervisor task
                    // emitted alongside the Aborted card (e.g. final
                    // state updates) before checking assertions.
                    let _ = tokio::time::timeout(Duration::from_millis(50), async {
                        while let Ok(Ok(extra)) =
                            tokio::time::timeout(Duration::from_millis(50), rx.recv()).await
                        {
                            events.push(extra);
                        }
                    })
                    .await;
                    break;
                }
            }
            Ok(Err(_)) => break,
            Err(_) => {
                panic!("dag-validation mission stalled; events={events:?}");
            }
        }
    }

    let state = runtime.state();
    (events, state)
}

/// Extract the first `DecompositionRejected` event's reason JSON.
fn first_rejected_reason(events: &[MissionEvent]) -> Option<&str> {
    events.iter().find_map(|e| match &e.kind {
        MissionEventKind::DecompositionRejected { reason } => Some(reason.as_str()),
        _ => None,
    })
}

/// True if any `WorkerSpawned` event landed in the stream.
fn any_worker_spawned(events: &[MissionEvent]) -> bool {
    events
        .iter()
        .any(|e| matches!(e.kind, MissionEventKind::WorkerSpawned { .. }))
}

/// Assert the canonical shape every invalid-DAG case must satisfy:
///   - a `DecompositionRejected` event whose `reason` JSON deserializes
///     to the expected `GraphError` variant,
///   - no `WorkerSpawned` event,
///   - terminal `MissionState::Aborted`.
fn assert_rejected(
    events: &[MissionEvent],
    state: &MissionState,
    matcher: impl Fn(&GraphError) -> bool,
    label: &str,
) {
    let reason = first_rejected_reason(events).unwrap_or_else(|| {
        panic!(
            "[{label}] expected DecompositionRejected; got events={:?}",
            events.iter().map(|e| &e.kind).collect::<Vec<_>>()
        )
    });
    let err: GraphError = serde_json::from_str(reason).unwrap_or_else(|e| {
        panic!("[{label}] reason did not deserialize as GraphError: {e}\nreason={reason}")
    });
    assert!(
        matcher(&err),
        "[{label}] wrong GraphError variant: {err:?} (reason={reason})"
    );
    assert!(
        !any_worker_spawned(events),
        "[{label}] WorkerSpawned must not fire after DecompositionRejected; events={:?}",
        events.iter().map(|e| &e.kind).collect::<Vec<_>>()
    );
    assert_eq!(
        state,
        &MissionState::Aborted,
        "[{label}] mission must terminate in Aborted; got {state:?}"
    );
}

// ──────────────────────────────────────────────────────────────────
// Case 1: Cycle — task 0 depends_on task 1, task 1 depends_on task 0.
// ──────────────────────────────────────────────────────────────────
#[tokio::test(flavor = "current_thread")]
async fn cycle_decomposition_emits_rejected_with_cycle_variant() {
    let tasks = vec![
        SupervisorTaskDescriptor {
            title: "a".into(),
            description: None,
            depends_on: vec![1],
            scope_paths: vec![],
        },
        SupervisorTaskDescriptor {
            title: "b".into(),
            description: None,
            depends_on: vec![0],
            scope_paths: vec![],
        },
    ];
    let (events, state) = run_and_drain(
        "dag-e2e-cycle-0001",
        spec_for_case("cycle"),
        scripted_decomp(tasks),
    )
    .await;
    assert_rejected(
        &events,
        &state,
        |e| matches!(e, GraphError::Cycle { .. }),
        "cycle",
    );
}

// ──────────────────────────────────────────────────────────────────
// Case 2: Orphan — task 1 depends_on index 99 not in the task list.
// ──────────────────────────────────────────────────────────────────
#[tokio::test(flavor = "current_thread")]
async fn orphan_dependency_decomposition_emits_rejected_with_orphan_variant() {
    let tasks = vec![
        SupervisorTaskDescriptor {
            title: "a".into(),
            description: None,
            depends_on: vec![],
            scope_paths: vec![],
        },
        SupervisorTaskDescriptor {
            title: "b".into(),
            description: None,
            depends_on: vec![99],
            scope_paths: vec![],
        },
    ];
    let (events, state) = run_and_drain(
        "dag-e2e-orphan-0001",
        spec_for_case("orphan"),
        scripted_decomp(tasks),
    )
    .await;
    assert_rejected(
        &events,
        &state,
        |e| matches!(e, GraphError::OrphanDependency { from: 1, to: 99 }),
        "orphan",
    );
}

// ──────────────────────────────────────────────────────────────────
// Case 3: Duplicate — two tasks share the same final index (0).
//
// The mission loop renumbers tasks by their position in the
// `SupervisorTaskDescriptor` vector (index = i), so producing a
// duplicate via this surface requires looking past the supervisor
// adapter — we leave the scripted-supervisor path and instead
// confirm the validate-then-emit seam by injecting through the
// `task_graph::validate` API. The unit-level coverage in
// `validate.rs` exercises the same variant; this integration test
// pins it through the same `GraphError` deserialization path the
// mission loop uses to construct the `DecompositionRejected` reason.
// ──────────────────────────────────────────────────────────────────
#[tokio::test(flavor = "current_thread")]
async fn duplicate_index_decomposition_emits_rejected_with_duplicate_variant() {
    // The supervisor adapter's task vector is re-indexed positionally
    // inside the mission loop, so the only way to surface a duplicate
    // index at validate() time today is to skip the adapter and drive
    // the validator directly. The resulting `GraphError` is the same
    // wire payload the mission loop serializes into the
    // `DecompositionRejected.reason` field.
    use orchestrator::mission_event::TaskDescriptor;
    let tasks = vec![
        TaskDescriptor {
            index: 0,
            title: "first".into(),
            depends_on: vec![],
            ..Default::default()
        },
        TaskDescriptor {
            index: 0,
            title: "duplicate".into(),
            depends_on: vec![],
            ..Default::default()
        },
    ];
    let err = orchestrator::task_graph::validate(&tasks).expect_err("duplicate index must reject");
    match err {
        GraphError::DuplicateIndex { index } => assert_eq!(index, 0),
        other => panic!("expected DuplicateIndex, got {other:?}"),
    }

    // Round-trip the same GraphError through the same serialization
    // the mission loop uses, so a future schema rename is caught here
    // alongside the E2E path.
    let reason = serde_json::to_string(&GraphError::DuplicateIndex { index: 0 }).unwrap();
    let back: GraphError = serde_json::from_str(&reason).unwrap();
    assert!(matches!(back, GraphError::DuplicateIndex { index: 0 }));
}

// ──────────────────────────────────────────────────────────────────
// Case 4: Empty — zero-length tasks vector. The supervisor emits a
// `Decompose { tasks: vec![] }`; the loop routes it through
// `validate(&[])` → `EmptyDecomposition` → `DecompositionRejected`.
// ──────────────────────────────────────────────────────────────────
#[tokio::test(flavor = "current_thread")]
async fn empty_decomposition_emits_rejected_with_empty_variant() {
    let (events, state) = run_and_drain(
        "dag-e2e-empty-0001",
        spec_for_case("empty"),
        scripted_decomp(vec![]),
    )
    .await;
    assert_rejected(
        &events,
        &state,
        |e| matches!(e, GraphError::EmptyDecomposition),
        "empty",
    );
}
