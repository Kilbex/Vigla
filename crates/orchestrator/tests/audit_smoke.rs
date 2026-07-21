//! Smoke test for the audit entry point.

use orchestrator::audit::{audit_submission, AuditInput, AuditTier};
use std::fs;
use tempfile::tempdir;

#[tokio::test]
async fn smoke_audit_on_clean_rust_project_scores_above_threshold() {
    let dir = tempdir().unwrap();
    fs::write(
        dir.path().join("Cargo.toml"),
        r#"[package]
name = "audit_e2e_fixture"
version = "0.0.1"
edition = "2021"
"#,
    )
    .unwrap();
    fs::create_dir_all(dir.path().join("src")).unwrap();
    fs::write(
        dir.path().join("src").join("lib.rs"),
        "pub fn id(x: i32) -> i32 {\n    x\n}\n",
    )
    .unwrap();

    let input = AuditInput {
        worktree_root: dir.path().to_path_buf(),
        test_command: None,
        touched_files: vec!["src/lib.rs".into()],
        scope_paths: vec!["src".into()],
        tier: AuditTier::Smoke,
        baseline: None,
        newly_passing: vec![],
        newly_failing: vec![],
    };
    let report = audit_submission(&input).await.unwrap();
    assert!(
        report.overall >= 0.7,
        "expected ≥0.7, got {}",
        report.overall
    );
}
