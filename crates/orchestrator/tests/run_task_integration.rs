//! T2 — `run_task` integration tests.
//!
//! `run_task` (`orchestrator/src/mission_supervisor_run/run_task.rs`)
//! is the central per-task function. This file provides end-to-end
//! mock-driven mission runs that exercise the event-stream contract
//! of the per-task loop:
//!
//! 1. **happy_path** — single Happy mock worker → Accept on first
//!    pass → Integrated → CompletePendingMerge. Asserts event
//!    ordering `WorkerSpawned → WorkerResultSubmitted →
//!    ReviewStarted → ArbiterDecided(Accept) → Integrated`.
//! 2. **parallel_dispatch_preserves_per_worker_event_ordering** —
//!    two parallel tasks; asserts each worker's own event subsequence
//!    is internally ordered even when events from different workers
//!    interleave on the bus. Pins the `attempts_used_for_task` /
//!    `attempts_used_for_mission` coordination story under parallel
//!    fan-out.
//! 3. **abort_during_task_unwinds_cleanly** — runtime.abort() in the
//!    middle of an in-flight task must produce an Aborted event,
//!    leave the mission in `Aborted` state, and not deadlock on the
//!    JoinSet shutdown.
//! 4. **semantic_review_can_rework_a_submission_with_green_automated_gates**
//!    — a real supervisor review sees the committed patch, rejects an obvious
//!    draft despite a green audit, then accepts the corrected second pass.
//!
//! Budget exhaustion and per-rework-kind behavior are covered by the pure
//! arbiter and mission-loop suites, where policies can be varied directly.

use orchestrator::mission::{MissionSpec, MissionState};
use orchestrator::mission_event::{MissionEvent, MissionEventKind};
use orchestrator::mission_runtime::MissionRuntime;
use orchestrator::mission_supervisor_run::{ScriptedSupervisor, SupervisorDriver, WorkerBackend};
use orchestrator::mission_workspace::MissionWorkspace;
use std::process::Command as SyncCommand;
use std::time::{Duration, Instant};
use supervisor_adapter::{
    ReviewDecisionTag, ReviewIntent, SupervisorIntent, SupervisorOutput, SupervisorTaskDescriptor,
};
use tempfile::TempDir;

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
    run(&["config", "user.email", "run-task-e2e@vigla.local"]);
    run(&["config", "user.name", "vigla-run-task-e2e"]);
    run(&["config", "commit.gpgsign", "false"]);
    std::fs::write(path.join("README.md"), "hello\n").expect("write README");
    run(&["add", "README.md"]);
    run(&["commit", "-m", "initial"]);
    dir
}

fn td(title: &str, deps: Vec<u32>) -> SupervisorTaskDescriptor {
    SupervisorTaskDescriptor {
        title: title.into(),
        description: None,
        depends_on: deps,
        scope_paths: Vec::new(),
    }
}

fn review(
    worker_id: &str,
    decision: ReviewDecisionTag,
    directive: Option<&str>,
) -> SupervisorOutput {
    SupervisorOutput::Intent(SupervisorIntent::Review(ReviewIntent {
        worker_id: worker_id.into(),
        decision,
        summary: None,
        directive: directive.map(str::to_string),
        reason: None,
        from_worker: None,
        to_vendor: None,
        sub_tasks: None,
        reduced_scope: None,
        new_brief: None,
        rationale: None,
    }))
}

fn spec(title: &str, worker_count: u32) -> MissionSpec {
    MissionSpec {
        title: title.into(),
        objective: "exercise run_task per-task loop".into(),
        target_ref: "main".into(),
        tests: None,
        supervisor_model: Some("claude".into()),
        worker_model: None,
        worker_count: Some(worker_count),
        confirm_plan: None,
        scope_paths: Vec::new(),
    }
}

/// Drive the runtime to terminal (Completed or Aborted) and return
/// the full event log. Times out at 30s rather than hanging the test
/// runner.
async fn drain_to_terminal(runtime: &MissionRuntime) -> Vec<MissionEvent> {
    let mut rx = runtime.subscribe();
    let mut events: Vec<MissionEvent> = Vec::new();
    let deadline = Instant::now() + Duration::from_secs(30);
    loop {
        let remaining = deadline
            .checked_duration_since(Instant::now())
            .unwrap_or_default();
        if remaining.is_zero() {
            panic!("mission did not terminate within deadline; events={events:?}");
        }
        match tokio::time::timeout(remaining, rx.recv()).await {
            Ok(Ok(e)) => {
                let terminal = matches!(
                    e.kind,
                    MissionEventKind::Completed { .. } | MissionEventKind::Aborted { .. }
                );
                events.push(e);
                if terminal {
                    break;
                }
            }
            Ok(Err(_)) => break,
            Err(_) => break,
        }
    }
    events
}

fn position<F: Fn(&MissionEventKind) -> bool>(events: &[MissionEvent], pred: F) -> Option<usize> {
    events.iter().position(|e| pred(&e.kind))
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn happy_path_single_task_emits_expected_event_sequence() {
    let repo = init_temp_repo();
    let mission_id = "mid-run-task-happy-0001";

    let tasks = vec![td("task-0-happy", vec![])];
    let turns: Vec<Vec<SupervisorOutput>> = vec![
        vec![SupervisorOutput::Intent(SupervisorIntent::Decompose {
            tasks,
            overview: None,
            tech_stack: None,
            envelope_fit: None,
        })],
        vec![SupervisorOutput::Intent(
            SupervisorIntent::DeclareComplete {
                summary: "shipped task-0".into(),
            },
        )],
    ];
    let driver = SupervisorDriver::Scripted(ScriptedSupervisor::new(turns));

    let workspace =
        MissionWorkspace::new(repo.path().to_path_buf(), mission_id.into()).expect("workspace");

    let runtime = MissionRuntime::start_supervised_with(
        spec("run_task happy path", 1),
        workspace,
        driver,
        WorkerBackend::Mock,
    )
    .await
    .expect("start_supervised");

    let events = drain_to_terminal(&runtime).await;

    let final_state = runtime.state();
    assert!(
        matches!(final_state, MissionState::CompletePendingMerge),
        "happy single-task mission must reach CompletePendingMerge; got {final_state:?}; \
         events={:?}",
        events.iter().map(|e| &e.kind).collect::<Vec<_>>()
    );

    // Event ordering on the single worker.
    let spawn_idx = position(&events, |k| {
        matches!(k, MissionEventKind::WorkerSpawned { .. })
    })
    .expect("WorkerSpawned");
    match &events[spawn_idx].kind {
        MissionEventKind::WorkerSpawned { vendor, model, .. } => {
            assert_eq!(*vendor, Some(event_schema::Vendor::Mock));
            assert_eq!(model, &None);
        }
        other => panic!("expected WorkerSpawned, got {other:?}"),
    }
    let submit_idx = position(&events, |k| {
        matches!(k, MissionEventKind::WorkerResultSubmitted { .. })
    })
    .expect("WorkerResultSubmitted");
    let review_idx = position(&events, |k| {
        matches!(k, MissionEventKind::ReviewStarted { .. })
    })
    .expect("ReviewStarted");
    let decided_idx = position(&events, |k| {
        matches!(k, MissionEventKind::ArbiterDecided { .. })
    })
    .expect("ArbiterDecided");
    let integrated_idx = position(&events, |k| {
        matches!(k, MissionEventKind::Integrated { .. })
    })
    .expect("Integrated");

    assert!(
        spawn_idx < submit_idx
            && submit_idx < review_idx
            && review_idx < decided_idx
            && decided_idx < integrated_idx,
        "per-task event ordering broken: spawn={spawn_idx} submit={submit_idx} \
         review={review_idx} decided={decided_idx} integrated={integrated_idx}"
    );

    // The lone arbiter decision must be Accept (substring match keeps
    // the test resilient to ArbiterDecision shape tweaks).
    let decisions: Vec<&str> = events
        .iter()
        .filter_map(|e| match &e.kind {
            MissionEventKind::ArbiterDecided { decision_json, .. } => Some(decision_json.as_str()),
            _ => None,
        })
        .collect();
    assert_eq!(
        decisions.len(),
        1,
        "exactly one decision expected; got {decisions:?}"
    );
    assert!(
        decisions[0].contains("\"kind\":\"accept\""),
        "first decision must be Accept; got {}",
        decisions[0]
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn semantic_review_can_rework_a_submission_with_green_automated_gates() {
    let repo = init_temp_repo();
    let mission_id = "mid-run-task-semantic-review-0001";
    let turns = vec![
        vec![SupervisorOutput::Intent(SupervisorIntent::Decompose {
            tasks: vec![td("task-0-semantic-review", vec![])],
            overview: None,
            tech_stack: None,
            envelope_fit: None,
        })],
        vec![review(
            "mock-1",
            ReviewDecisionTag::Revise,
            Some("replace the draft with complete behavior"),
        )],
        vec![review("mock-1", ReviewDecisionTag::Accept, None)],
        vec![SupervisorOutput::Intent(
            SupervisorIntent::DeclareComplete {
                summary: "semantic rework integrated".into(),
            },
        )],
    ];
    let scripted = ScriptedSupervisor::new(turns).with_semantic_reviews();
    let prompt_observer = scripted.clone();
    let driver = SupervisorDriver::Scripted(scripted);
    let workspace =
        MissionWorkspace::new(repo.path().to_path_buf(), mission_id.into()).expect("workspace");
    let runtime = MissionRuntime::start_supervised_with(
        spec("semantic review", 1),
        workspace,
        driver,
        WorkerBackend::Mock,
    )
    .await
    .expect("start supervised mission");

    let events = drain_to_terminal(&runtime).await;

    let submissions = events
        .iter()
        .filter(|event| {
            matches!(
                &event.kind,
                MissionEventKind::WorkerResultSubmitted { worker_id, .. }
                    if worker_id == "mock-1"
            )
        })
        .count();
    assert_eq!(
        submissions, 2,
        "semantic revise must schedule a second pass"
    );

    let decisions = events
        .iter()
        .filter_map(|event| match &event.kind {
            MissionEventKind::ArbiterDecided { decision_json, .. } => {
                serde_json::from_str::<orchestrator::arbiter::ArbiterDecision>(decision_json).ok()
            }
            _ => None,
        })
        .collect::<Vec<_>>();
    assert!(matches!(
        decisions.first(),
        Some(orchestrator::arbiter::ArbiterDecision::Extend { .. })
    ));
    assert!(decisions
        .iter()
        .any(|decision| matches!(decision, orchestrator::arbiter::ArbiterDecision::Accept(_))));
    let review_prompts = prompt_observer
        .captured_prompts()
        .await
        .into_iter()
        .filter(|prompt| prompt.starts_with("Review worker mock-1."))
        .collect::<Vec<_>>();
    assert_eq!(review_prompts.len(), 2);
    assert!(
        review_prompts[0].contains("(mock content)"),
        "semantic review must receive the committed first-pass diff"
    );
    assert!(
        review_prompts[1].contains("revised at pass 1"),
        "semantic review must receive the committed rework diff"
    );
    assert_eq!(runtime.state(), MissionState::CompletePendingMerge);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn abort_during_task_unwinds_cleanly() {
    // Exercise the run_task path's response to MissionRuntime::abort.
    // The runtime spawns workers, then we abort almost immediately;
    // the per-task loop must observe the cancel token and unwind
    // without deadlocking the JoinSet. The final event must be
    // Aborted (not Completed), and the state must be Aborted.
    let repo = init_temp_repo();
    let mission_id = "mid-run-task-abort-0001";

    let tasks = vec![td("task-0-happy", vec![]), td("task-1", vec![])];
    let turns: Vec<Vec<SupervisorOutput>> = vec![
        vec![SupervisorOutput::Intent(SupervisorIntent::Decompose {
            tasks,
            overview: None,
            tech_stack: None,
            envelope_fit: None,
        })],
        vec![SupervisorOutput::Intent(
            SupervisorIntent::DeclareComplete {
                summary: "would-be summary".into(),
            },
        )],
    ];
    let driver = SupervisorDriver::Scripted(ScriptedSupervisor::new(turns));

    let workspace =
        MissionWorkspace::new(repo.path().to_path_buf(), mission_id.into()).expect("workspace");

    let runtime = MissionRuntime::start_supervised_with(
        spec("run_task abort", 2),
        workspace,
        driver,
        WorkerBackend::Mock,
    )
    .await
    .expect("start_supervised");

    let mut rx = runtime.subscribe();
    // Wait until at least one WorkerSpawned event lands so we know
    // the per-task loop is active when the abort signal hits.
    let deadline = Instant::now() + Duration::from_secs(15);
    let mut spawned = false;
    while Instant::now() < deadline {
        match tokio::time::timeout(Duration::from_secs(5), rx.recv()).await {
            Ok(Ok(e)) => {
                if matches!(e.kind, MissionEventKind::WorkerSpawned { .. }) {
                    spawned = true;
                    break;
                }
                // If the mission terminated before we could observe a
                // spawn (e.g. integration runs at sub-ms latency on
                // happy mocks), short-circuit — abort post-terminal
                // is meaningless. Fail loudly so a future scheduler
                // change doesn't silently invalidate this test.
                if matches!(
                    e.kind,
                    MissionEventKind::Completed { .. } | MissionEventKind::Aborted { .. }
                ) {
                    panic!(
                        "mission terminated before any WorkerSpawned event; \
                         abort test cannot exercise the per-task loop"
                    );
                }
            }
            _ => break,
        }
    }
    assert!(
        spawned,
        "expected at least one WorkerSpawned event before abort"
    );

    runtime.abort().await.expect("abort sends");

    let events = drain_to_terminal(&runtime).await;
    let final_state = runtime.state();
    assert_eq!(
        final_state,
        MissionState::Aborted,
        "abort must land the mission in Aborted; got {final_state:?}; events={:?}",
        events.iter().map(|e| &e.kind).collect::<Vec<_>>()
    );

    // The terminal event must be Aborted, not Completed.
    let last_terminal = events
        .iter()
        .rev()
        .find(|e| {
            matches!(
                e.kind,
                MissionEventKind::Completed { .. } | MissionEventKind::Aborted { .. }
            )
        })
        .expect("a terminal event must land");
    assert!(
        matches!(last_terminal.kind, MissionEventKind::Aborted { .. }),
        "terminal event must be Aborted; got {:?}",
        last_terminal.kind
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn parallel_dispatch_preserves_per_worker_event_ordering() {
    // Stress the parallel-dispatch coordination in `run_task`: two
    // tasks run concurrently under the default `JoinSet` scheduler.
    // The NeedsRevision worker reworks once; the Happy worker
    // accepts immediately. Each worker's own event stream must
    // remain internally ordered (Spawn → Submit → Review → Decide
    // → Integrate) even though events from the two workers may
    // interleave on the bus.
    let repo = init_temp_repo();
    let mission_id = "mid-run-task-parallel-0001";

    let tasks = vec![
        td("task-0-happy", vec![]),
        td("task-1-needs-revision", vec![]),
    ];
    let turns: Vec<Vec<SupervisorOutput>> = vec![
        vec![SupervisorOutput::Intent(SupervisorIntent::Decompose {
            tasks,
            overview: None,
            tech_stack: None,
            envelope_fit: None,
        })],
        vec![SupervisorOutput::Intent(
            SupervisorIntent::DeclareComplete {
                summary: "shipped both in parallel".into(),
            },
        )],
    ];
    let driver = SupervisorDriver::Scripted(ScriptedSupervisor::new(turns));

    let workspace =
        MissionWorkspace::new(repo.path().to_path_buf(), mission_id.into()).expect("workspace");

    let runtime = MissionRuntime::start_supervised_with(
        spec("run_task parallel", 2),
        workspace,
        driver,
        WorkerBackend::Mock,
    )
    .await
    .expect("start_supervised");

    let events = drain_to_terminal(&runtime).await;

    // For each worker_id, project the subsequence of events tagged
    // with that worker and assert internal ordering.
    let mut worker_ids: std::collections::BTreeSet<String> = std::collections::BTreeSet::new();
    for e in &events {
        let wid = match &e.kind {
            MissionEventKind::WorkerSpawned { worker_id, .. }
            | MissionEventKind::WorkerResultSubmitted { worker_id, .. }
            | MissionEventKind::ReviewStarted { worker_id, .. }
            | MissionEventKind::ArbiterDecided { worker_id, .. }
            | MissionEventKind::Integrated { worker_id, .. } => Some(worker_id.clone()),
            _ => None,
        };
        if let Some(w) = wid {
            worker_ids.insert(w);
        }
    }
    assert!(
        worker_ids.len() >= 2,
        "expected at least 2 distinct worker_ids; got {worker_ids:?}"
    );

    for wid in &worker_ids {
        // Project this worker's positions in the global stream.
        let mut spawn = None;
        let mut first_submit = None;
        let mut first_review = None;
        let mut first_decided = None;
        let mut integrated = None;
        for (i, e) in events.iter().enumerate() {
            let matches_wid = match &e.kind {
                MissionEventKind::WorkerSpawned { worker_id, .. } => worker_id == wid,
                MissionEventKind::WorkerResultSubmitted { worker_id, .. } => worker_id == wid,
                MissionEventKind::ReviewStarted { worker_id } => worker_id == wid,
                MissionEventKind::ArbiterDecided { worker_id, .. } => worker_id == wid,
                MissionEventKind::Integrated { worker_id, .. } => worker_id == wid,
                _ => false,
            };
            if !matches_wid {
                continue;
            }
            match &e.kind {
                MissionEventKind::WorkerSpawned { .. } if spawn.is_none() => spawn = Some(i),
                MissionEventKind::WorkerResultSubmitted { .. } if first_submit.is_none() => {
                    first_submit = Some(i)
                }
                MissionEventKind::ReviewStarted { .. } if first_review.is_none() => {
                    first_review = Some(i)
                }
                MissionEventKind::ArbiterDecided { .. } if first_decided.is_none() => {
                    first_decided = Some(i)
                }
                MissionEventKind::Integrated { .. } if integrated.is_none() => integrated = Some(i),
                _ => {}
            }
        }
        let spawn = spawn.unwrap_or_else(|| panic!("{wid} missing WorkerSpawned"));
        let first_submit =
            first_submit.unwrap_or_else(|| panic!("{wid} missing WorkerResultSubmitted"));
        let first_review = first_review.unwrap_or_else(|| panic!("{wid} missing ReviewStarted"));
        let first_decided = first_decided.unwrap_or_else(|| panic!("{wid} missing ArbiterDecided"));
        let integrated = integrated.unwrap_or_else(|| panic!("{wid} missing Integrated"));
        assert!(
            spawn < first_submit
                && first_submit < first_review
                && first_review < first_decided
                && first_decided <= integrated,
            "per-worker ordering broken for {wid}: \
             spawn={spawn} submit={first_submit} review={first_review} \
             decided={first_decided} integrated={integrated}"
        );
    }

    // Sanity: mission still terminated cleanly.
    assert!(matches!(
        runtime.state(),
        MissionState::CompletePendingMerge
    ));
}
