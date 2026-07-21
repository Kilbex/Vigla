//! Pure-function coverage for recovery::classify_failure +
//! recovery::recover. Per-module tests live alongside each file;
//! this suite is the integration-level exhaustiveness check
//! (every FailureClass variant exercised through to a
//! RecoveryAction).

use std::path::PathBuf;
use std::time::Duration;

use event_schema::Vendor;
use orchestrator::mission_worker_dispatch::WorkerDispatchError;
use orchestrator::recovery::{
    classify::ClassifyContext,
    classify_failure,
    policy::RecoveryPolicy,
    recover,
    types::{
        CommandErrorKind, ContextRequest, ContextRequestKind, FailureClass, RecoveryAction,
        SupervisorRequestKind,
    },
    RecoveryHistory,
};

fn ctx() -> ClassifyContext {
    ClassifyContext {
        vendor: Vendor::Claude,
        touched_files: vec![],
        declared_scope: vec![],
        quota_signals: vec![],
        context_requests: vec![],
    }
}

#[test]
fn missing_file_first_pass_requests_supervisor() {
    let class = FailureClass::MissingFile {
        path: PathBuf::from("src/lib.rs"),
    };
    let action = recover(
        &class,
        &mut RecoveryHistory::new(),
        &RecoveryPolicy::default(),
        0,
    );
    assert!(matches!(
        action,
        RecoveryAction::RequestSupervisor {
            kind: SupervisorRequestKind::NeedsContext { .. },
        }
    ));
}

#[test]
fn missing_file_after_budget_escalates_scope() {
    let class = FailureClass::MissingFile {
        path: PathBuf::from("src/lib.rs"),
    };
    let mut h = RecoveryHistory::new();
    let first = recover(&class, &mut h, &RecoveryPolicy::default(), 0);
    assert!(matches!(first, RecoveryAction::RequestSupervisor { .. }));
    let action = recover(&class, &mut h, &RecoveryPolicy::default(), 0);
    assert!(matches!(
        action,
        RecoveryAction::Escalate {
            bound: orchestrator::arbiter::AuthorityBound::Scope,
            ..
        }
    ));
}

#[test]
fn timeout_classifies_as_vendor_crash() {
    let c = ctx();
    let err = WorkerDispatchError::Timeout(Duration::from_secs(900));
    let class = classify_failure(Some(&err), &c, 0, 0);
    assert!(matches!(
        class,
        FailureClass::VendorCrash { signal: true, .. }
    ));
}

#[test]
fn vendor_crash_uses_budget_then_escalates_risk() {
    let class = FailureClass::VendorCrash {
        vendor: Vendor::Gemini,
        last_exit_code: Some(139),
        signal: true,
    };
    let mut h = RecoveryHistory::new();
    // 2 retries default.
    assert!(matches!(
        recover(&class, &mut h, &RecoveryPolicy::default(), 0),
        RecoveryAction::Retry { .. }
    ));
    assert!(matches!(
        recover(&class, &mut h, &RecoveryPolicy::default(), 0),
        RecoveryAction::Retry { .. }
    ));
    assert!(matches!(
        recover(&class, &mut h, &RecoveryPolicy::default(), 0),
        RecoveryAction::Escalate {
            bound: orchestrator::arbiter::AuthorityBound::Risk,
            ..
        }
    ));
}

#[test]
fn quota_signal_yields_pause_regardless_of_dispatch_error() {
    let mut c = ctx();
    c.quota_signals
        .push(orchestrator::recovery::classify::QuotaSignal {
            vendor: Vendor::Claude,
            estimated_reset_at_ms: Some(2_000),
        });
    let err = WorkerDispatchError::Exit("died with code 1".into());
    let class = classify_failure(Some(&err), &c, 0, 1_000);
    let action = recover(
        &class,
        &mut RecoveryHistory::new(),
        &RecoveryPolicy::default(),
        1_000,
    );
    match action {
        RecoveryAction::Pause { until_unix_ms, .. } => {
            assert_eq!(until_unix_ms, 2_000);
        }
        other => panic!("expected Pause, got {other:?}"),
    }
}

#[test]
fn context_request_classifies_as_inadequate_context() {
    let mut c = ctx();
    c.context_requests.push(ContextRequest {
        kind: ContextRequestKind::Documentation,
        detail: "async-trait".into(),
    });
    let class = classify_failure(None, &c, 0, 0);
    assert!(matches!(class, FailureClass::InadequateContext { .. }));
    let action = recover(
        &class,
        &mut RecoveryHistory::new(),
        &RecoveryPolicy::default(),
        0,
    );
    assert!(matches!(
        action,
        RecoveryAction::RequestSupervisor {
            kind: SupervisorRequestKind::NeedsContext { .. },
        }
    ));
}

#[test]
fn permissions_dispatch_error_classifies_then_escalates_risk() {
    let c = ctx();
    let err = WorkerDispatchError::Io("EACCES \"/etc/passwd\"".into());
    let class = classify_failure(Some(&err), &c, 0, 0);
    assert!(matches!(class, FailureClass::Permissions { .. }));
    let action = recover(
        &class,
        &mut RecoveryHistory::new(),
        &RecoveryPolicy::default(),
        0,
    );
    assert!(matches!(
        action,
        RecoveryAction::Escalate {
            bound: orchestrator::arbiter::AuthorityBound::Risk,
            ..
        }
    ));
}

#[test]
fn transient_command_error_retries_once_then_escalates_quality() {
    let class = FailureClass::CommandError {
        exit_code: 75,
        kind: CommandErrorKind::Transient,
    };
    let policy = RecoveryPolicy::default();
    let mut h = RecoveryHistory::new();
    assert!(matches!(
        recover(&class, &mut h, &policy, 0),
        RecoveryAction::Retry { .. }
    ));
    assert!(matches!(
        recover(&class, &mut h, &policy, 0),
        RecoveryAction::Escalate {
            bound: orchestrator::arbiter::AuthorityBound::Quality,
            ..
        }
    ));
}
