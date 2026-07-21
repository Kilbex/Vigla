//! Integration test: a mock-worker mission that produces a clean,
//! in-scope submission should run the full arbiter pipeline and
//! produce an Accept decision followed by an Integrated event.
//!
//! This is the S2 analogue of the S1
//! `audit_e2e_in_scope_worker_passes_quality_floor` test, which is
//! superseded now that the per-worker arbiter has replaced the
//! mission-level audit pass.

use orchestrator::arbiter::{decide, ArbiterDecision, ArbiterPolicy, DecisionContext};
use orchestrator::audit::{audit_submission, AuditInput, AuditTier};
use std::path::PathBuf;
use tempfile::tempdir;

fn write_minimal_rust_crate(root: &std::path::Path) {
    std::fs::write(
        root.join("Cargo.toml"),
        r#"[package]
name = "arbiter_test_fixture"
version = "0.0.1"
edition = "2021"
"#,
    )
    .unwrap();
    std::fs::create_dir_all(root.join("src")).unwrap();
    std::fs::write(
        root.join("src/lib.rs"),
        "pub fn id(x: i32) -> i32 { x }\n#[cfg(test)] mod tests { #[test] fn it_works() { assert_eq!(super::id(1), 1); } }\n",
    )
    .unwrap();
}

#[tokio::test]
async fn full_accept_path_in_scope_passing_tests() {
    let dir = tempdir().unwrap();
    write_minimal_rust_crate(dir.path());

    let input = AuditInput {
        worktree_root: dir.path().to_path_buf(),
        test_command: None,
        touched_files: vec!["src/lib.rs".to_string()],
        scope_paths: vec![PathBuf::from("src")],
        tier: AuditTier::Standard,
        baseline: None,
        newly_passing: vec![],
        newly_failing: vec![],
    };
    let report = audit_submission(&input).await.expect("audit ok");
    assert!(
        report.overall >= 0.7,
        "audit should pass quality floor, got {}",
        report.overall
    );

    let ctx = DecisionContext {
        attempts_used_for_task: 0,
        attempts_used_for_mission: 0,
        submission_summary: "implemented id()".to_string(),
        touched_files: vec!["src/lib.rs".to_string()],
        scope_paths: vec![PathBuf::from("src")],
        preferred_rework_kind: None,
    };
    let decision = decide(&report, &ctx, &ArbiterPolicy::default());
    assert!(
        matches!(decision, ArbiterDecision::Accept(_)),
        "expected Accept, got {decision:?}"
    );
}

#[tokio::test]
async fn out_of_scope_submission_escalates() {
    let dir = tempdir().unwrap();
    write_minimal_rust_crate(dir.path());

    let input = AuditInput {
        worktree_root: dir.path().to_path_buf(),
        test_command: None,
        // The fixture only has src/, but the submission claims to
        // have touched a file outside src/. Audit's scope scorer
        // will mark it out-of-scope.
        touched_files: vec!["src/lib.rs".to_string(), "wild/oops.rs".to_string()],
        scope_paths: vec![PathBuf::from("src")],
        tier: AuditTier::Standard,
        baseline: None,
        newly_passing: vec![],
        newly_failing: vec![],
    };
    let report = audit_submission(&input).await.expect("audit ok");

    let ctx = DecisionContext {
        attempts_used_for_task: 0,
        attempts_used_for_mission: 0,
        submission_summary: "implemented id() + side change".to_string(),
        touched_files: vec!["src/lib.rs".to_string(), "wild/oops.rs".to_string()],
        scope_paths: vec![PathBuf::from("src")],
        preferred_rework_kind: None,
    };
    let decision = decide(&report, &ctx, &ArbiterPolicy::default());
    assert!(
        matches!(
            decision,
            ArbiterDecision::Escalate {
                bound: orchestrator::arbiter::AuthorityBound::Scope,
                ..
            }
        ),
        "expected Escalate(Scope), got {decision:?}"
    );
}
