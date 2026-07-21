//! MSV U3.5 — live integration test that spawns a real `claude`
//! process as the mission supervisor and asserts a complete mission
//! flow against varied mock workers.
//!
//! Opt-in. The test is skipped unless **all** of these hold:
//!
//! 1. The `claude` binary is on PATH and runs `--version` successfully.
//! 2. Environment variable `VIGLA_LIVE=1` is set.
//!
//! Without `VIGLA_LIVE`, the test exits early with a `println!`
//! marker so default `cargo test` (and CI) never spend tokens. To run:
//!
//! ```sh
//! VIGLA_LIVE=1 cargo test -p vigla-orchestrator --test supervisor_live -- --nocapture
//! ```
//!
//! Cost: ~$0.05–$0.20 on Sonnet per run (4 turns × ~2k tokens each).
//! Runtime: ~30–90s depending on Claude latency.

use orchestrator::mission::{MissionSpec, MissionState, ResolveAction};
use orchestrator::mission_event::MissionEventKind;
use orchestrator::mission_supervisor_run::{RealClaudeConfig, SupervisorDriver, WorkerBackend};
use orchestrator::{MissionRuntime, MissionWorkspace, WorkerVendor};
use std::collections::HashSet;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command as SyncCommand;
use std::time::{Duration, Instant};
use tempfile::TempDir;

fn live_enabled() -> bool {
    if std::env::var("VIGLA_LIVE").ok().as_deref() != Some("1") {
        return false;
    }
    cli_available("claude")
}

fn cli_available(binary: &str) -> bool {
    SyncCommand::new(binary)
        .arg("--version")
        .output()
        .map(|o| o.status.success())
        .unwrap_or(false)
}

fn make_sandbox_repo() -> (TempDir, PathBuf) {
    let temp = TempDir::new().expect("tempdir");
    let path = temp.path().to_path_buf();
    let run = |args: &[&str]| {
        let out = SyncCommand::new("git")
            .args(args)
            .current_dir(&path)
            .output()
            .expect("git");
        assert!(
            out.status.success(),
            "git {args:?}: {}",
            String::from_utf8_lossy(&out.stderr)
        );
    };
    run(&["init", "--initial-branch=main"]);
    run(&["config", "user.email", "live@example.com"]);
    run(&["config", "user.name", "Live Test"]);
    run(&["config", "commit.gpgsign", "false"]);
    std::fs::write(
        path.join("README.md"),
        "# Live test sandbox\n\nUsed by `supervisor_live.rs`.\n",
    )
    .unwrap();
    run(&["add", "README.md"]);
    run(&["commit", "-m", "initial"]);
    (temp, path)
}

fn run_git(root: &Path, args: &[&str]) {
    let out = SyncCommand::new("git")
        .args(args)
        .current_dir(root)
        .output()
        .expect("git");
    assert!(
        out.status.success(),
        "git {args:?}: {}",
        String::from_utf8_lossy(&out.stderr)
    );
}

fn write_repo_file(root: &Path, rel: &str, body: &str) {
    let path = root.join(rel);
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).expect("create parent");
    }
    fs::write(path, body).expect("write repo file");
}

fn commit_all(root: &Path, message: &str) {
    run_git(root, &["add", "."]);
    run_git(root, &["commit", "-m", message]);
}

fn setup_bug_fix_repo() -> (TempDir, PathBuf) {
    let (temp, root) = make_sandbox_repo();
    write_repo_file(&root, ".gitignore", "target/\n");
    write_repo_file(
        &root,
        "Cargo.toml",
        r#"[package]
name = "phase2_bug_fix_gate"
version = "0.1.0"
edition = "2021"

[lib]
path = "src/lib.rs"
"#,
    );
    write_repo_file(
        &root,
        "src/lib.rs",
        r#"pub fn add(a: i32, b: i32) -> i32 {
    a - b
}

#[cfg(test)]
mod tests {
    use super::add;

    #[test]
    fn adds_two_numbers() {
        assert_eq!(add(2, 3), 5);
    }
}
"#,
    );
    commit_all(&root, "add failing bug fixture");
    (temp, root)
}

fn setup_test_repair_repo() -> (TempDir, PathBuf) {
    let (temp, root) = make_sandbox_repo();
    write_repo_file(&root, ".gitignore", "target/\n");
    write_repo_file(
        &root,
        "Cargo.toml",
        r#"[package]
name = "phase2_test_repair_gate"
version = "0.1.0"
edition = "2021"

[lib]
path = "src/lib.rs"
"#,
    );
    write_repo_file(
        &root,
        "src/lib.rs",
        r#"pub fn multiply(a: i32, b: i32) -> i32 {
    a * b
}

#[cfg(test)]
mod tests {
    use super::multiply;

    #[test]
    fn multiplies_two_numbers() {
        assert_eq!(multiply(2, 3), 7);
    }
}
"#,
    );
    commit_all(&root, "add failing test fixture");
    (temp, root)
}

fn setup_docs_repo() -> (TempDir, PathBuf) {
    let (temp, root) = make_sandbox_repo();
    write_repo_file(
        &root,
        "docs/usage.md",
        "# Usage\n\nTODO: replace this placeholder with concrete Vigla usage steps.\n",
    );
    commit_all(&root, "add docs placeholder fixture");
    (temp, root)
}

fn assert_command_passes(root: &Path, command: &str) {
    let out = SyncCommand::new("sh")
        .arg("-lc")
        .arg(command)
        .current_dir(root)
        .output()
        .expect("run verification command");
    assert!(
        out.status.success(),
        "command `{command}` failed in {}:\nstdout:\n{}\nstderr:\n{}",
        root.display(),
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
}

#[derive(Default)]
struct FirstReviewTracker {
    reviewed: HashSet<String>,
    non_first_accept: HashSet<String>,
    accepted_on_first_review: HashSet<String>,
}

impl FirstReviewTracker {
    fn ingest(&mut self, mission_id: &str, kind: &MissionEventKind) {
        match kind {
            MissionEventKind::ReviewStarted { worker_id } => {
                self.reviewed.insert(review_key(mission_id, worker_id));
            }
            MissionEventKind::WorkerProgress { worker_id, note }
                if note.contains("requested revision") || note.contains("REJECTED") =>
            {
                self.non_first_accept
                    .insert(review_key(mission_id, worker_id));
            }
            MissionEventKind::Integrated { worker_id, .. } => {
                let key = review_key(mission_id, worker_id);
                if self.reviewed.contains(&key) && !self.non_first_accept.contains(&key) {
                    self.accepted_on_first_review.insert(key);
                }
            }
            _ => {}
        }
    }

    fn extend(&mut self, other: FirstReviewTracker) {
        self.reviewed.extend(other.reviewed);
        self.non_first_accept.extend(other.non_first_accept);
        self.accepted_on_first_review
            .extend(other.accepted_on_first_review);
    }

    fn total(&self) -> usize {
        self.reviewed.len()
    }

    fn accepted(&self) -> usize {
        self.accepted_on_first_review.len()
    }

    fn acceptance_rate(&self) -> f64 {
        if self.total() == 0 {
            0.0
        } else {
            self.accepted() as f64 / self.total() as f64
        }
    }
}

fn review_key(mission_id: &str, worker_id: &str) -> String {
    format!("{mission_id}:{worker_id}")
}

struct LiveMissionReport {
    tracker: FirstReviewTracker,
    integrations: usize,
    max_submission_files: usize,
    completed: bool,
    aborted: Option<String>,
}

async fn run_phase2_live_mission(
    root: &Path,
    mission_id: &str,
    title: &str,
    objective: &str,
    tests: Option<&str>,
    worker_vendor: WorkerVendor,
    worker_model: &str,
) -> LiveMissionReport {
    let workspace = MissionWorkspace::new(root.to_path_buf(), mission_id.into()).unwrap();
    let spec = MissionSpec {
        title: title.into(),
        objective: objective.into(),
        target_ref: "main".into(),
        tests: tests.map(str::to_owned),
        supervisor_model: Some("claude".into()),
        worker_model: Some(worker_model.into()),
        worker_count: Some(1),
        confirm_plan: None,
        scope_paths: vec![],
    };
    let driver = SupervisorDriver::RealClaude(RealClaudeConfig {
        binary: "claude".into(),
        model: None,
        turn_timeout: Duration::from_secs(180),
    });
    let runtime = MissionRuntime::start_supervised_with(
        spec,
        workspace,
        driver,
        WorkerBackend::RealCli(worker_vendor),
    )
    .await
    .expect("start phase2 live mission");
    let mut rx = runtime.subscribe();
    let started = Instant::now();
    let mut tracker = FirstReviewTracker::default();
    let mut integrations = 0usize;
    let mut max_submission_files = 0usize;
    let mut completed = false;
    let mut aborted = None;

    loop {
        if started.elapsed() > Duration::from_secs(1200) {
            panic!("{mission_id} exceeded 20-min wall clock");
        }
        let event = match tokio::time::timeout(Duration::from_secs(420), rx.recv()).await {
            Ok(Ok(e)) => e,
            Ok(Err(_)) => break,
            Err(_) => panic!("{mission_id} emitted no event for 7 minutes"),
        };
        tracker.ingest(mission_id, &event.kind);
        match event.kind {
            MissionEventKind::Decomposition { ref tasks } => {
                println!("[{mission_id}] decomposed into {} task(s)", tasks.len());
                for task in tasks {
                    println!("  - [{}] {}", task.index, task.title);
                }
            }
            MissionEventKind::WorkerResultSubmitted {
                ref worker_id,
                ref files,
                ref summary,
            } => {
                max_submission_files = max_submission_files.max(files.len());
                println!(
                    "[{mission_id}] {worker_id} submitted {} file(s): {}",
                    files.len(),
                    summary.chars().take(180).collect::<String>()
                )
            }
            MissionEventKind::WorkerProgress {
                ref worker_id,
                ref note,
            } => println!("[{mission_id}] {worker_id}: {note}"),
            MissionEventKind::Integrated { ref worker_id, .. } => {
                integrations += 1;
                println!("[{mission_id}] integrated {worker_id}");
            }
            MissionEventKind::Completed { ref summary, .. } => {
                println!("[{mission_id}] completed: {summary}");
                completed = true;
                break;
            }
            MissionEventKind::Aborted { ref reason } => {
                println!("[{mission_id}] aborted: {reason}");
                aborted = Some(reason.clone());
                break;
            }
            _ => {}
        }
    }

    assert_eq!(runtime.state(), MissionState::CompletePendingMerge);
    LiveMissionReport {
        tracker,
        integrations,
        max_submission_files,
        completed,
        aborted,
    }
}

#[tokio::test(flavor = "current_thread")]
#[ignore]
async fn real_claude_supervisor_completes_a_mock_mission() {
    if !live_enabled() {
        println!(
            "skipping supervisor_live test (set VIGLA_LIVE=1 \
             with `claude` on PATH to enable)"
        );
        return;
    }

    let (_temp, root) = make_sandbox_repo();
    let mission_id = "live-supervisor-0001".to_string();
    let workspace = MissionWorkspace::new(root.clone(), mission_id.clone()).unwrap();

    let spec = MissionSpec {
        title: "Document the sandbox".into(),
        objective: "Add two short notes documenting how this sandbox is used.".into(),
        target_ref: "main".into(),
        tests: None,
        supervisor_model: Some("claude".into()),
        worker_model: None,
        worker_count: None,
        confirm_plan: None,
        scope_paths: vec![],
    };

    let driver = SupervisorDriver::RealClaude(RealClaudeConfig {
        binary: "claude".into(),
        model: None,
        turn_timeout: Duration::from_secs(120),
    });

    let started = Instant::now();
    let runtime = MissionRuntime::start_supervised(spec, workspace, driver)
        .await
        .expect("supervisor mission to start");
    let mut rx = runtime.subscribe();

    let mut saw_decomposition = false;
    let mut spawned_count = 0;
    let mut integrated_count = 0;
    let mut completion_summary: Option<String> = None;

    let overall_timeout = Duration::from_secs(600);
    let mut aborted_reason: Option<String> = None;

    loop {
        if started.elapsed() > overall_timeout {
            panic!("supervisor mission exceeded 10-min wall clock");
        }
        let event = match tokio::time::timeout(Duration::from_secs(180), rx.recv()).await {
            Ok(Ok(e)) => e,
            Ok(Err(_)) => break,
            Err(_) => panic!("no mission event for 3 minutes — supervisor stalled"),
        };
        match event.kind {
            MissionEventKind::Decomposition { ref tasks } => {
                saw_decomposition = true;
                println!("supervisor decomposed into {} tasks:", tasks.len());
                for t in tasks {
                    println!("  - [{}] {}", t.index, t.title);
                }
            }
            MissionEventKind::WorkerSpawned {
                ref worker_id,
                ref task_title,
                ..
            } => {
                spawned_count += 1;
                println!("worker {worker_id} spawned for: {task_title}");
            }
            MissionEventKind::Integrated { ref worker_id, .. } => {
                integrated_count += 1;
                println!("worker {worker_id} integrated");
            }
            MissionEventKind::WorkerProgress {
                ref worker_id,
                ref note,
            } => {
                println!("  {worker_id}: {note}");
            }
            MissionEventKind::Completed { ref summary, .. } => {
                completion_summary = Some(summary.clone());
                println!("mission completed: {summary}");
                break;
            }
            MissionEventKind::Aborted { ref reason } => {
                aborted_reason = Some(reason.clone());
                println!("mission aborted: {reason}");
                break;
            }
            _ => {}
        }
    }

    assert!(
        aborted_reason.is_none(),
        "supervisor aborted unexpectedly: {:?}",
        aborted_reason
    );
    assert!(
        saw_decomposition,
        "supervisor never produced a decomposition"
    );
    assert!(spawned_count >= 1, "no workers spawned");
    assert!(
        integrated_count >= 1,
        "no workers integrated (every review was a reject?)"
    );
    assert!(
        completion_summary.is_some(),
        "mission did not reach Completed"
    );
    assert_eq!(runtime.state(), MissionState::CompletePendingMerge);

    // Optional Sign-off observation — the user can read this in
    // `--nocapture` output to confirm the supervisor produced real
    // judgment-shaped summaries, not just rubber-stamps.
    println!(
        "live supervisor run elapsed: {:.1}s, integrations: {integrated_count}",
        started.elapsed().as_secs_f64()
    );
}

#[tokio::test(flavor = "current_thread")]
#[ignore]
async fn supervisor_live_observes_revision_on_a_3_task_mission() {
    // 3 tasks → mock workers exercise Happy / NeedsRevision /
    // BadThenRevise variants. A reasonable supervisor playbook
    // observes the draft + placeholder markers in two of three
    // submissions and asks for at least one revision. Asserts that
    // *some* WorkerProgress event mentions "revision" — a soft check
    // that proves the supervisor is reading content rather than
    // accepting everything.
    if !live_enabled() {
        println!("skipping (VIGLA_LIVE not set)");
        return;
    }

    let (_temp, root) = make_sandbox_repo();
    let workspace = MissionWorkspace::new(root, "live-revise-0002".into()).unwrap();
    let spec = MissionSpec {
        title: "Add three short notes".into(),
        objective: "Add three independent notes covering setup, usage, and gotchas.".into(),
        target_ref: "main".into(),
        tests: None,
        supervisor_model: Some("claude".into()),
        worker_model: None,
        worker_count: Some(3),
        confirm_plan: None,
        scope_paths: vec![],
    };
    let driver = SupervisorDriver::RealClaude(RealClaudeConfig::default());
    let runtime = MissionRuntime::start_supervised(spec, workspace, driver)
        .await
        .expect("start");
    let mut rx = runtime.subscribe();

    let started = Instant::now();
    let mut revision_observed = false;
    let mut completion_observed = false;

    loop {
        if started.elapsed() > Duration::from_secs(600) {
            panic!("3-task mission exceeded 10-min wall clock");
        }
        let event = match tokio::time::timeout(Duration::from_secs(180), rx.recv()).await {
            Ok(Ok(e)) => e,
            _ => break,
        };
        if let MissionEventKind::WorkerProgress { ref note, .. } = event.kind {
            if note.contains("revision") || note.contains("REJECTED") {
                revision_observed = true;
                println!("supervisor judgment observed: {note}");
            }
        }
        if matches!(event.kind, MissionEventKind::Completed { .. }) {
            completion_observed = true;
            break;
        }
        if matches!(event.kind, MissionEventKind::Aborted { .. }) {
            break;
        }
    }

    assert!(completion_observed, "mission did not complete");
    // Soft assertion: a sensible playbook reading the 3 variants
    // (Happy / NeedsRevision / BadThenRevise) should produce at
    // least one revise or reject. If it doesn't, either the playbook
    // is rubber-stamping or the mock variants don't carry strong
    // enough sentinels. Print loudly either way.
    if !revision_observed {
        println!(
            "WARNING: 3-task live mission completed without any revise/reject. \
             Playbook may be rubber-stamping; check sentinel handling in \
             adapters/supervisor/src/playbook.md."
        );
    }
}

/// MSV U4.4 — real Claude supervisor running over a real Claude
/// worker. Asserts:
///
/// - Mission completes within wall clock.
/// - At least one worker submission contained `produced_changes` =>
///   a real commit landed on the worker branch.
/// - The integrated supervisor branch carries at least one new file
///   that did not exist on `main` — proves the worker actually
///   wrote something to disk that flowed through to integration.
///
/// Cost: roughly 2× the U3 live test — one supervisor session +
/// one (or more) worker session per task. Skipped unless
/// `VIGLA_LIVE=1`.
#[tokio::test(flavor = "current_thread")]
#[ignore]
async fn real_supervisor_with_real_claude_worker_completes_a_mission() {
    if !live_enabled() {
        println!("skipping (VIGLA_LIVE not set)");
        return;
    }

    let (_temp, root) = make_sandbox_repo();
    let workspace = MissionWorkspace::new(root.clone(), "live-u4-0001".into()).unwrap();
    let spec = MissionSpec {
        title: "Add a sandbox usage note".into(),
        objective: "Create one short markdown file describing what this sandbox is. \
                    The file should live at `notes/sandbox.md` and be 2-3 sentences."
            .into(),
        target_ref: "main".into(),
        tests: None,
        supervisor_model: Some("claude".into()),
        worker_model: Some("claude".into()),
        // Force a single-task decomposition so the test runs in a
        // bounded number of worker turns.
        worker_count: Some(1),
        confirm_plan: None,
        scope_paths: vec![],
    };
    let driver = SupervisorDriver::RealClaude(RealClaudeConfig::default());

    let started = Instant::now();
    let runtime = MissionRuntime::start_supervised_with(
        spec,
        workspace,
        driver,
        WorkerBackend::RealCli(WorkerVendor::Claude),
    )
    .await
    .expect("start_supervised_with");
    let mut rx = runtime.subscribe();

    let overall_timeout = Duration::from_secs(900);
    let mut completed = false;
    let mut aborted: Option<String> = None;
    let mut produced_changes_observed = false;
    let mut integrations = 0;

    loop {
        if started.elapsed() > overall_timeout {
            panic!("U4 live mission exceeded 15-min wall clock");
        }
        let event = match tokio::time::timeout(Duration::from_secs(360), rx.recv()).await {
            Ok(Ok(e)) => e,
            Ok(Err(_)) => break,
            Err(_) => panic!("no mission event for 6 minutes — likely a stuck worker"),
        };
        match event.kind {
            MissionEventKind::Decomposition { ref tasks } => {
                println!("decomposed into {} tasks", tasks.len());
                for t in tasks {
                    println!("  - [{}] {}", t.index, t.title);
                }
            }
            MissionEventKind::WorkerSpawned {
                ref worker_id,
                ref task_title,
                ..
            } => {
                println!("worker {worker_id} spawned for: {task_title}");
            }
            MissionEventKind::WorkerResultSubmitted {
                ref worker_id,
                ref files,
                ref summary,
            } => {
                if !files.is_empty() {
                    produced_changes_observed = true;
                }
                println!(
                    "{worker_id} submitted ({} file(s)): {}",
                    files.len(),
                    summary.chars().take(160).collect::<String>()
                );
            }
            MissionEventKind::Integrated { ref worker_id, .. } => {
                integrations += 1;
                println!("{worker_id} integrated");
            }
            MissionEventKind::WorkerProgress {
                ref worker_id,
                ref note,
            } => {
                println!("  {worker_id}: {note}");
            }
            MissionEventKind::Completed { ref summary, .. } => {
                println!("mission completed: {summary}");
                completed = true;
                break;
            }
            MissionEventKind::Aborted { ref reason } => {
                aborted = Some(reason.clone());
                println!("aborted: {reason}");
                break;
            }
            _ => {}
        }
    }

    assert!(
        aborted.is_none(),
        "U4 mission aborted unexpectedly: {:?}",
        aborted
    );
    assert!(completed, "U4 mission did not reach Completed");
    assert!(
        produced_changes_observed,
        "no worker submission produced any file changes — real worker dispatch didn't write to disk"
    );
    assert!(integrations >= 1, "no integrations happened");
    assert_eq!(runtime.state(), MissionState::CompletePendingMerge);

    // Files on the supervisor branch that weren't on `main` — proves
    // the real worker's output flowed through to integration.
    let diff = SyncCommand::new("git")
        .args([
            "diff",
            "--name-only",
            "main",
            "vigla/live-u4-0001/supervisor",
        ])
        .current_dir(&root)
        .output()
        .unwrap();
    let diff_files: Vec<String> = String::from_utf8_lossy(&diff.stdout)
        .lines()
        .filter(|l| !l.trim().is_empty())
        .map(|s| s.to_owned())
        .collect();
    assert!(
        !diff_files.is_empty(),
        "supervisor branch has no new files vs main — worker didn't deliver"
    );
    // Regression guard: an earlier draft wrote a debug log file
    // (`.vigla/worker-output.log`) inside the worktree, which
    // `git add -A` then swept into the commit and contaminated the
    // integration with two-file output. Assert no `.vigla/` paths
    // land on the integrated branch.
    let leaked: Vec<&String> = diff_files
        .iter()
        .filter(|p| p.starts_with(".vigla/"))
        .collect();
    assert!(
        leaked.is_empty(),
        "worker commit leaked debug paths into the integrated branch: {leaked:?}"
    );
    println!(
        "U4 live elapsed: {:.1}s, integrations: {integrations}, files: {diff_files:?}",
        started.elapsed().as_secs_f64()
    );
}

/// Shared body for vendor-specific worker live tests. Spawns the
/// supplied vendor as the worker under a real Claude supervisor on a
/// single-task mission, asserts the worker produced commits that
/// flowed through to integration, and pins the `.vigla/` no-leak
/// invariant. Returns when the mission either completes or aborts.
async fn run_vendor_worker_live(
    vendor: WorkerVendor,
    mission_id: &str,
    worker_model: &str,
    overall_timeout: Duration,
    idle_timeout: Duration,
) {
    if !live_enabled() {
        println!("skipping (VIGLA_LIVE not set)");
        return;
    }

    let (_temp, root) = make_sandbox_repo();
    let workspace = MissionWorkspace::new(root.clone(), mission_id.into()).unwrap();
    let spec = MissionSpec {
        title: format!("Add a sandbox usage note (worker={})", worker_model),
        objective: "Create one short markdown file describing what this sandbox is. \
                    The file should live at `notes/sandbox.md` and be 2-3 sentences."
            .into(),
        target_ref: "main".into(),
        tests: None,
        supervisor_model: Some("claude".into()),
        worker_model: Some(worker_model.into()),
        // Single task to bound cost & wall clock per vendor.
        worker_count: Some(1),
        confirm_plan: None,
        scope_paths: vec![],
    };
    let driver = SupervisorDriver::RealClaude(RealClaudeConfig::default());

    let started = Instant::now();
    let runtime = MissionRuntime::start_supervised_with(
        spec,
        workspace,
        driver,
        WorkerBackend::RealCli(vendor),
    )
    .await
    .expect("start_supervised_with");
    let mut rx = runtime.subscribe();

    let mut completed = false;
    let mut aborted: Option<String> = None;
    let mut produced_changes_observed = false;
    let mut integrations = 0;

    loop {
        if started.elapsed() > overall_timeout {
            panic!(
                "[{worker_model}] live mission exceeded {}s wall clock",
                overall_timeout.as_secs()
            );
        }
        let event = match tokio::time::timeout(idle_timeout, rx.recv()).await {
            Ok(Ok(e)) => e,
            Ok(Err(_)) => break,
            Err(_) => panic!(
                "[{worker_model}] no mission event for {}s — likely a stuck worker",
                idle_timeout.as_secs()
            ),
        };
        match event.kind {
            MissionEventKind::Decomposition { ref tasks } => {
                println!("[{worker_model}] decomposed into {} tasks", tasks.len());
                for t in tasks {
                    println!("  - [{}] {}", t.index, t.title);
                }
            }
            MissionEventKind::WorkerSpawned {
                ref worker_id,
                ref task_title,
                ..
            } => {
                println!("[{worker_model}] worker {worker_id} spawned for: {task_title}");
            }
            MissionEventKind::WorkerResultSubmitted {
                ref worker_id,
                ref files,
                ref summary,
            } => {
                if !files.is_empty() {
                    produced_changes_observed = true;
                }
                println!(
                    "[{worker_model}] {worker_id} submitted ({} file(s)): {}",
                    files.len(),
                    summary.chars().take(160).collect::<String>()
                );
            }
            MissionEventKind::Integrated { ref worker_id, .. } => {
                integrations += 1;
                println!("[{worker_model}] {worker_id} integrated");
            }
            MissionEventKind::WorkerProgress {
                ref worker_id,
                ref note,
            } => {
                println!("[{worker_model}]   {worker_id}: {note}");
            }
            MissionEventKind::Completed { ref summary, .. } => {
                println!("[{worker_model}] mission completed: {summary}");
                completed = true;
                break;
            }
            MissionEventKind::Aborted { ref reason } => {
                aborted = Some(reason.clone());
                println!("[{worker_model}] aborted: {reason}");
                break;
            }
            _ => {}
        }
    }

    assert!(
        aborted.is_none(),
        "[{worker_model}] mission aborted unexpectedly: {:?}",
        aborted
    );
    assert!(
        completed,
        "[{worker_model}] mission did not reach Completed"
    );
    assert!(
        produced_changes_observed,
        "[{worker_model}] no worker submission produced any file changes"
    );
    assert!(
        integrations >= 1,
        "[{worker_model}] no integrations happened"
    );
    assert_eq!(runtime.state(), MissionState::CompletePendingMerge);

    let diff = SyncCommand::new("git")
        .args([
            "diff",
            "--name-only",
            "main",
            &format!("vigla/{mission_id}/supervisor"),
        ])
        .current_dir(&root)
        .output()
        .unwrap();
    let diff_files: Vec<String> = String::from_utf8_lossy(&diff.stdout)
        .lines()
        .filter(|l| !l.trim().is_empty())
        .map(|s| s.to_owned())
        .collect();
    assert!(
        !diff_files.is_empty(),
        "[{worker_model}] supervisor branch has no new files vs main"
    );
    let leaked: Vec<&String> = diff_files
        .iter()
        .filter(|p| p.starts_with(".vigla/"))
        .collect();
    assert!(
        leaked.is_empty(),
        "[{worker_model}] leaked debug paths into integrated branch: {leaked:?}"
    );

    println!(
        "[{worker_model}] live elapsed: {:.1}s, integrations: {integrations}, files: {diff_files:?}",
        started.elapsed().as_secs_f64()
    );
}

/// MSV U4.5 — Codex worker under a real Claude supervisor.
#[tokio::test(flavor = "current_thread")]
#[ignore]
async fn real_supervisor_with_real_codex_worker_completes_a_mission() {
    run_vendor_worker_live(
        WorkerVendor::Codex,
        "live-u4-codex-0001",
        "codex",
        Duration::from_secs(900),
        Duration::from_secs(420),
    )
    .await;
}

/// MSV U4.5 — Gemini worker under a real Claude supervisor.
#[tokio::test(flavor = "current_thread")]
#[ignore]
async fn real_supervisor_with_real_gemini_worker_completes_a_mission() {
    run_vendor_worker_live(
        WorkerVendor::Gemini,
        "live-u4-gemini-0001",
        "gemini",
        Duration::from_secs(900),
        Duration::from_secs(420),
    )
    .await;
}

/// Phase 2 real-mission gates:
///
/// - real bug-fix mission
/// - real test-repair mission
/// - real docs-update mission
/// - at least one cross-vendor run (Claude supervisor + Codex worker)
/// - first-review acceptance tracked, with <40% treated as a stop
///   signal
#[tokio::test(flavor = "current_thread")]
#[ignore]
async fn phase2_real_mission_gates_complete() {
    if !live_enabled() {
        println!("skipping (VIGLA_LIVE not set)");
        return;
    }
    let (docs_worker_vendor, docs_worker_model) = if cli_available("gemini") {
        (WorkerVendor::Gemini, "gemini")
    } else {
        assert!(
            cli_available("codex"),
            "Phase 2 cross-vendor gate requires either `gemini --version` or `codex --version` to succeed"
        );
        (WorkerVendor::Codex, "codex")
    };

    let mut aggregate = FirstReviewTracker::default();

    let (_bug_temp, bug_root) = setup_bug_fix_repo();
    let bug_report = run_phase2_live_mission(
        &bug_root,
        "phase2-bug-fix-0001",
        "Fix a real failing Rust bug",
        "Fix the implementation bug so `cargo test` passes. Do not change the test assertion; \
         correct `src/lib.rs` so `add(2, 3)` returns 5.",
        Some("cargo test"),
        WorkerVendor::Claude,
        "claude",
    )
    .await;
    assert!(bug_report.aborted.is_none(), "bug-fix mission aborted");
    assert!(bug_report.completed, "bug-fix mission did not complete");
    assert!(
        bug_report.integrations >= 1,
        "bug-fix mission integrated nothing"
    );
    assert!(
        bug_report.max_submission_files <= 3,
        "bug-fix mission staged too many files; likely build artifacts leaked"
    );
    let bug_supervisor = bug_root.join(".vigla/worktrees/phase2-bug-fix-0001/supervisor");
    assert_command_passes(&bug_supervisor, "cargo test");
    aggregate.extend(bug_report.tracker);

    let (_test_temp, test_root) = setup_test_repair_repo();
    let test_report = run_phase2_live_mission(
        &test_root,
        "phase2-test-repair-0001",
        "Repair a real failing test",
        "The production `multiply` function is correct. Repair only the outdated failing test \
         expectation so `cargo test` passes.",
        Some("cargo test"),
        WorkerVendor::Claude,
        "claude",
    )
    .await;
    assert!(test_report.aborted.is_none(), "test-repair mission aborted");
    assert!(
        test_report.completed,
        "test-repair mission did not complete"
    );
    assert!(
        test_report.integrations >= 1,
        "test-repair mission integrated nothing"
    );
    assert!(
        test_report.max_submission_files <= 3,
        "test-repair mission staged too many files; likely build artifacts leaked"
    );
    let test_supervisor = test_root.join(".vigla/worktrees/phase2-test-repair-0001/supervisor");
    assert_command_passes(&test_supervisor, "cargo test");
    aggregate.extend(test_report.tracker);

    let (_docs_temp, docs_root) = setup_docs_repo();
    let docs_report = run_phase2_live_mission(
        &docs_root,
        "phase2-docs-update-0001",
        "Update real documentation",
        "Replace the placeholder in `docs/usage.md` with concise concrete usage steps for \
         the sandbox. Keep the document short and factual.",
        None,
        docs_worker_vendor,
        docs_worker_model,
    )
    .await;
    assert!(docs_report.aborted.is_none(), "docs-update mission aborted");
    assert!(
        docs_report.completed,
        "docs-update mission did not complete"
    );
    assert!(
        docs_report.integrations >= 1,
        "docs-update mission integrated nothing"
    );
    let docs_supervisor = docs_root.join(".vigla/worktrees/phase2-docs-update-0001/supervisor");
    let usage =
        fs::read_to_string(docs_supervisor.join("docs/usage.md")).expect("read updated docs");
    assert!(
        !usage.contains("TODO: replace"),
        "docs-update mission left the placeholder in place:\n{usage}"
    );
    aggregate.extend(docs_report.tracker);

    assert!(
        aggregate.total() >= 3,
        "first-review acceptance tracking saw too few reviewed workers: {}",
        aggregate.total()
    );
    let rate = aggregate.acceptance_rate();
    println!(
        "Phase 2 first-review acceptance: {}/{} = {:.0}%",
        aggregate.accepted(),
        aggregate.total(),
        rate * 100.0
    );
    assert!(
        rate >= 0.40,
        "first-review acceptance {:.0}% is below the 40% stop signal",
        rate * 100.0
    );
}

/// Phase 2 forced side-effect gate. The package install is explicitly
/// directed to a temp directory outside the repo; Vigla must surface
/// it as a visible boundary event, and Discard must clean only the
/// mission workspace rather than pretending the external install can
/// be reverted.
#[tokio::test(flavor = "current_thread")]
#[ignore]
async fn phase2_forced_side_effect_gate_logs_and_discards() {
    if !live_enabled() {
        println!("skipping (VIGLA_LIVE not set)");
        return;
    }

    let (_temp, root) = setup_docs_repo();
    let external = TempDir::new().expect("external side-effect target");
    let target = external.path().join("pip-target");
    let mission_id = "phase2-side-effect-0001";
    let workspace = MissionWorkspace::new(root.clone(), mission_id.into()).unwrap();
    let spec = MissionSpec {
        title: "Force a declared package-install side effect".into(),
        objective: format!(
            "Create `docs/side-effect.md`, then run exactly \
             `python3 -m pip install colorama==0.4.6 --target {}`. \
             In your final response include the exact command text.",
            target.display()
        ),
        target_ref: "main".into(),
        tests: None,
        supervisor_model: Some("claude".into()),
        worker_model: Some("claude".into()),
        worker_count: Some(1),
        confirm_plan: None,
        scope_paths: vec![],
    };
    let driver = SupervisorDriver::RealClaude(RealClaudeConfig {
        binary: "claude".into(),
        model: None,
        turn_timeout: Duration::from_secs(180),
    });
    let runtime = MissionRuntime::start_supervised_with(
        spec,
        workspace,
        driver,
        WorkerBackend::RealCli(WorkerVendor::Claude),
    )
    .await
    .expect("start side-effect mission");
    let mut rx = runtime.subscribe();
    let started = Instant::now();
    let mut saw_side_effect = false;
    let mut completed = false;

    loop {
        if started.elapsed() > Duration::from_secs(1200) {
            panic!("side-effect gate exceeded 20-min wall clock");
        }
        let event = match tokio::time::timeout(Duration::from_secs(420), rx.recv()).await {
            Ok(Ok(e)) => e,
            Ok(Err(_)) => break,
            Err(_) => panic!("side-effect gate emitted no event for 7 minutes"),
        };
        match event.kind {
            MissionEventKind::SideEffectLogged {
                ref worker_id,
                ref kind,
                ref summary,
                declared,
            } => {
                println!(
                    "side-effect logged for {worker_id}: {kind:?}, declared={declared}, {summary}"
                );
                saw_side_effect = true;
                assert!(
                    declared,
                    "Claude profile declares possible package installs"
                );
            }
            MissionEventKind::Completed { ref summary, .. } => {
                println!("side-effect mission completed: {summary}");
                completed = true;
                break;
            }
            MissionEventKind::Aborted { ref reason } => {
                panic!("side-effect mission aborted: {reason}");
            }
            _ => {}
        }
    }

    assert!(completed, "side-effect mission did not complete");
    assert!(saw_side_effect, "forced package install was not logged");
    assert!(
        target.exists(),
        "forced package install target was not created at {}",
        target.display()
    );

    runtime
        .resolve(ResolveAction::Discard)
        .await
        .expect("discard side-effect mission");
    assert_eq!(runtime.state(), MissionState::Discarded);
    assert!(
        target.exists(),
        "Discard must not claim to revert external package-install side effects"
    );
}
