//! Run the project's test command and score the result.
//!
//! Rust path uses `cargo test --no-fail-fast` and parses the
//! summary line. Node path is task 5 (separate file). The shape
//! returned is [`TestPassScore`] from `report.rs`. Final scores are
//! routed through [`crate::audit::report::clamp_score`] to enforce
//! the [0.0, 1.0] invariant.

use crate::audit::process::{output_with_timeout, TimedCommandError};
use crate::audit::report::{clamp_score, TestPassScore};
use std::path::Path;
use std::time::Duration;
use tokio::process::Command;

const CARGO_TIMEOUT: Duration = Duration::from_secs(300);
const CUSTOM_TIMEOUT: Duration = Duration::from_secs(300);

#[derive(Debug, thiserror::Error)]
pub enum TestPassError {
    #[error("test runner spawn failed: {0}")]
    Spawn(String),
    #[error("test runner timed out after {0:?}")]
    Timeout(Duration),
    /// Not currently emitted by `run_rust_tests` — reserved for the
    /// Node path (T5) and future strict-parse modes that reject
    /// unrecognised output.
    #[error("test output parse failed: {0}")]
    Parse(String),
}

pub async fn run_rust_tests(worktree: &Path) -> Result<TestPassScore, TestPassError> {
    let mut cmd = Command::new("cargo");
    cmd.arg("test")
        .arg("--no-fail-fast")
        .arg("--quiet")
        .current_dir(worktree);

    let output = output_with_timeout(&mut cmd, CARGO_TIMEOUT)
        .await
        .map_err(map_command_error)?;

    // cargo test prints "test result: ok. N passed; M failed; K ignored; ..."
    // on each test binary. Sum them.
    let combined = String::from_utf8_lossy(&output.stdout);
    let (passed, failed, skipped) = parse_cargo_summaries(&combined);

    let score = score_from_result(output.status.success(), passed, failed);

    Ok(TestPassScore {
        ran: true,
        passed,
        failed,
        skipped,
        score,
    })
}

fn parse_cargo_summaries(stdout: &str) -> (u32, u32, u32) {
    // Lines look like:
    //   test result: ok. 12 passed; 0 failed; 1 ignored; 0 measured; 0 filtered out;
    //   test result: FAILED. 3 passed; 2 failed; 0 ignored; 0 measured; 0 filtered out;
    let mut passed = 0u32;
    let mut failed = 0u32;
    let mut skipped = 0u32;
    for line in stdout.lines() {
        if !line.contains("test result:") {
            continue;
        }
        passed += extract_count(line, "passed");
        failed += extract_count(line, "failed");
        skipped += extract_count(line, "ignored");
    }
    (passed, failed, skipped)
}

fn extract_count(line: &str, keyword: &str) -> u32 {
    let Some(idx) = line.find(keyword) else {
        return 0;
    };
    let prefix = &line[..idx];
    let Some(num_str) = prefix.split_whitespace().last() else {
        return 0;
    };
    num_str.parse().unwrap_or(0)
}

const NODE_TIMEOUT: Duration = Duration::from_secs(180);

pub async fn run_node_tests(worktree: &Path) -> Result<TestPassScore, TestPassError> {
    let pkg = worktree.join("package.json");
    let pkg_body = match std::fs::read_to_string(&pkg) {
        Ok(b) => b,
        Err(_) => {
            return Ok(TestPassScore {
                ran: false,
                passed: 0,
                failed: 0,
                skipped: 0,
                score: 0.0,
            });
        }
    };

    // Cheap parse — look for `"test"` key under `scripts`. If absent,
    // there is no test command; report "did not run."
    let parsed: serde_json::Value = serde_json::from_str(&pkg_body)
        .map_err(|e| TestPassError::Parse(format!("package.json: {e}")))?;
    if parsed.get("scripts").and_then(|s| s.get("test")).is_none() {
        return Ok(TestPassScore {
            ran: false,
            passed: 0,
            failed: 0,
            skipped: 0,
            score: 0.0,
        });
    }

    let mut cmd = Command::new("npm");
    cmd.arg("test").arg("--silent").current_dir(worktree);

    let output = output_with_timeout(&mut cmd, NODE_TIMEOUT)
        .await
        .map_err(map_command_error)?;

    // Heuristic parse — many JS test runners print "N passing" / "N failing"
    // or "Tests: X passed, Y failed" (Jest, Vitest, Mocha). Falls back to
    // exit code if neither pattern matches.
    let combined = String::from_utf8_lossy(&output.stdout).to_string()
        + &String::from_utf8_lossy(&output.stderr);
    let (passed, failed) = parse_node_summary(&combined);
    let total_run = passed + failed;
    let score = score_from_result(output.status.success(), passed, failed);
    let ran = total_run > 0 || output.status.success();

    Ok(TestPassScore {
        ran,
        passed,
        failed,
        skipped: 0,
        score,
    })
}

/// Run the exact test command supplied in [`crate::mission::MissionSpec`].
/// The command comes from the local user, not a worker response. It runs in
/// the worker/supervisor worktree and inherits the timeout-safe process-group
/// handling used by the built-in runners.
pub async fn run_custom_tests(
    worktree: &Path,
    command: &str,
) -> Result<TestPassScore, TestPassError> {
    let mut cmd = shell_command(command);
    cmd.current_dir(worktree);
    let output = output_with_timeout(&mut cmd, CUSTOM_TIMEOUT)
        .await
        .map_err(map_command_error)?;

    let combined = String::from_utf8_lossy(&output.stdout).to_string()
        + &String::from_utf8_lossy(&output.stderr);
    let (cargo_passed, cargo_failed, skipped) = parse_cargo_summaries(&combined);
    let (passed, failed) = if cargo_passed + cargo_failed > 0 {
        (cargo_passed, cargo_failed)
    } else {
        parse_node_summary(&combined)
    };
    let score = score_from_result(output.status.success(), passed, failed);

    Ok(TestPassScore {
        ran: true,
        passed,
        failed,
        skipped,
        score,
    })
}

#[cfg(unix)]
fn shell_command(command: &str) -> Command {
    let mut shell = Command::new("sh");
    shell.arg("-c").arg(command);
    shell
}

#[cfg(windows)]
fn shell_command(command: &str) -> Command {
    let mut shell = Command::new("cmd");
    shell.arg("/C").arg(command);
    shell
}

fn map_command_error(error: TimedCommandError) -> TestPassError {
    match error {
        TimedCommandError::Spawn(error) => TestPassError::Spawn(error.to_string()),
        TimedCommandError::Timeout(duration) => TestPassError::Timeout(duration),
    }
}

fn score_from_result(success: bool, passed: u32, failed: u32) -> f64 {
    let total_run = passed + failed;
    let raw_score = if total_run == 0 {
        if success {
            1.0
        } else {
            0.0
        }
    } else if !success && failed == 0 {
        // Parsed passing counts do not override a command failure; setup,
        // collection, and teardown errors often arrive after test summaries.
        0.0
    } else if success && failed == 0 {
        1.0
    } else {
        // Test failures are a binary release gate. A weighted pass ratio can
        // hide one regression in a large suite once blended with scope/lint.
        0.0
    };
    clamp_score(raw_score)
}

fn parse_node_summary(out: &str) -> (u32, u32) {
    // Look for the most common patterns. First match wins per type.
    // "12 passing", "Tests: 12 passed", "✓ 12 passed" — all reduce to
    // a number adjacent to one of these keywords.
    let passed = scan_count(out, &["passing", "passed"]);
    let failed = scan_count(out, &["failing", "failed"]);
    (passed, failed)
}

fn scan_count(out: &str, keywords: &[&str]) -> u32 {
    for line in out.lines() {
        for kw in keywords {
            if let Some(idx) = line.find(kw) {
                let prefix = &line[..idx];
                if let Some(num_str) = prefix.split_whitespace().last() {
                    if let Ok(n) = num_str.parse::<u32>() {
                        return n;
                    }
                }
            }
        }
    }
    0
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use tempfile::tempdir;

    #[tokio::test]
    async fn rust_project_with_passing_test_scores_one() {
        let dir = tempdir().unwrap();
        write_minimal_cargo_project(dir.path(), /* failing */ false);

        let score = run_rust_tests(dir.path()).await.unwrap();
        assert!(score.ran);
        assert_eq!(score.failed, 0);
        assert!(score.passed >= 1);
        assert_eq!(score.score, 1.0);
    }

    #[tokio::test]
    async fn custom_test_command_is_executed_and_parsed() {
        let dir = tempdir().unwrap();
        let score = run_custom_tests(
            dir.path(),
            "printf 'test result: ok. 3 passed; 0 failed; 1 ignored; 0 measured; 0 filtered out;\\n'",
        )
        .await
        .unwrap();
        assert!(score.ran);
        assert_eq!(score.passed, 3);
        assert_eq!(score.failed, 0);
        assert_eq!(score.skipped, 1);
        assert_eq!(score.score, 1.0);
    }

    #[tokio::test]
    async fn failing_custom_test_command_scores_zero_without_parseable_counts() {
        let dir = tempdir().unwrap();
        let score = run_custom_tests(dir.path(), "printf failure >&2; exit 7")
            .await
            .unwrap();
        assert!(score.ran);
        assert_eq!(score.passed, 0);
        assert_eq!(score.failed, 0);
        assert_eq!(score.score, 0.0);
    }

    #[test]
    fn one_failure_in_a_large_suite_is_still_zero() {
        assert_eq!(score_from_result(false, 99, 1), 0.0);
        // Defensive against runners that return success while still printing
        // a failing summary.
        assert_eq!(score_from_result(true, 99, 1), 0.0);
    }

    #[test]
    fn setup_or_collection_failure_overrides_passing_counts() {
        assert_eq!(score_from_result(false, 99, 0), 0.0);
    }

    #[tokio::test]
    async fn failing_custom_command_cannot_hide_behind_passing_counts() {
        let dir = tempdir().unwrap();
        let score = run_custom_tests(
            dir.path(),
            "printf 'test result: ok. 3 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out;\\n'; exit 7",
        )
        .await
        .unwrap();
        assert_eq!(score.passed, 3);
        assert_eq!(score.failed, 0);
        assert_eq!(score.score, 0.0);
    }

    #[test]
    fn failed_runner_status_overrides_parsed_passing_counts() {
        assert_eq!(score_from_result(false, 12, 0), 0.0);
        assert_eq!(score_from_result(true, 12, 0), 1.0);
    }

    #[tokio::test]
    async fn rust_project_with_failing_test_scores_below_one() {
        let dir = tempdir().unwrap();
        write_minimal_cargo_project(dir.path(), /* failing */ true);

        let score = run_rust_tests(dir.path()).await.unwrap();
        assert!(score.ran);
        assert!(score.failed >= 1);
        assert!(score.score < 1.0);
    }

    #[tokio::test]
    async fn node_project_without_test_script_scores_zero_ran() {
        let dir = tempdir().unwrap();
        fs::write(
            dir.path().join("package.json"),
            r#"{"name": "x", "version": "0.0.1"}"#,
        )
        .unwrap();
        let score = run_node_tests(dir.path()).await.unwrap();
        assert!(!score.ran);
        assert_eq!(score.passed, 0);
        assert_eq!(score.failed, 0);
    }

    #[tokio::test]
    async fn node_project_with_passing_test_script_scores_one() {
        let dir = tempdir().unwrap();
        fs::write(
            dir.path().join("package.json"),
            r#"{
        "name": "x",
        "version": "0.0.1",
        "scripts": { "test": "node -e \"console.log('1 passing')\"" }
    }"#,
        )
        .unwrap();
        let score = run_node_tests(dir.path()).await.unwrap();
        assert!(score.ran);
        assert_eq!(score.failed, 0);
        assert_eq!(score.score, 1.0);
    }

    fn write_minimal_cargo_project(root: &std::path::Path, failing: bool) {
        fs::write(
            root.join("Cargo.toml"),
            r#"[package]
name = "audit_test_fixture"
version = "0.0.1"
edition = "2021"
"#,
        )
        .unwrap();
        fs::create_dir_all(root.join("src")).unwrap();
        let test_body = if failing {
            "#[test] fn t() { assert_eq!(1, 2); }"
        } else {
            "#[test] fn t() { assert_eq!(1, 1); }"
        };
        fs::write(root.join("src").join("lib.rs"), test_body).unwrap();
    }
}
