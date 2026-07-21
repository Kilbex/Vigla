//! Launch receipt: exhaust every terminal recovery-policy branch with
//! deterministic seeded failures and compare the result with the committed
//! machine-readable evidence.

use std::collections::BTreeMap;
use std::path::PathBuf;
use std::time::Duration;

use event_schema::Vendor;
use orchestrator::arbiter::AuthorityBound;
use orchestrator::mission_worker_dispatch::WorkerDispatchError;
use orchestrator::recovery::history::wire_name_for_class;
use orchestrator::recovery::{
    classify_failure, recover, ClassifyContext, FailureClass, RecoveryAction, RecoveryHistory,
    RecoveryPolicy,
};
use serde_json::{json, Value};

const EXPECTED_CASES: usize = 27;

struct SeedCase {
    id: String,
    category: &'static str,
    class: FailureClass,
    terminal_bound: AuthorityBound,
    max_decisions: u8,
}

fn context(vendor: Vendor) -> ClassifyContext {
    ClassifyContext {
        vendor,
        touched_files: Vec::new(),
        declared_scope: Vec::new(),
        quota_signals: Vec::new(),
        context_requests: Vec::new(),
    }
}

fn classified_case(
    id: impl Into<String>,
    category: &'static str,
    error: Option<WorkerDispatchError>,
    vendor: Vendor,
    terminal_bound: AuthorityBound,
    max_decisions: u8,
) -> SeedCase {
    let ctx = context(vendor);
    let class = classify_failure(error.as_ref(), &ctx, 0, 0);
    assert_eq!(wire_name_for_class(&class), category);
    SeedCase {
        id: id.into(),
        category,
        class,
        terminal_bound,
        max_decisions,
    }
}

fn seed_cases() -> Vec<SeedCase> {
    let mut cases = Vec::new();

    for (index, path) in [
        "src/missing.rs",
        "tests/fixtures/absent.json",
        "docs/missing.md",
        "nested path/absent.txt",
    ]
    .into_iter()
    .enumerate()
    {
        cases.push(classified_case(
            format!("missing-file-{index}"),
            "missing_file",
            Some(WorkerDispatchError::Io(format!("ENOENT \"{path}\""))),
            Vendor::Claude,
            AuthorityBound::Scope,
            2,
        ));
    }

    for (index, error) in [
        WorkerDispatchError::Exit("exited with code 75".into()),
        WorkerDispatchError::Exit("exited with code 124".into()),
        WorkerDispatchError::Io("connection reset by peer".into()),
        WorkerDispatchError::Io("network is unreachable".into()),
    ]
    .into_iter()
    .enumerate()
    {
        cases.push(classified_case(
            format!("transient-command-{index}"),
            "command_error",
            Some(error),
            Vendor::Codex,
            AuthorityBound::Quality,
            2,
        ));
    }

    for (index, error) in [
        Some(WorkerDispatchError::Exit("exited with code 1".into())),
        Some(WorkerDispatchError::Exit("exited with code 2".into())),
        Some(WorkerDispatchError::Exit("exited with code 127".into())),
        None,
    ]
    .into_iter()
    .enumerate()
    {
        cases.push(classified_case(
            format!("persistent-command-{index}"),
            "command_error",
            error,
            Vendor::Antigravity,
            AuthorityBound::Quality,
            1,
        ));
    }

    for (index, message) in [
        "CONFLICT (content): Merge conflict in src/lib.rs",
        "Merge conflict while integrating worker-2",
        "CONFLICT (rename/delete): docs/guide.md",
    ]
    .into_iter()
    .enumerate()
    {
        cases.push(classified_case(
            format!("merge-conflict-{index}"),
            "merge_conflict",
            Some(WorkerDispatchError::Git(message.into())),
            Vendor::Claude,
            AuthorityBound::Quality,
            1,
        ));
    }

    for (index, error) in [
        WorkerDispatchError::Io("EACCES \"secrets.env\"".into()),
        WorkerDispatchError::Spawn("Permission denied \"/usr/local/bin/codex\"".into()),
        WorkerDispatchError::Git("Permission denied \".git/index.lock\"".into()),
    ]
    .into_iter()
    .enumerate()
    {
        cases.push(classified_case(
            format!("permissions-{index}"),
            "permissions",
            Some(error),
            Vendor::Codex,
            AuthorityBound::Risk,
            1,
        ));
    }

    for index in 0..3 {
        cases.push(SeedCase {
            id: format!("task-drift-{index}"),
            category: "task_drift",
            class: FailureClass::TaskDrift {
                observed_files: vec![format!("outside-{index}/change.rs")],
                declared_scope: vec![PathBuf::from(format!("scope-{index}"))],
            },
            terminal_bound: AuthorityBound::Scope,
            max_decisions: 2,
        });
    }

    for vendor in [
        Vendor::Claude,
        Vendor::Codex,
        Vendor::Gemini,
        Vendor::Antigravity,
        Vendor::Kiro,
        Vendor::Copilot,
    ] {
        cases.push(classified_case(
            format!("vendor-crash-{vendor:?}").to_ascii_lowercase(),
            "vendor_crash",
            Some(WorkerDispatchError::Timeout(Duration::from_secs(900))),
            vendor,
            AuthorityBound::Risk,
            3,
        ));
    }

    cases
}

fn bound_name(bound: AuthorityBound) -> &'static str {
    match bound {
        AuthorityBound::Scope => "scope",
        AuthorityBound::Reversibility => "reversibility",
        AuthorityBound::Risk => "risk",
        AuthorityBound::Quality => "quality",
    }
}

#[test]
fn launch_recovery_receipt_matches_committed_evidence() {
    let cases = seed_cases();
    assert_eq!(cases.len(), EXPECTED_CASES, "receipt case count changed");

    let policy = RecoveryPolicy::default();
    let mut passed = 0usize;
    let mut category_counts = BTreeMap::<&str, (usize, u8, AuthorityBound)>::new();

    for case in &cases {
        let mut history = RecoveryHistory::new();
        let mut terminal = None;

        for decision in 1..=case.max_decisions {
            match recover(&case.class, &mut history, &policy, 0) {
                RecoveryAction::Retry { attempt, max } => {
                    assert!(attempt <= max, "{} exceeded its retry budget", case.id);
                    assert!(
                        decision < case.max_decisions,
                        "{} did not terminate by decision {}",
                        case.id,
                        case.max_decisions
                    );
                }
                RecoveryAction::RequestSupervisor { .. } => {
                    assert!(
                        decision < case.max_decisions,
                        "{} remained non-terminal after its bound",
                        case.id
                    );
                }
                RecoveryAction::Escalate { bound, .. } => {
                    assert_eq!(bound, case.terminal_bound, "{} wrong bound", case.id);
                    assert_eq!(
                        decision, case.max_decisions,
                        "{} terminated at an unexpected decision",
                        case.id
                    );
                    terminal = Some(bound);
                    break;
                }
                RecoveryAction::Pause { .. } => {
                    panic!("{} unexpectedly entered the quota pause path", case.id)
                }
            }
        }

        assert_eq!(
            terminal,
            Some(case.terminal_bound),
            "{} never reached an authority-bound escalation",
            case.id
        );
        assert_eq!(history.total(), case.max_decisions);
        passed += 1;

        let entry = category_counts.entry(case.category).or_insert((
            0,
            case.max_decisions,
            case.terminal_bound,
        ));
        entry.0 += 1;
        entry.1 = entry.1.max(case.max_decisions);
        assert_eq!(entry.2, case.terminal_bound);
    }

    let category_order = [
        "missing_file",
        "command_error",
        "merge_conflict",
        "permissions",
        "task_drift",
        "vendor_crash",
    ];
    let categories = category_order
        .into_iter()
        .map(|name| {
            let (seeds, max_decisions, bound) = category_counts[name];
            json!({
                "failure_class": name,
                "seeds": seeds,
                "max_decisions": max_decisions,
                "terminal_bound": bound_name(bound),
            })
        })
        .collect::<Vec<_>>();

    let receipt = json!({
        "schema_version": 1,
        "claim": "27/27 seeded failure trajectories escalated within the default retry bounds",
        "cases_total": cases.len(),
        "cases_passed": passed,
        "maximum_decisions_before_escalation": 3,
        "default_retry_policy": {
            "missing_file_retries": policy.missing_file_retries,
            "transient_command_retries": policy.transient_command_retries,
            "persistent_command_retries": policy.persistent_command_retries,
            "merge_conflict_retries": policy.merge_conflict_retries,
            "permissions_retries": policy.permissions_retries,
            "task_drift_retries": policy.task_drift_retries,
            "vendor_crash_retries": policy.vendor_crash_retries,
        },
        "categories": categories,
        "excluded_from_denominator": [
            "inadequate_context is an informational supervisor request, not a terminal failure",
            "quota_exhaustion is a planned pause with a reset time, not a terminal failure",
        ],
    });

    let committed: Value =
        serde_json::from_str(include_str!("../../../docs/evidence/recovery-receipt.json"))
            .expect("committed recovery receipt must be valid JSON");
    assert_eq!(receipt, committed, "committed receipt is stale");

    println!(
        "VIGLA_RECOVERY_RECEIPT {}",
        serde_json::to_string(&receipt).expect("serialize receipt")
    );
}
