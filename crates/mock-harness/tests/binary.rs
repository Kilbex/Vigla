//! Subprocess test: spawn the compiled `mock-harness` binary, capture
//! stdout, and verify every emitted line parses as a canonical
//! [`Event`]. Closes the loop on the success criterion: "feeds the
//! output through the schema parser with zero errors."

use event_schema::Event;
use std::process::Command;

fn binary_path() -> &'static str {
    env!("CARGO_BIN_EXE_mock-harness")
}

fn run_script(script: &str) -> Vec<String> {
    let output = Command::new(binary_path())
        .args([
            "--script",
            script,
            "--speed",
            "0",
            "--worker-id",
            "test-worker",
            "--task-id",
            "test-task",
        ])
        .output()
        .expect("subprocess to launch");
    assert!(
        output.status.success(),
        "exit: {:?}\nstderr: {}",
        output.status,
        String::from_utf8_lossy(&output.stderr)
    );
    String::from_utf8(output.stdout)
        .expect("stdout to be UTF-8")
        .lines()
        .map(String::from)
        .collect()
}

#[test]
fn claude_happy_subprocess_yields_parseable_jsonl() {
    let lines = run_script("claude_happy");
    assert!(!lines.is_empty());
    for line in &lines {
        let _evt: Event =
            serde_json::from_str(line).unwrap_or_else(|e| panic!("parse failed for {line}: {e}"));
    }
}

#[test]
fn codex_blocked_subprocess_yields_parseable_jsonl() {
    let lines = run_script("codex_blocked");
    assert!(!lines.is_empty());
    for line in &lines {
        let _evt: Event =
            serde_json::from_str(line).unwrap_or_else(|e| panic!("parse failed for {line}: {e}"));
    }
}

#[test]
fn unknown_script_exits_with_code_2() {
    let output = Command::new(binary_path())
        .args(["--script", "doesnt_exist"])
        .output()
        .expect("subprocess to launch");
    assert!(!output.status.success(), "should fail on unknown script");
    assert_eq!(output.status.code(), Some(2));
}

#[test]
fn missing_script_arg_exits_with_code_2() {
    let output = Command::new(binary_path())
        .output()
        .expect("subprocess to launch");
    assert!(!output.status.success(), "should fail without --script");
    assert_eq!(output.status.code(), Some(2));
}

#[test]
fn rejects_negative_speed() {
    let output = Command::new(binary_path())
        .args(["--script", "claude_happy", "--speed", "-1"])
        .output()
        .expect("subprocess to launch");
    assert!(!output.status.success(), "negative speed should fail");
}
