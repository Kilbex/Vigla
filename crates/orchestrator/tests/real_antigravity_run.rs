//! Real Antigravity CLI integration gate.
//!
//! The supervisor is scripted so this test spends one Antigravity worker
//! invocation and exercises the production mission path: profile rendering,
//! `agy` process launch, adapter normalization, worker commit, audit,
//! integration, and final merge.

use orchestrator::mission::{MissionSpec, MissionState, ResolveAction};
use orchestrator::mission_event::{MissionEvent, MissionEventKind};
use orchestrator::mission_supervisor_run::{ScriptedSupervisor, SupervisorDriver, WorkerBackend};
use orchestrator::{MissionRuntime, MissionWorkspace, WorkerVendor};
use std::path::Path;
use std::process::Command;
use std::time::{Duration, Instant};
use supervisor_adapter::{SupervisorIntent, SupervisorOutput, SupervisorTaskDescriptor};
use tempfile::TempDir;

fn run_git(root: &Path, args: &[&str]) {
    let output = Command::new("git")
        .args(args)
        .current_dir(root)
        .output()
        .expect("git command");
    assert!(
        output.status.success(),
        "git {args:?} failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
}

fn make_failing_sandbox() -> TempDir {
    let dir = tempfile::tempdir().expect("tempdir");
    let root = dir.path();
    run_git(root, &["init", "--initial-branch=main"]);
    run_git(
        root,
        &["config", "user.email", "antigravity-gate@vigla.local"],
    );
    run_git(root, &["config", "user.name", "vigla-antigravity-gate"]);
    run_git(root, &["config", "commit.gpgsign", "false"]);

    std::fs::create_dir_all(root.join("src")).expect("create src");
    std::fs::create_dir_all(root.join("tests")).expect("create tests");
    std::fs::write(root.join(".gitignore"), "target/\n").expect("write gitignore");
    std::fs::write(
        root.join("Cargo.toml"),
        r#"[package]
name = "antigravity_gate_fixture"
version = "0.1.0"
edition = "2021"
"#,
    )
    .expect("write Cargo.toml");
    std::fs::write(
        root.join("src/lib.rs"),
        "pub fn multiply(a: i64, b: i64) -> i64 {\n    a + b\n}\n",
    )
    .expect("write lib.rs");
    std::fs::write(
        root.join("tests/multiply.rs"),
        "use antigravity_gate_fixture::multiply;\n\n#[test]\nfn multiplies() {\n    assert_eq!(multiply(6, 7), 42);\n}\n",
    )
    .expect("write test");
    let lockfile = Command::new(env!("CARGO"))
        .arg("generate-lockfile")
        .current_dir(root)
        .output()
        .expect("generate fixture lockfile");
    assert!(
        lockfile.status.success(),
        "generate fixture lockfile: {}",
        String::from_utf8_lossy(&lockfile.stderr)
    );
    run_git(root, &["add", "."]);
    run_git(root, &["commit", "-m", "add failing multiply fixture"]);
    dir
}

fn cargo_test(root: &Path) -> std::process::Output {
    Command::new(env!("CARGO"))
        .args(["test", "--quiet"])
        .current_dir(root)
        .output()
        .expect("cargo test fixture")
}

async fn drain_to_terminal(runtime: &MissionRuntime) -> Vec<MissionEvent> {
    let mut rx = runtime.subscribe();
    let mut events = Vec::new();
    let deadline = Instant::now() + Duration::from_secs(600);
    loop {
        let remaining = deadline
            .checked_duration_since(Instant::now())
            .unwrap_or_default();
        assert!(
            !remaining.is_zero(),
            "Antigravity mission timed out: {events:?}"
        );
        let event = match tokio::time::timeout(Duration::from_secs(1), rx.recv()).await {
            Ok(Ok(event)) => event,
            Ok(Err(_)) => panic!(
                "mission event stream closed: state={:?}, events={events:?}",
                runtime.state()
            ),
            Err(_) if runtime.state() == MissionState::Attention => return events,
            Err(_) => continue,
        };
        let terminal = matches!(
            event.kind,
            MissionEventKind::Completed { .. } | MissionEventKind::Aborted { .. }
        );
        events.push(event);
        if terminal {
            return events;
        }
    }
}

#[tokio::test(flavor = "current_thread")]
#[ignore = "requires authenticated `agy` CLI; opt in via `cargo test -- --ignored`"]
async fn real_antigravity_fixes_failing_multiply_test() {
    let version = Command::new("agy")
        .arg("--version")
        .output()
        .expect("`agy` is required for the Antigravity gate");
    assert!(
        version.status.success(),
        "`agy --version` failed: {}",
        String::from_utf8_lossy(&version.stderr)
    );

    let sandbox = make_failing_sandbox();
    let root = sandbox.path();
    let baseline = cargo_test(root);
    assert!(
        !baseline.status.success(),
        "fixture must be red before Antigravity runs"
    );

    let task = SupervisorTaskDescriptor {
        title: "Fix the multiply implementation".into(),
        description: Some(
            "Change only src/lib.rs so multiply returns the product. Run cargo test before finishing."
                .into(),
        ),
        depends_on: Vec::new(),
        scope_paths: vec!["src/lib.rs".into()],
    };
    let driver = SupervisorDriver::Scripted(ScriptedSupervisor::new(vec![
        vec![SupervisorOutput::Intent(SupervisorIntent::Decompose {
            tasks: vec![task],
            overview: None,
            tech_stack: None,
            envelope_fit: None,
        })],
        vec![SupervisorOutput::Intent(
            SupervisorIntent::DeclareComplete {
                summary: "multiply implementation repaired and verified".into(),
            },
        )],
    ]));
    let spec = MissionSpec {
        title: "Antigravity real-CLI gate".into(),
        objective: "Repair the failing multiply implementation and leave the fixture tests green."
            .into(),
        target_ref: "main".into(),
        tests: Some("cargo test".into()),
        supervisor_model: Some("claude".into()),
        worker_model: Some("antigravity".into()),
        worker_count: Some(1),
        confirm_plan: None,
        scope_paths: vec!["src/lib.rs".into()],
    };
    let workspace = MissionWorkspace::new(root.to_path_buf(), "real-antigravity-0001".into())
        .expect("mission workspace");
    let runtime = MissionRuntime::start_supervised_with(
        spec,
        workspace,
        driver,
        WorkerBackend::RealCli(WorkerVendor::Antigravity),
    )
    .await
    .expect("start Antigravity mission");

    let events = drain_to_terminal(&runtime).await;
    assert_eq!(
        runtime.state(),
        MissionState::CompletePendingMerge,
        "Antigravity mission did not complete: {events:#?}"
    );
    assert!(
        events
            .iter()
            .any(|event| matches!(event.kind, MissionEventKind::WorkerProgress { .. })),
        "Antigravity adapter emitted no canonical worker progress: {events:?}"
    );
    assert!(
        events.iter().any(|event| matches!(
            &event.kind,
            MissionEventKind::WorkerResultSubmitted { files, .. }
                if files.iter().any(|path| path == "src/lib.rs")
        )),
        "Antigravity submission did not contain src/lib.rs: {events:?}"
    );
    assert!(
        events
            .iter()
            .any(|event| matches!(event.kind, MissionEventKind::Integrated { .. })),
        "Antigravity worker was not integrated: {events:?}"
    );

    runtime
        .resolve(ResolveAction::Merge)
        .await
        .expect("merge Antigravity mission");
    // Resolve advances the target ref atomically; the fixture's checked-out
    // worktree still has its pre-merge files until Git refreshes it.
    run_git(root, &["reset", "--hard", "main"]);
    let final_test = cargo_test(root);
    assert!(
        final_test.status.success(),
        "fixture remained red after Antigravity mission\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&final_test.stdout),
        String::from_utf8_lossy(&final_test.stderr)
    );

    eprintln!(
        "Antigravity gate passed with agy {}",
        String::from_utf8_lossy(&version.stdout).trim()
    );
}
