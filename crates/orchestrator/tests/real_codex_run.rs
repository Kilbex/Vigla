//! Step 13 — REAL Codex CLI integration test.
//!
//! Mirrors `real_claude_gate.rs` but uses Codex against the same
//! sandbox crate. Proves the adapter contract generalises: an
//! identical pipeline (Supervisor + Repository + sink) shepherds a
//! different vendor CLI to the same canonical event stream.

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

fn codex_available() -> bool {
    if std::env::var("VIGLA_SKIP_REAL_CODEX").is_ok() {
        return false;
    }
    Command::new("codex")
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

fn reset_sandbox(sandbox: &Path) {
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
#[ignore = "requires real `codex` CLI; opt in via `cargo test -- --ignored`"]
async fn real_codex_fixes_failing_multiply_test() {
    if !codex_available() {
        eprintln!("[step-13] skipping: codex CLI not available");
        return;
    }

    let sandbox = workspace_root().join("tests/samples/sandbox");
    reset_sandbox(&sandbox);

    let baseline = Command::new(env!("CARGO"))
        .args(["test", "--quiet"])
        .current_dir(&sandbox)
        .output()
        .expect("baseline cargo test");
    assert!(
        !baseline.status.success(),
        "sandbox baseline must fail before the run"
    );

    let repo = Repository::open_in_memory().await.unwrap();
    let sink: Arc<CapturingSink> = Arc::new(CapturingSink::default());
    let placeholder = sandbox.join(".unused-mock-harness");
    let supervisor = Supervisor::new(repo.clone(), Arc::clone(&sink) as _, placeholder);

    let prompt = "The current directory is a Rust crate. The function `multiply` in src/lib.rs is wrong: it returns a + b instead of a * b. Fix the body of the multiply function so all tests in tests/multiply.rs pass. Don't change anything else. Run `cargo test` to verify when done.";

    let started = Instant::now();
    let worker_id = supervisor
        .spawn_codex(prompt.into(), sandbox.clone())
        .await
        .expect("spawn_codex");

    let deadline = Instant::now() + Duration::from_secs(300);
    while Instant::now() < deadline && supervisor.is_running(&worker_id).await {
        tokio::time::sleep(Duration::from_millis(250)).await;
    }
    let elapsed = started.elapsed();
    assert!(
        !supervisor.is_running(&worker_id).await,
        "codex worker did not finish within 5 minutes (elapsed {elapsed:?})"
    );

    let events = repo.replay_for_worker(&worker_id).await.unwrap();
    assert!(!events.is_empty(), "codex produced no canonical events");

    match &events[0].kind {
        EventKind::StateChange(sc) => assert_eq!(sc.state, WorkerState::Idle),
        other => panic!("first event must be idle, got {other:?}"),
    }
    let last_state = events.iter().rev().find_map(|e| match &e.kind {
        EventKind::StateChange(sc) => Some(sc.state),
        _ => None,
    });
    let cost = events.iter().find_map(|e| match &e.kind {
        EventKind::Cost(c) => Some(c),
        _ => None,
    });
    eprintln!(
        "[step-13] codex run finished in {elapsed:?}, last state {:?}, {} events emitted",
        last_state,
        events.len()
    );
    if let Some(c) = cost {
        eprintln!(
            "[step-13] cost: {} input / {} output tokens (codex doesn't report USD)",
            c.input_tokens, c.output_tokens
        );
    }

    let final_test = Command::new(env!("CARGO"))
        .args(["test", "--quiet"])
        .current_dir(&sandbox)
        .output()
        .expect("final cargo test");
    if !final_test.status.success() {
        panic!(
            "sandbox tests still failing after codex run.\nstdout:\n{}\nstderr:\n{}",
            String::from_utf8_lossy(&final_test.stdout),
            String::from_utf8_lossy(&final_test.stderr)
        );
    }
    eprintln!("[step-13] PASS — sandbox tests green after codex run");
}
