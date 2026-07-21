//! Smoke test for the orchestrator_audit CLI binary.
//!
//! Builds the binary and verifies --help exits 0 with a usage banner.

#[test]
fn audit_cli_binary_builds_and_prints_usage() {
    let status = std::process::Command::new(env!("CARGO"))
        .args([
            "build",
            "--bin",
            "orchestrator_audit",
            "-p",
            "vigla-orchestrator",
        ])
        .status()
        .expect("cargo build invocation");
    assert!(
        status.success(),
        "orchestrator_audit binary failed to build"
    );

    let bin = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../target/debug/orchestrator_audit");
    let output = std::process::Command::new(&bin)
        .arg("--help")
        .output()
        .expect("run --help");
    assert!(output.status.success(), "--help should exit 0");
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains("Usage:"), "expected usage banner in stdout");
}
