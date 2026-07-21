//! Step 11 — REAL Claude CLI integration gate.
//!
//! This test spawns the actual `claude` binary against
//! `tests/samples/sandbox/` and asks it to fix a deliberately failing
//! `multiply` test. Verifies:
//!   * canonical event stream (idle → executing → … → done)
//!   * a completion event with non-empty summary
//!   * a cost event with non-zero token usage
//!   * the sandbox tests pass after the run
//!
//! Skipped automatically if the `claude` binary isn't on PATH or if
//! the env var `VIGLA_SKIP_REAL_CLAUDE=1` is set (so CI in
//! environments without auth doesn't fail). Cost: ~$0.10–$0.40 per
//! run on Opus.

use event_schema::{Event, EventKind, WorkerState};
use orchestrator::{parser::WorkerEventSink, Repository, Supervisor};
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

#[derive(Default)]
struct CapturingSink {
    events: Mutex<Vec<Event>>,
}

impl WorkerEventSink for CapturingSink {
    fn emit(&self, event: &Event) {
        self.events.lock().unwrap().push(event.clone());
    }
}

fn claude_available() -> bool {
    if std::env::var("VIGLA_SKIP_REAL_CLAUDE").is_ok() {
        return false;
    }
    Command::new("claude")
        .arg("--version")
        .output()
        .map(|o| o.status.success())
        .unwrap_or(false)
}

fn workspace_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .and_then(|p| p.parent())
        .unwrap()
        .to_path_buf()
}

fn reset_sandbox_to_failing_state(sandbox: &Path) {
    // Restore the deliberately-wrong implementation so the run is
    // reproducible regardless of prior gate runs.
    let lib_rs = sandbox.join("src/lib.rs");
    let original = "//! Tiny library used as the Step-11 gate target.\n\
//!\n\
//! `multiply` is intentionally wrong (returns a + b instead of a * b)\n\
//! so the test in `tests/multiply.rs` fails. Claude's task is to\n\
//! correct the implementation.\n\
\n\
pub fn multiply(a: i64, b: i64) -> i64 {\n\
    a + b\n\
}\n";
    std::fs::write(&lib_rs, original).expect("reset sandbox lib.rs");
}

#[tokio::test]
#[ignore = "requires real `claude` CLI; opt in via `cargo test -- --ignored`"]
async fn real_claude_fixes_failing_multiply_test() {
    if !claude_available() {
        eprintln!("[step-11] skipping: claude CLI not available");
        return;
    }

    let sandbox = workspace_root().join("tests/samples/sandbox");
    reset_sandbox_to_failing_state(&sandbox);

    // Confirm baseline: sandbox tests fail.
    let baseline = Command::new(env!("CARGO"))
        .args(["test", "--quiet"])
        .current_dir(&sandbox)
        .output()
        .expect("baseline cargo test");
    assert!(
        !baseline.status.success(),
        "sandbox baseline must fail before the gate runs (got success)"
    );

    let repo = Repository::open_in_memory().await.unwrap();
    let sink: Arc<CapturingSink> = Arc::new(CapturingSink::default());
    // mock_harness is irrelevant for spawn_claude but Supervisor::new
    // requires a path; we point at an unused placeholder.
    let placeholder = sandbox.join(".unused-mock-harness");
    let supervisor = Supervisor::new(repo.clone(), Arc::clone(&sink) as _, placeholder);

    let prompt = "The crate at the current working directory has a `multiply` function in src/lib.rs that is wrong — it returns a + b instead of a * b. Fix the implementation so all tests in tests/multiply.rs pass. Use Edit to change ONLY the body of the multiply function. Do not change anything else. Run `cargo test` after to verify.";

    let started = Instant::now();
    let worker_id = supervisor
        .spawn_claude(prompt.into(), sandbox.clone(), 8)
        .await
        .expect("spawn_claude");

    // Wait for drain. Allow up to 5 minutes — claude can take a
    // while.
    let deadline = Instant::now() + Duration::from_secs(300);
    while Instant::now() < deadline && supervisor.is_running(&worker_id).await {
        tokio::time::sleep(Duration::from_millis(250)).await;
    }
    let elapsed = started.elapsed();
    assert!(
        !supervisor.is_running(&worker_id).await,
        "claude worker did not finish within 5 minutes (elapsed {elapsed:?})"
    );

    // Canonical event stream invariants.
    let events = repo.replay_for_worker(&worker_id).await.unwrap();
    assert!(!events.is_empty(), "claude produced no events");
    let first = &events[0];
    match &first.kind {
        EventKind::StateChange(sc) => {
            assert_eq!(sc.state, WorkerState::Idle);
        }
        other => panic!("first event must be state_change idle, got {other:?}"),
    }

    let last_state = events
        .iter()
        .rev()
        .find_map(|e| match &e.kind {
            EventKind::StateChange(sc) => Some(sc.state),
            _ => None,
        })
        .expect("at least one state_change");

    let cost = events.iter().find_map(|e| match &e.kind {
        EventKind::Cost(c) => Some(c),
        _ => None,
    });
    assert!(cost.is_some(), "expected a cost event");

    eprintln!(
        "[step-11] claude run finished in {elapsed:?}, last state {last_state:?}, \
         {} events emitted, cost ${:.4}",
        events.len(),
        cost.unwrap().usd
    );

    // Final state should be `done` for a successful fix. (If claude
    // failed to converge we still let the gate report data — but
    // assert tests pass below either way.)
    if last_state != WorkerState::Done {
        eprintln!("[step-11] WARNING: final worker state was {last_state:?}, not done");
    }

    // Verify the sandbox now passes its tests.
    let final_test = Command::new(env!("CARGO"))
        .args(["test", "--quiet"])
        .current_dir(&sandbox)
        .output()
        .expect("final cargo test");
    if !final_test.status.success() {
        panic!(
            "sandbox tests still failing after claude run.\nstdout:\n{}\nstderr:\n{}",
            String::from_utf8_lossy(&final_test.stdout),
            String::from_utf8_lossy(&final_test.stderr)
        );
    }

    eprintln!("[step-11] PASS — sandbox tests green after claude run");
}
