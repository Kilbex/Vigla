//! Launch safety regression: `Split` is fail-closed until live DAG grafting
//! can preserve dependency semantics. Neither generated replacements nor an
//! original dependent may run after a prerequisite requests Split.
//!
//! ## Setup
//!
//! - Tempdir initialised as a minimal Cargo crate (`Cargo.toml` +
//!   `src/lib.rs`) with one deliberately panicking test. The audit's
//!   project detector picks up `Cargo.toml` → runs `cargo test`
//!   → the test fails → `test_pass.score = 0` → blended overall
//!   drops below `ArbiterPolicy.quality_min` (0.7) → the per-pass
//!   supervisor review turn fires.
//! - Two-task decomposition A → B; A is `mock-1`, B is `mock-2`.
//! - Scripted supervisor turns:
//!   1. `Decompose` (1 task — "parent task")
//!   2. `Review { decision: Split, sub_tasks: [A, B] }` for `mock-1`
//!   3. `DeclareComplete` (must not be reached on the escalation path)
//!
//! `MarkUnachievable` produces `NextLoopAction::Escalate`, so the
//! first sub-task to land its review turn drives the mission to
//! `Attention`. The second sub-task spawns in the same fill pass
//! (max_parallel = 4 by default, both sub-tasks indegree-0) before
//! either's review turn fires — so we still observe both
//! `WorkerSpawned` events even though only one needs to complete its
//! review for the mission to escalate.
//!
//! ## What this pins (D5 acceptance)
//!
//! 1. Parent `mock-1` emits `ArbiterDecided` with the outer
//!    `kind:"extend"` AND an inner rework_kind `kind:"split"`.
//! 2. No worker other than A is spawned, including dependent B.
//! 3. A fail-closed explanation is emitted.
//! 4. No `Integrated` event for the parent (`mock-1`) — Split sends
//!    it down `NextLoopAction::Skip`, bypassing integration.
//! 5. Final state is `Attention`.

use orchestrator::mission::{MissionSpec, MissionState, ResolveAction};
use orchestrator::mission_event::MissionEventKind;
use orchestrator::mission_runtime::MissionRuntime;
use orchestrator::mission_supervisor_run::{ScriptedSupervisor, SupervisorDriver, WorkerBackend};
use orchestrator::mission_workspace::MissionWorkspace;
use std::process::Command as SyncCommand;
use std::time::{Duration, Instant};
use supervisor_adapter::{
    ReviewDecisionTag, ReviewIntent, SupervisorIntent, SupervisorOutput, SupervisorTaskDescriptor,
};
use tempfile::TempDir;

/// Tempdir with `git init --initial-branch=main`, a Cargo crate
/// whose `src/lib.rs` contains an intentionally panicking unit test,
/// and one initial commit. Audit will detect the Cargo project and
/// run `cargo test`, which fails, dropping the blended audit score
/// well below the quality floor (0.7) and forcing the per-pass
/// supervisor review turn to fire.
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

    // Minimal Rust crate. `cargo test` will compile + run; the
    // single test panics, producing
    // `test result: FAILED. 0 passed; 1 failed; ...` → score=0.
    let cargo_toml = "\
[package]
name = \"split_test_fixture\"
version = \"0.0.1\"
edition = \"2021\"

[lib]
path = \"src/lib.rs\"
";
    let lib_rs = "\
pub fn id(x: i32) -> i32 { x }

#[cfg(test)]
mod tests {
    #[test]
    fn always_fails() {
        panic!(\"intentional failure to force audit below quality floor\");
    }
}
";
    std::fs::create_dir_all(path.join("src")).expect("mkdir src");
    std::fs::write(path.join("Cargo.toml"), cargo_toml).expect("write Cargo.toml");
    std::fs::write(path.join("src/lib.rs"), lib_rs).expect("write lib.rs");
    std::fs::write(path.join("README.md"), "split test fixture\n").expect("write README");

    run(&["add", "Cargo.toml", "src/lib.rs", "README.md"]);
    run(&["commit", "-m", "initial: failing Cargo fixture"]);
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
    // `worker_model: None` forces the Mock backend regardless of
    // host-side CLI availability. `scope_paths: vec![]` keeps the
    // ACL unconstrained (the mock worker writes `MOCK_*.md` at the
    // repo root, which would otherwise need a scope entry).
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
async fn split_escalates_without_starting_replacements_or_dependants() {
    let repo = init_temp_repo();
    let mission_id = "mid-split-drain-0001";

    // ── Scripted supervisor turns ─────────────────────────────────
    //
    // Each `vec![SupervisorOutput::...]` is one turn's outputs. The
    // ScriptedSupervisor consumes them in order. If we run out, the
    // driver returns `NoIntent` (no-op for review; the arbiter
    // defaults to a Revise/Scrub path).
    let turns: Vec<Vec<SupervisorOutput>> = vec![
        // Turn 1: decompose into prerequisite A and dependent B.
        vec![SupervisorOutput::Intent(SupervisorIntent::Decompose {
            tasks: vec![
                td(0, "parent task", vec![]),
                td(1, "dependent task", vec![0]),
            ],
            overview: None,
            tech_stack: None,
            envelope_fit: None,
        })],
        // Turn 2: parent fails audit → review turn fires; supervisor
        // returns Split with 2 sub-tasks targeting worker `mock-1`.
        vec![SupervisorOutput::Intent(SupervisorIntent::Review(
            ReviewIntent {
                worker_id: "mock-1".into(),
                decision: ReviewDecisionTag::Split,
                sub_tasks: Some(vec![
                    SupervisorTaskDescriptor {
                        title: "sub-task A".into(),
                        description: None,
                        depends_on: vec![],
                        scope_paths: vec![],
                    },
                    SupervisorTaskDescriptor {
                        title: "sub-task B".into(),
                        description: None,
                        depends_on: vec![],
                        scope_paths: vec![],
                    },
                ]),
                summary: None,
                directive: None,
                reason: None,
                from_worker: None,
                to_vendor: None,
                reduced_scope: None,
                new_brief: None,
                rationale: None,
            },
        ))],
        // Final turn: DeclareComplete (only consumed on the
        // CompletePendingMerge path; the Escalate path returns
        // before this turn would fire).
        vec![SupervisorOutput::Intent(
            SupervisorIntent::DeclareComplete {
                summary: "split mission complete".into(),
            },
        )],
    ];
    let driver = SupervisorDriver::Scripted(ScriptedSupervisor::new(turns));

    let workspace =
        MissionWorkspace::new(repo.path().to_path_buf(), mission_id.into()).expect("workspace");

    let runtime = MissionRuntime::start_supervised_with(
        ok_spec(
            "split drain",
            "force a Split rework and exercise the dispatcher drain loop",
        ),
        workspace,
        driver,
        WorkerBackend::Mock,
    )
    .await
    .expect("start_supervised");

    // ── Drain events into a buffer ────────────────────────────────
    // Subscribe BEFORE we await terminal state so we don't miss
    // anything. Drain in a background task; the main test thread
    // awaits terminal state separately. A safety deadline guards
    // against the dispatcher hanging on a regression.
    let mut rx = runtime.subscribe();
    let deadline = Instant::now() + Duration::from_secs(45);
    let mut events: Vec<MissionEventKind> = Vec::new();

    // Wait for terminal state with a deadline. The escalate path
    // never emits `Completed` or `Aborted` — only the state
    // transitions to `Attention`. `await_complete_or_terminal`
    // unblocks on Attention | CompletePendingMerge | Merged |
    // Discarded | Aborted, which is exactly what we want.
    let terminal_state = tokio::time::timeout(
        Duration::from_secs(45),
        runtime.await_complete_or_terminal(),
    )
    .await
    .expect("mission did not terminate within deadline");

    // Drain any events still pending in the broadcast channel. We
    // give a short post-terminal window so trailing emits (the
    // escalation rationale WorkerProgress, etc.) land in the buffer.
    let drain_deadline = Instant::now() + Duration::from_millis(500);
    loop {
        let remaining = drain_deadline
            .checked_duration_since(Instant::now())
            .unwrap_or_default();
        if Instant::now() >= deadline {
            break;
        }
        match tokio::time::timeout(remaining, rx.recv()).await {
            Ok(Ok(e)) => events.push(e.kind),
            Ok(Err(_)) => break, // sender dropped — mission task ended
            Err(_) => break,     // post-terminal drain window elapsed
        }
    }

    // ── Diagnostics ──────────────────────────────────────────────
    // Emit a compact summary so failures are debuggable without
    // re-running. Only printed under `-- --nocapture`.
    eprintln!("─── terminal_state: {terminal_state:?} ───");
    for (i, ev) in events.iter().enumerate() {
        match ev {
            MissionEventKind::WorkerSpawned {
                worker_id,
                task_index,
                ..
            } => eprintln!("  [{i}] WorkerSpawned wid={worker_id} task_index={task_index}"),
            MissionEventKind::Integrated { worker_id, .. } => {
                eprintln!("  [{i}] Integrated wid={worker_id}")
            }
            MissionEventKind::ArbiterDecided {
                worker_id,
                decision_json,
                ..
            } => eprintln!("  [{i}] ArbiterDecided wid={worker_id} decision={decision_json}"),
            MissionEventKind::WorkerProgress { worker_id, note } => {
                eprintln!("  [{i}] WorkerProgress wid={worker_id} note={note}")
            }
            MissionEventKind::AuditCompleted { overall, .. } => {
                eprintln!("  [{i}] AuditCompleted overall={overall:.3}")
            }
            other => eprintln!("  [{i}] {other:?}"),
        }
    }

    // ── Assertion 1: parent ArbiterDecided extend+split ──────────
    let parent_extend_split = events.iter().any(|e| match e {
        MissionEventKind::ArbiterDecided {
            worker_id,
            decision_json,
            ..
        } => {
            worker_id == "mock-1"
                && decision_json.contains("\"kind\":\"extend\"")
                && decision_json.contains("\"kind\":\"split\"")
        }
        _ => false,
    });
    assert!(
        parent_extend_split,
        "expected an ArbiterDecided for mock-1 with kind=extend + rework_kind=split; \
         events={events:?}"
    );

    // ── Assertion 2: neither dependent nor replacement spawned ──
    let later_spawns: Vec<&String> = events
        .iter()
        .filter_map(|e| match e {
            MissionEventKind::WorkerSpawned { worker_id, .. } => {
                let n: u32 = worker_id
                    .trim_start_matches("mock-")
                    .split('-')
                    .next()
                    .unwrap_or("0")
                    .parse()
                    .unwrap_or(0);
                if n >= 2 {
                    Some(worker_id)
                } else {
                    None
                }
            }
            _ => None,
        })
        .collect();
    assert!(later_spawns.is_empty(), "Split must not start dependent or replacement workers; got {later_spawns:?}; events={events:?}");

    // ── Assertion 3: fail-closed explanation ─────────────────────
    let fail_closed = events.iter().any(|e| match e {
        MissionEventKind::WorkerProgress { note, .. } => {
            note.contains("live DAG grafting") && note.contains("no replacement or dependent tasks")
        }
        _ => false,
    });
    assert!(
        fail_closed,
        "expected explicit fail-closed Split explanation; events={events:?}"
    );

    // ── Assertion 4: no Integrated for the parent ────────────────
    // Split → NextLoopAction::Skip → run_task returns Done WITHOUT
    // entering the integration phase. If we see Integrated for
    // mock-1, the Skip path regressed.
    let parent_integrated = events.iter().any(|e| {
        matches!(
            e,
            MissionEventKind::Integrated { worker_id, .. } if worker_id == "mock-1"
        )
    });
    assert!(
        !parent_integrated,
        "parent worker mock-1 was skipped by Split; must not produce an \
         Integrated event; events={events:?}"
    );

    // ── Assertion 5: terminal state is Attention ─────────────────
    assert_eq!(terminal_state, MissionState::Attention);

    // ── Assertion 6: fully skipped work cannot mint final anchors ─
    let merge_error = runtime
        .resolve(ResolveAction::Merge)
        .await
        .expect_err("a mission with no integrated commits must not be mergeable");
    assert!(merge_error.to_string().contains("no mission commits"));
    assert_eq!(runtime.state(), MissionState::Attention);
    for tag in [
        format!("vigla/revert/{mission_id}/before/main"),
        format!("vigla/revert/{mission_id}/merged/main"),
    ] {
        let status = SyncCommand::new("git")
            .args(["rev-parse", "--verify", &format!("refs/tags/{tag}")])
            .current_dir(repo.path())
            .status()
            .unwrap();
        assert!(!status.success(), "refused empty merge created {tag}");
    }

    runtime.resolve(ResolveAction::Discard).await.unwrap();
    assert_eq!(runtime.state(), MissionState::Discarded);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn scrubbed_prerequisite_never_releases_its_dependant() {
    let repo = init_temp_repo();
    let mission_id = "mid-scrub-dependency-0001";
    let revise = || {
        vec![SupervisorOutput::Intent(SupervisorIntent::Review(
            ReviewIntent {
                worker_id: "mock-1".into(),
                decision: ReviewDecisionTag::Revise,
                summary: None,
                directive: Some("fix the failing test".into()),
                reason: None,
                from_worker: None,
                to_vendor: None,
                sub_tasks: None,
                reduced_scope: None,
                new_brief: None,
                rationale: None,
            },
        ))]
    };
    let driver = SupervisorDriver::Scripted(ScriptedSupervisor::new(vec![
        vec![SupervisorOutput::Intent(SupervisorIntent::Decompose {
            tasks: vec![
                td(0, "failing prerequisite", vec![]),
                td(1, "dependant", vec![0]),
            ],
            overview: None,
            tech_stack: None,
            envelope_fit: None,
        })],
        revise(),
        revise(),
        revise(),
    ]));
    let runtime = MissionRuntime::start_supervised_with(
        ok_spec("scrub dependency", "exhaust rework on a prerequisite"),
        MissionWorkspace::new(repo.path().to_path_buf(), mission_id.into()).unwrap(),
        driver,
        WorkerBackend::Mock,
    )
    .await
    .unwrap();
    let mut events = runtime.subscribe();

    assert_eq!(
        tokio::time::timeout(
            Duration::from_secs(45),
            runtime.await_complete_or_terminal()
        )
        .await
        .expect("mission must quiesce"),
        MissionState::Attention
    );
    let mut captured = Vec::new();
    while let Ok(Ok(event)) = tokio::time::timeout(Duration::from_millis(100), events.recv()).await
    {
        captured.push(event.kind);
    }
    assert!(captured.iter().any(|kind| matches!(
        kind,
        MissionEventKind::ArbiterDecided { decision_json, .. }
            if decision_json.contains("\"kind\":\"scrub\"")
    )));
    assert!(
        !captured
            .iter()
            .any(|kind| matches!(kind, MissionEventKind::WorkerSpawned { task_index: 1, .. })),
        "a scrubbed task must not satisfy its dependant: {captured:?}"
    );
    runtime.resolve(ResolveAction::Discard).await.unwrap();
}
