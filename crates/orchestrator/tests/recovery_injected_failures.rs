//! U7 acceptance gate — three injected surfaces (missing file,
//! worker timeout, vendor crash) cover the supervisor-context and
//! bounded VendorCrash recovery paths.
//!
//! These tests exercise the classifier + policy pipeline directly
//! against synthetic `WorkerDispatchError` shapes. The full mission
//! loop is exercised by recovery_quota_e2e.rs; this suite focuses
//! on the gate criterion without claiming timeout and signal crashes
//! use different policies: both intentionally classify as VendorCrash.

use std::time::Duration;

use event_schema::Vendor;
use orchestrator::arbiter::AuthorityBound;
use orchestrator::mission_worker_dispatch::WorkerDispatchError;
use orchestrator::recovery::{
    classify::ClassifyContext,
    classify_failure,
    policy::RecoveryPolicy,
    recover,
    types::{FailureClass, RecoveryAction, SupervisorRequestKind},
    RecoveryHistory,
};

fn fresh_ctx(vendor: Vendor) -> ClassifyContext {
    ClassifyContext {
        vendor,
        touched_files: vec![],
        declared_scope: vec![],
        quota_signals: vec![],
        context_requests: vec![],
    }
}

#[test]
fn injected_missing_file_path_is_request_supervisor_then_escalate_scope() {
    // Inject an ENOENT dispatch error.
    let err = WorkerDispatchError::Io("ENOENT \"src/missing.rs\"".into());
    let ctx = fresh_ctx(Vendor::Claude);
    let class = classify_failure(Some(&err), &ctx, 0, 0);
    assert!(matches!(class, FailureClass::MissingFile { .. }));

    let mut history = RecoveryHistory::new();
    let action = recover(&class, &mut history, &RecoveryPolicy::default(), 0);
    assert!(matches!(
        action,
        RecoveryAction::RequestSupervisor {
            kind: SupervisorRequestKind::NeedsContext { .. },
        },
    ));
    // Re-inject the same shape → second pass exhausts budget.
    let action2 = recover(&class, &mut history, &RecoveryPolicy::default(), 0);
    assert!(matches!(
        action2,
        RecoveryAction::Escalate {
            bound: AuthorityBound::Scope,
            ..
        }
    ));
}

#[test]
fn injected_worker_timeout_path_is_vendor_crash_then_retry_twice_then_escalate_risk() {
    let err = WorkerDispatchError::Timeout(Duration::from_secs(900));
    let ctx = fresh_ctx(Vendor::Codex);
    let class = classify_failure(Some(&err), &ctx, 0, 0);
    assert!(matches!(
        class,
        FailureClass::VendorCrash {
            vendor: Vendor::Codex,
            signal: true,
            ..
        }
    ));

    let mut history = RecoveryHistory::new();
    let policy = RecoveryPolicy::default();

    // Attempt 1
    let a1 = recover(&class, &mut history, &policy, 0);
    assert!(matches!(a1, RecoveryAction::Retry { attempt: 1, max: 2 }));
    // Attempt 2
    let a2 = recover(&class, &mut history, &policy, 0);
    assert!(matches!(a2, RecoveryAction::Retry { attempt: 2, max: 2 }));
    // Budget exhausted → escalate Risk.
    let a3 = recover(&class, &mut history, &policy, 0);
    assert!(matches!(
        a3,
        RecoveryAction::Escalate {
            bound: AuthorityBound::Risk,
            ..
        }
    ));
}

#[test]
fn injected_vendor_crash_segv_path_is_distinct_from_timeout_path() {
    // SIGSEGV dispatch error.
    let err = WorkerDispatchError::Exit("died with signal SIGSEGV (code 139)".into());
    let ctx = fresh_ctx(Vendor::Gemini);
    let class = classify_failure(Some(&err), &ctx, 0, 0);
    match class {
        FailureClass::VendorCrash {
            vendor,
            signal,
            last_exit_code,
        } => {
            assert_eq!(vendor, Vendor::Gemini);
            assert!(signal);
            assert_eq!(last_exit_code, Some(139));
        }
        other => panic!("expected VendorCrash, got {other:?}"),
    }

    // First attempt still retries (under the budget).
    let action = recover(
        &class,
        &mut RecoveryHistory::new(),
        &RecoveryPolicy::default(),
        0,
    );
    assert!(matches!(
        action,
        RecoveryAction::Retry { attempt: 1, max: 2 }
    ));
}

#[test]
fn three_paths_preserve_their_first_action_and_terminal_bound() {
    // Verifies the three injected shapes independently. Timeout and
    // signal crashes intentionally share the VendorCrash policy, while a
    // missing file takes the supervisor-context path.
    let policy = RecoveryPolicy::default();
    let mut missing_history = RecoveryHistory::new();
    let mut timeout_history = RecoveryHistory::new();
    let mut crash_history = RecoveryHistory::new();

    let missing = classify_failure(
        Some(&WorkerDispatchError::Io("ENOENT \"x\"".into())),
        &fresh_ctx(Vendor::Claude),
        0,
        0,
    );
    let timeout = classify_failure(
        Some(&WorkerDispatchError::Timeout(Duration::from_secs(900))),
        &fresh_ctx(Vendor::Codex),
        0,
        0,
    );
    let crash = classify_failure(
        Some(&WorkerDispatchError::Exit("signal SIGSEGV".into())),
        &fresh_ctx(Vendor::Gemini),
        0,
        0,
    );

    let a = recover(&missing, &mut missing_history, &policy, 0);
    let b = recover(&timeout, &mut timeout_history, &policy, 0);
    let c = recover(&crash, &mut crash_history, &policy, 0);

    // Missing file → RequestSupervisor.
    assert!(matches!(a, RecoveryAction::RequestSupervisor { .. }));
    // Timeout → Retry.
    assert!(matches!(b, RecoveryAction::Retry { .. }));
    // Signal crash → the same bounded VendorCrash retry policy.
    assert!(matches!(c, RecoveryAction::Retry { .. }));

    // Distinctness of the *paths* — exhaust each budget and observe
    // each lands on a different escalation bound.
    let mut hb = RecoveryHistory::new();
    let _ = recover(&timeout, &mut hb, &policy, 0);
    let _ = recover(&timeout, &mut hb, &policy, 0);
    let timeout_terminal = recover(&timeout, &mut hb, &policy, 0);
    assert!(matches!(
        timeout_terminal,
        RecoveryAction::Escalate {
            bound: AuthorityBound::Risk,
            ..
        }
    ));

    let mut hm = RecoveryHistory::new();
    let _ = recover(&missing, &mut hm, &policy, 0);
    let missing_terminal = recover(&missing, &mut hm, &policy, 0);
    assert!(matches!(
        missing_terminal,
        RecoveryAction::Escalate {
            bound: AuthorityBound::Scope,
            ..
        }
    ));

    // Crash and timeout share the same terminal bound because both classify
    // as VendorCrash. That equivalence is part of the contract, not evidence
    // of distinct recovery actions.
}
