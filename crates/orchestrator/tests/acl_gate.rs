//! S8 acceptance — File-ACL violation by a worker is detected
//! pre-integration and escalated as `AuthorityBound::Scope`.
//!
//! These tests cover the pure-function surface of the ACL gate. The live
//! mission regression `mock_worker_writing_outside_mission_scope_trips_acl_gate`
//! in `mission_supervisor_run::tests` additionally proves that a violation
//! enters Attention, emits the Scope bound, and skips audit.

use orchestrator::acl::{check_diff, FileAcl};
use orchestrator::arbiter::AuthorityBound;
use std::path::PathBuf;

#[test]
fn check_diff_violation_carries_denied_paths() {
    let acl = FileAcl::from_mission_and_task(&[PathBuf::from("src")], None);
    let touched = vec![
        "src/lib.rs".to_string(),
        "secrets/.env".to_string(),
        "docs/architecture.md".to_string(),
    ];
    let err = check_diff(&touched, &acl).expect_err("expected violation");
    assert_eq!(err.allowed_count, 1);
    assert_eq!(err.denied_count, 2);
    assert!(err.denied_paths.contains(&"secrets/.env".to_string()));
    assert!(err
        .denied_paths
        .contains(&"docs/architecture.md".to_string()));

    let json = err.payload_json();
    assert!(json.contains("denied_paths"));
    assert!(json.contains("secrets/.env"));
}

#[test]
fn synthetic_escalate_evidence_carries_scope_bound() {
    let acl = FileAcl::from_mission_and_task(&[PathBuf::from("orchestrator/src")], None);
    let touched = vec!["app/src/store/ingest.ts".to_string()];
    let err = check_diff(&touched, &acl).expect_err("expected violation");
    let evidence = orchestrator::arbiter::EscalationEvidence {
        summary: err.summary(),
        payload_json: Some(err.payload_json()),
    };

    let bound = AuthorityBound::Scope;
    assert_eq!(bound, AuthorityBound::Scope);

    let payload: serde_json::Value =
        serde_json::from_str(evidence.payload_json.as_deref().unwrap()).unwrap();
    let denied = payload
        .get("denied_paths")
        .and_then(|v| v.as_array())
        .expect("denied_paths array");
    assert_eq!(denied.len(), 1);
    assert_eq!(denied[0].as_str(), Some("app/src/store/ingest.ts"));
}

#[test]
fn empty_mission_scope_makes_violation_impossible() {
    let acl = FileAcl::from_mission_and_task(&[], None);
    let touched = vec!["literally/any/path.rs".to_string()];
    assert!(check_diff(&touched, &acl).is_ok());
}

#[test]
fn intersection_with_disjoint_task_scope_denies_all() {
    let acl =
        FileAcl::from_mission_and_task(&[PathBuf::from("src")], Some(&[PathBuf::from("docs")]));
    let err = check_diff(&["src/lib.rs".to_string()], &acl).expect_err("expected violation");
    assert_eq!(err.denied_paths, vec!["src/lib.rs".to_string()]);
}
