//! Lint scorer — rustfmt + clippy for Rust, biome for Node (task 9).
//!
//! Each tool is shelled out; missing tool = unscored (None), not 0.0,
//! so a project without the tool installed doesn't get penalised.
//! Final score routed through [`crate::audit::report::clamp_score`].

use crate::audit::process::{output_with_timeout, TimedCommandError};
use crate::audit::report::{clamp_score, LintScore};
use std::path::{Path, PathBuf};
use std::time::Duration;
use tokio::process::Command;

const LINT_TIMEOUT: Duration = Duration::from_secs(180);

#[derive(Debug, thiserror::Error)]
pub enum LintError {
    #[error("lint spawn failed: {0}")]
    Spawn(String),
    #[error("lint timed out after {0:?}")]
    Timeout(Duration),
}

pub async fn run_rust_lint(worktree: &Path) -> Result<LintScore, LintError> {
    let (fmt, warns) = tokio::try_join!(rustfmt_check(worktree), clippy_warnings(worktree))?;

    // Score: rustfmt clean (1.0) + clippy clean (1.0) → 1.0
    //        rustfmt dirty OR ≥1 clippy warning → 0.5 (one strike)
    //        both bad → 0.0
    let strikes = (!fmt.unwrap_or(true)) as u32 + (warns.unwrap_or(0) > 0) as u32;
    let raw_score = match strikes {
        0 => 1.0,
        1 => 0.5,
        _ => 0.0,
    };
    Ok(LintScore {
        rustfmt_clean: fmt,
        clippy_warnings: warns,
        biome_diagnostics: None,
        score: clamp_score(raw_score),
    })
}

async fn rustfmt_check(worktree: &Path) -> Result<Option<bool>, LintError> {
    let mut cmd = Command::new("cargo");
    cmd.arg("fmt")
        .arg("--all")
        .arg("--check")
        .current_dir(worktree);
    let output = match output_with_timeout(&mut cmd, LINT_TIMEOUT).await {
        Ok(out) => out,
        Err(TimedCommandError::Spawn(e)) if e.kind() == std::io::ErrorKind::NotFound => {
            // Tool not installed on this host — unscored, not penalised.
            return Ok(None);
        }
        Err(error) => return Err(map_command_error(error)),
    };
    // Exit 0 = clean; exit non-zero = formatting differences.
    Ok(Some(output.status.success()))
}

async fn clippy_warnings(worktree: &Path) -> Result<Option<u32>, LintError> {
    let mut cmd = Command::new("cargo");
    cmd.arg("clippy")
        .arg("--no-deps")
        .arg("--message-format=short")
        .arg("--")
        .arg("-D")
        .arg("warnings")
        .current_dir(worktree);
    let output = match output_with_timeout(&mut cmd, LINT_TIMEOUT).await {
        Ok(out) => out,
        Err(TimedCommandError::Spawn(e)) if e.kind() == std::io::ErrorKind::NotFound => {
            // Tool not installed on this host — unscored, not penalised.
            return Ok(None);
        }
        Err(error) => return Err(map_command_error(error)),
    };
    if output.status.success() {
        return Ok(Some(0));
    }
    // -D warnings turns every warning into an error. Count lines that
    // include "warning:" OR "error:" in stderr.
    let stderr = String::from_utf8_lossy(&output.stderr);
    let warns = stderr
        .lines()
        .filter(|l| {
            (l.contains("warning:") || l.contains("error:")) && !l.contains("aborting due to")
        })
        .count();
    Ok(Some(warns as u32))
}

pub async fn run_node_lint(worktree: &Path) -> Result<LintScore, LintError> {
    // Look for a standalone config or the package.json `biome` key;
    // without either, invoking Biome would invent a project policy.
    if !has_biome_config(worktree) {
        return Ok(unscored_lint());
    }

    let Some(biome) = local_biome_binary(worktree) else {
        // Never fetch or execute a registry package during an audit. Projects
        // that configure Biome but have not installed their pinned dependency
        // are unscored, matching the missing-tool contract above.
        return Ok(unscored_lint());
    };
    let mut cmd = Command::new(biome);
    cmd.arg("check").current_dir(worktree);

    let output = match output_with_timeout(&mut cmd, LINT_TIMEOUT).await {
        Ok(out) => out,
        Err(TimedCommandError::Spawn(e)) if e.kind() == std::io::ErrorKind::NotFound => {
            // A concurrently removed local install is unscored, not penalised.
            return Ok(unscored_lint());
        }
        Err(error) => return Err(map_command_error(error)),
    };

    let stderr = String::from_utf8_lossy(&output.stderr);
    let diagnostics = parse_biome_summary(&stderr);
    // Score tiers: clean run → 1.0; <10 diagnostics = minor issues
    // worth a soft strike (0.5); ≥10 = project-wide problems (0.0).
    let raw_score = if output.status.success() {
        1.0
    } else if diagnostics < 10 {
        0.5
    } else {
        0.0
    };
    Ok(LintScore {
        rustfmt_clean: None,
        clippy_warnings: None,
        biome_diagnostics: Some(diagnostics),
        score: clamp_score(raw_score),
    })
}

fn local_biome_binary(worktree: &Path) -> Option<PathBuf> {
    let bin = worktree.join("node_modules/.bin");
    [
        bin.join("biome"),
        bin.join("biome.exe"),
        bin.join("biome.cmd"),
    ]
    .into_iter()
    .find(|candidate| candidate.is_file())
}

fn map_command_error(error: TimedCommandError) -> LintError {
    match error {
        TimedCommandError::Spawn(error) => LintError::Spawn(error.to_string()),
        TimedCommandError::Timeout(duration) => LintError::Timeout(duration),
    }
}

fn has_biome_config(worktree: &Path) -> bool {
    if worktree.join("biome.json").is_file() || worktree.join("biome.jsonc").is_file() {
        return true;
    }
    std::fs::read_to_string(worktree.join("package.json"))
        .ok()
        .and_then(|body| serde_json::from_str::<serde_json::Value>(&body).ok())
        .is_some_and(|package| package.get("biome").is_some())
}

/// Unscored LintScore used by both Node and Rust paths when a tool
/// or config is unavailable. All fields `None`; score 1.0 (no
/// penalty for missing tools per module contract).
fn unscored_lint() -> LintScore {
    LintScore {
        rustfmt_clean: None,
        clippy_warnings: None,
        biome_diagnostics: None,
        score: clamp_score(1.0),
    }
}

/// Parse Biome's "Found N errors, M warnings" summary line.
///
/// Biome's `check` command always ends with a single summary line of
/// the form "Found <n> errors, <m> warnings" (or similar). Counting
/// raw stderr lines is wrong because the summary itself is one line
/// while each diagnostic spans multiple lines. We extract the
/// numbers from the summary directly.
fn parse_biome_summary(stderr: &str) -> u32 {
    let mut total = 0u32;
    for line in stderr.lines() {
        if !line.contains("Found ") {
            continue;
        }
        // Match patterns like "Found 3 errors", "Found 12 warnings",
        // "Found 5 errors, 2 warnings". Walk tokens after each
        // recognised keyword ("error", "warning", "diagnostic") and
        // sum the preceding integer token.
        let tokens: Vec<&str> = line.split_whitespace().collect();
        for (i, t) in tokens.iter().enumerate() {
            let is_keyword =
                t.starts_with("error") || t.starts_with("warning") || t.starts_with("diagnostic");
            if !is_keyword || i == 0 {
                continue;
            }
            if let Ok(n) = tokens[i - 1].trim_end_matches(',').parse::<u32>() {
                total += n;
            }
        }
    }
    total
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use tempfile::tempdir;

    fn write_clean_rust_project(root: &std::path::Path) {
        fs::write(
            root.join("Cargo.toml"),
            r#"[package]
name = "lint_test_fixture"
version = "0.0.1"
edition = "2021"
"#,
        )
        .unwrap();
        fs::create_dir_all(root.join("src")).unwrap();
        fs::write(
            root.join("src").join("lib.rs"),
            "pub fn add(a: i32, b: i32) -> i32 {\n    a + b\n}\n",
        )
        .unwrap();
    }

    #[tokio::test]
    async fn clean_rust_project_is_lint_clean() {
        let dir = tempdir().unwrap();
        write_clean_rust_project(dir.path());
        let score = run_rust_lint(dir.path()).await.unwrap();
        assert_eq!(score.rustfmt_clean, Some(true));
        assert_eq!(score.clippy_warnings, Some(0));
        assert_eq!(score.score, 1.0);
    }

    #[tokio::test]
    async fn missing_biome_returns_none_diagnostics() {
        // Project with no biome config; we expect None (unscored), not 0.
        let dir = tempdir().unwrap();
        fs::write(
            dir.path().join("package.json"),
            r#"{"name":"x","version":"0.0.1"}"#,
        )
        .unwrap();
        let score = run_node_lint(dir.path()).await.unwrap();
        assert!(score.biome_diagnostics.is_none());
        assert_eq!(score.score, 1.0);
    }

    #[tokio::test]
    async fn configured_but_uninstalled_biome_is_unscored_without_fetching() {
        let dir = tempdir().unwrap();
        fs::write(
            dir.path().join("package.json"),
            r#"{"name":"x","biome":{"formatter":{"enabled":true}}}"#,
        )
        .unwrap();

        let score = run_node_lint(dir.path()).await.unwrap();

        assert!(score.biome_diagnostics.is_none());
        assert_eq!(score.score, 1.0);
    }

    #[test]
    fn parse_biome_summary_extracts_count() {
        assert_eq!(parse_biome_summary("Found 3 errors, 0 warnings\n"), 3);
        assert_eq!(parse_biome_summary("Found 12 warnings\n"), 12);
        assert_eq!(parse_biome_summary("Found 5 errors, 2 warnings\n"), 7);
        assert_eq!(parse_biome_summary(""), 0);
        assert_eq!(parse_biome_summary("Random output with no Found line\n"), 0);
    }

    #[test]
    fn detects_biome_configuration_in_package_json() {
        let dir = tempdir().unwrap();
        fs::write(
            dir.path().join("package.json"),
            r#"{"name":"x","biome":{"formatter":{"enabled":true}}}"#,
        )
        .unwrap();
        assert!(has_biome_config(dir.path()));
    }

    #[test]
    fn malformed_package_json_does_not_invent_biome_configuration() {
        let dir = tempdir().unwrap();
        fs::write(dir.path().join("package.json"), "not json").unwrap();
        assert!(!has_biome_config(dir.path()));
    }
}
