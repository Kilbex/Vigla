//! S9-T9: end-to-end completion-verdict integration test.
//!
//! A scripted-supervisor + mock-worker mission run that asserts the
//! new `MissionEventKind::CompletionVerdictRendered` event lands
//! between the per-task work and the legacy `Completed` event,
//! with a typed payload shape that matches a happy-path Accept
//! recommendation.
//!
//! The mock workers produce empty diffs; the project detector sees
//! no Cargo.toml or package.json, so audit's test_pass / lint
//! subscores abstain and only scope + security run. With empty
//! `scope_paths`, the scope subscore blends to 1.0; security has no
//! schema/secrets to flag, so `overall = 1.0`. The verdict's
//! recommendation derives to Accept under those conditions.

use orchestrator::arbiter::decision::ArbiterDecision;
use orchestrator::judgment::{CompletionVerdict, RiskBand};
use orchestrator::mission::{MissionSpec, MissionState};
use orchestrator::mission_event::MissionEventKind;
use orchestrator::mission_runtime::MissionRuntime;
use orchestrator::mission_supervisor_run::{ScriptedSupervisor, SupervisorDriver, WorkerBackend};
use orchestrator::mission_workspace::MissionWorkspace;
use std::process::Command as SyncCommand;
use std::time::{Duration, Instant};
use supervisor_adapter::{SupervisorIntent, SupervisorOutput, SupervisorTaskDescriptor};
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
    run(&["config", "user.email", "verdict-e2e@vigla.local"]);
    run(&["config", "user.name", "vigla-verdict-e2e"]);
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

fn happy_spec(title: &str) -> MissionSpec {
    MissionSpec {
        title: title.into(),
        objective: "exercise verdict assembly".into(),
        target_ref: "main".into(),
        tests: None,
        supervisor_model: Some("claude".into()),
        worker_model: None,
        worker_count: Some(2),
        confirm_plan: None,
        scope_paths: Vec::new(),
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn happy_path_mission_emits_accept_verdict_before_completed() {
    let repo = init_temp_repo();
    let mission_id = "mid-verdict-happy-0001";

    // Two-task happy-path decomposition. Mock workers always produce
    // empty acceptable diffs; supervisor scripts two turns (Decompose,
    // then DeclareComplete with a freeform prose summary).
    let tasks = vec![td("task-a", vec![]), td("task-b", vec![])];
    let turns: Vec<Vec<SupervisorOutput>> = vec![
        vec![SupervisorOutput::Intent(SupervisorIntent::Decompose {
            tasks,
            overview: None,
            tech_stack: None,
            envelope_fit: None,
        })],
        vec![SupervisorOutput::Intent(
            SupervisorIntent::DeclareComplete {
                summary: "shipped task-a + task-b".into(),
            },
        )],
    ];
    let driver = SupervisorDriver::Scripted(ScriptedSupervisor::new(turns));

    let workspace =
        MissionWorkspace::new(repo.path().to_path_buf(), mission_id.into()).expect("workspace");

    let runtime = MissionRuntime::start_supervised_with(
        happy_spec("verdict happy path"),
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
            panic!("mission did not terminate within deadline; events={events:?}");
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
            Ok(Err(_)) => break,
            Err(_) => break,
        }
    }

    let final_state = runtime.state();
    assert!(
        matches!(final_state, MissionState::CompletePendingMerge),
        "happy-path mission must reach CompletePendingMerge; got {final_state:?}; events={events:?}"
    );

    // ── Assertion 1: a CompletionVerdictRendered event exists ──────
    let verdict_idx = events
        .iter()
        .position(|e| matches!(e, MissionEventKind::CompletionVerdictRendered { .. }))
        .unwrap_or_else(|| {
            panic!(
                "expected one MissionEventKind::CompletionVerdictRendered event; \
                 events={events:?}"
            )
        });

    let completed_idx = events
        .iter()
        .position(|e| matches!(e, MissionEventKind::Completed { .. }))
        .expect("expected MissionEventKind::Completed");

    assert!(
        verdict_idx < completed_idx,
        "verdict must land before Completed; verdict_idx={verdict_idx} completed_idx={completed_idx}"
    );

    // ── Assertion 1b: Completed.files_changed reflects the count of
    // UNIQUE touched files (derived from WorkerResultSubmitted), not the
    // task count. The mock workers here produce empty diffs, so the
    // expected count is 0 even though the mission has ≥1 task.
    let expected_touched: std::collections::BTreeSet<&str> = events
        .iter()
        .filter_map(|e| match e {
            MissionEventKind::WorkerResultSubmitted { files, .. } => Some(files.iter()),
            _ => None,
        })
        .flatten()
        .map(String::as_str)
        .collect();
    let reported_files_changed = match &events[completed_idx] {
        MissionEventKind::Completed { files_changed, .. } => *files_changed,
        _ => unreachable!(),
    };
    assert_eq!(
        reported_files_changed as usize,
        expected_touched.len(),
        "Completed.files_changed must equal the count of unique touched files, \
         not the task count; touched={expected_touched:?}"
    );

    // ── Assertion 2: typed payload shape matches a happy-path Accept
    let payload_json = match &events[verdict_idx] {
        MissionEventKind::CompletionVerdictRendered { payload_json } => payload_json.clone(),
        _ => unreachable!(),
    };
    let v: CompletionVerdict = serde_json::from_str(&payload_json)
        .expect("verdict payload deserializes as CompletionVerdict");

    assert!(
        v.all_subtasks_accepted,
        "happy path should mark all_subtasks_accepted=true; got {v:?}"
    );
    assert!(
        matches!(v.residual_risk, RiskBand::Low | RiskBand::Medium),
        "happy path should be Low or Medium residual risk; got {:?}",
        v.residual_risk
    );
    assert!(
        matches!(v.recommendation, ArbiterDecision::Accept(_)),
        "happy path should recommend Accept; got {:?}",
        v.recommendation
    );

    // ── Assertion 3: the supervisor's prose threads through into the
    //                  Accept summary so the inbox card carries it.
    if let ArbiterDecision::Accept(payload) = &v.recommendation {
        assert!(
            payload.summary.contains("shipped task-a"),
            "Accept summary must contain the supervisor's prose; got {:?}",
            payload.summary
        );
    }
}
