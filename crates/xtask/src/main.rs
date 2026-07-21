//! Workspace task runner — the `cargo xtask` pattern.
//!
//! Primary purpose: make the workspace build/test self-contained.
//! `vigla-host` declares the *release* `mock-harness` binary as a Tauri
//! bundle resource, and `tauri_build` validates that path on EVERY compile of
//! the host crate — not just `tauri build`. So a bare `cargo test --workspace`
//! fails from a clean tree until that binary exists. (A `build.rs` placeholder
//! was tried and reverted: a build script that synthesizes a file named
//! `mock-harness` confuses cargo's workspace build and zeroes the real
//! binary.)
//!
//! `cargo xtask test` builds the release `mock-harness` first, then runs the
//! workspace tests — green from a clean checkout, no manual prerequisite.

use std::process::{Command, ExitCode};

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let Some((cmd, rest)) = args.split_first() else {
        print_help();
        return ExitCode::FAILURE;
    };

    let result = match cmd.as_str() {
        "test" => run_test(rest),
        "build" => run_build(rest),
        "clippy" => run_clippy(rest),
        "ci" => run_ci(),
        "receipt" => run_receipt(),
        "build-mock-harness" => ensure_mock_harness(),
        "help" | "-h" | "--help" => {
            print_help();
            Ok(())
        }
        other => {
            eprintln!("xtask: unknown command `{other}`\n");
            print_help();
            Err(2)
        }
    };

    match result {
        Ok(()) => ExitCode::SUCCESS,
        Err(code) => ExitCode::from(code as u8),
    }
}

fn print_help() {
    eprintln!(
        "cargo xtask <command> [-- extra cargo args]\n\
         \n\
         Commands:\n\
        \x20 test [args]          build release mock-harness, then `cargo test --workspace`\n\
        \x20 build [args]         build release mock-harness, then `cargo build --workspace`\n\
        \x20 clippy [args]        build release mock-harness, then clippy --workspace -D warnings\n\
        \x20 ci                   test + clippy (the full green gate)\n\
        \x20 receipt              reproduce the launch recovery receipt + atomic revert proof\n\
        \x20 build-mock-harness   just build the release mock-harness binary\n\
         \n\
         Why this exists: vigla-host bundles target/release/mock-harness as a\n\
         Tauri resource, which tauri_build validates on every compile, so a bare\n\
         `cargo test --workspace` fails from a clean tree until that binary\n\
         exists. These commands build it first."
    );
}

/// Build the release `mock-harness` binary that vigla-host's bundle config
/// (and therefore `tauri_build`) requires to exist on every host compile.
fn ensure_mock_harness() -> Result<(), i32> {
    cargo(&[
        "build",
        "-p",
        "vigla-mock-harness",
        "--release",
        "--bin",
        "mock-harness",
    ])
}

fn run_test(extra: &[String]) -> Result<(), i32> {
    ensure_mock_harness()?;
    let mut args = vec!["test", "--workspace"];
    args.extend(extra.iter().map(String::as_str));
    cargo(&args)
}

fn run_build(extra: &[String]) -> Result<(), i32> {
    ensure_mock_harness()?;
    let mut args = vec!["build", "--workspace"];
    args.extend(extra.iter().map(String::as_str));
    cargo(&args)
}

fn run_clippy(extra: &[String]) -> Result<(), i32> {
    ensure_mock_harness()?;
    let mut args = vec!["clippy", "--workspace", "--all-targets"];
    args.extend(extra.iter().map(String::as_str));
    args.extend(["--", "-D", "warnings"]);
    cargo(&args)
}

fn run_ci() -> Result<(), i32> {
    run_test(&[])?;
    run_clippy(&[])
}

fn run_receipt() -> Result<(), i32> {
    cargo(&[
        "test",
        "-p",
        "vigla-orchestrator",
        "--test",
        "launch_recovery_receipt",
        "--",
        "--nocapture",
    ])?;
    cargo(&[
        "test",
        "-p",
        "vigla-orchestrator",
        "--test",
        "revert_mission",
        "integrate_then_revert_restores_supervisor",
        "--",
        "--exact",
    ])
}

/// Run `cargo <args>` inheriting stdio; map a non-zero exit to `Err(code)`.
fn cargo(args: &[&str]) -> Result<(), i32> {
    // Respect the cargo that invoked us (set by `cargo run`), falling back to
    // `cargo` on PATH when xtask is run directly.
    let cargo = std::env::var("CARGO").unwrap_or_else(|_| "cargo".to_string());
    eprintln!("xtask: {} {}", cargo, args.join(" "));
    let status = Command::new(&cargo).args(args).status().map_err(|e| {
        eprintln!("xtask: failed to spawn `{cargo}`: {e}");
        1
    })?;
    if status.success() {
        Ok(())
    } else {
        Err(status.code().unwrap_or(1))
    }
}
