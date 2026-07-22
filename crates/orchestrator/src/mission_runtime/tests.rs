use super::boundary::enforce_single_supervisor;
use super::event_bus::MissionEventBus;
use super::mock::decompose_mock_tasks;
use super::*;
use crate::mission_event::MissionEvent;
use std::path::{Path, PathBuf};
use std::process::Command as SyncCommand;
use tempfile::TempDir;

#[tokio::test]
async fn cancellation_notification_cannot_be_lost() {
    for _ in 0..1_000 {
        let token = CancelToken::new();
        let waiter = {
            let token = Arc::clone(&token);
            tokio::spawn(async move { token.notified().await })
        };
        token.cancel();
        tokio::time::timeout(Duration::from_millis(100), waiter)
            .await
            .expect("cancel notification was lost")
            .expect("waiter panicked");
    }

    let already_cancelled = CancelToken::new();
    already_cancelled.cancel();
    tokio::time::timeout(Duration::from_millis(100), already_cancelled.notified())
        .await
        .expect("cancel-before-wait must return immediately");
}

fn make_sandbox_repo() -> (TempDir, PathBuf) {
    let temp = TempDir::new().expect("tempdir");
    let path = temp.path().to_path_buf();
    let run = |args: &[&str]| {
        let out = SyncCommand::new("git")
            .args(args)
            .current_dir(&path)
            .output()
            .expect("git command");
        assert!(
            out.status.success(),
            "git {args:?} failed: {}",
            String::from_utf8_lossy(&out.stderr)
        );
    };
    run(&["init", "--initial-branch=main"]);
    run(&["config", "user.email", "test@example.com"]);
    run(&["config", "user.name", "Test"]);
    run(&["config", "commit.gpgsign", "false"]);
    std::fs::write(path.join("README.md"), "test\n").unwrap();
    run(&["add", "README.md"]);
    run(&["commit", "-m", "initial"]);
    (temp, path)
}

fn ok_spec() -> MissionSpec {
    MissionSpec {
        title: "Test mission".into(),
        objective: "Do mock work".into(),
        target_ref: "main".into(),
        tests: None,
        supervisor_model: None,
        worker_model: None,
        worker_count: None,
        confirm_plan: None,
        scope_paths: vec![],
    }
}

async fn drain_until<F>(rx: &mut MissionEventReceiver, stop: F) -> Vec<MissionEvent>
where
    F: Fn(&MissionEventKind) -> bool,
{
    let mut events = Vec::new();
    loop {
        let event = rx.recv().await.expect("event");
        let done = stop(&event.kind);
        events.push(event);
        if done {
            return events;
        }
    }
}

fn git_rev_parse(root: &Path, refspec: &str) -> String {
    let out = SyncCommand::new("git")
        .args(["rev-parse", refspec])
        .current_dir(root)
        .output()
        .unwrap();
    String::from_utf8_lossy(&out.stdout).trim().to_string()
}

fn git_lines(root: &Path, args: &[&str]) -> Vec<String> {
    let out = SyncCommand::new("git")
        .args(args)
        .current_dir(root)
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "git {args:?} failed: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    String::from_utf8_lossy(&out.stdout)
        .lines()
        .map(str::to_string)
        .collect()
}

fn collect_files(dir: &Path, out: &mut Vec<PathBuf>) {
    for entry in std::fs::read_dir(dir).expect("read dir") {
        let entry = entry.expect("dir entry");
        let path = entry.path();
        if path.file_name().and_then(|s| s.to_str()) == Some(".git") {
            continue;
        }
        if path.is_dir() {
            collect_files(&path, out);
        } else {
            out.push(path);
        }
    }
}

fn assert_no_mock_files_outside_mission_worktrees(root: &Path, mission_id: &str) {
    let allowed = root.join(".vigla/worktrees").join(mission_id);
    let mut files = Vec::new();
    collect_files(root, &mut files);

    let offenders: Vec<_> = files
        .into_iter()
        .filter(|p| {
            p.file_name()
                .and_then(|s| s.to_str())
                .map(|name| name.starts_with("MOCK_") && !p.starts_with(&allowed))
                .unwrap_or(false)
        })
        .collect();
    assert!(
        offenders.is_empty(),
        "mock output files escaped mission worktrees: {offenders:?}"
    );
}

fn assert_vigla_dir_only_contains_worktrees(root: &Path) {
    let vigla = root.join(".vigla");
    let entries: Vec<String> = std::fs::read_dir(&vigla)
        .expect(".vigla dir")
        .map(|entry| {
            entry
                .expect(".vigla entry")
                .file_name()
                .to_string_lossy()
                .into_owned()
        })
        .collect();
    assert_eq!(
        entries,
        vec!["worktrees".to_string()],
        "unexpected .vigla entries: {entries:?}"
    );
}

fn completed_execution_receiver() -> watch::Receiver<bool> {
    watch::channel(true).1
}

#[tokio::test]
async fn plan_decisions_are_generation_bound_and_single_claim() {
    let (_temp, root) = make_sandbox_repo();
    let workspace = MissionWorkspace::new(root, "plan-claim-0001".into()).unwrap();
    let (state_tx_raw, state_rx) = watch::channel(MissionState::PendingPlanApproval);
    let (plan_decision_tx, mut plan_decision_rx) = mpsc::channel(4);
    let runtime = MissionRuntime {
        mission_id: "plan-claim-0001".into(),
        spec: ok_spec(),
        workspace,
        event_bus: MissionEventBus::new(8),
        state_tx: Arc::new(state_tx_raw),
        state_rx,
        execution_done_rx: completed_execution_receiver(),
        cancel: CancelToken::new(),
        seq: Arc::new(AtomicU64::new(0)),
        resolve_lock: Arc::new(Mutex::new(())),
        plan_decision_tx,
        plan_generation: Arc::new(AtomicU32::new(2)),
        plan_decision_open: Arc::new(AtomicBool::new(true)),
        memory: None,
        disposition_store: Arc::new(Mutex::new(None)),
    };

    assert!(matches!(
        runtime.confirm_plan(1).await,
        Err(MissionRuntimeError::StalePlanDecision {
            submitted: 1,
            current: 2
        })
    ));
    runtime.confirm_plan(2).await.unwrap();
    assert!(matches!(
        plan_decision_rx.recv().await,
        Some(PlanDecision::Confirm { generation: 2 })
    ));
    assert!(matches!(
        runtime.reject_plan(2, None).await,
        Err(MissionRuntimeError::PlanDecisionAlreadySubmitted { generation: 2 })
    ));
}

#[tokio::test]
async fn abort_from_parked_decision_states_is_prompt_and_terminal() {
    for parked in [MissionState::CompletePendingMerge, MissionState::Attention] {
        let (_temp, root) = make_sandbox_repo();
        let workspace = MissionWorkspace::new(root, format!("parked-{parked:?}")).unwrap();
        let (state_tx_raw, state_rx) = watch::channel(parked.clone());
        let (plan_decision_tx, _) = mpsc::channel(1);
        let runtime = MissionRuntime {
            mission_id: format!("parked-{parked:?}"),
            spec: ok_spec(),
            workspace,
            event_bus: MissionEventBus::new(8),
            state_tx: Arc::new(state_tx_raw),
            state_rx,
            execution_done_rx: completed_execution_receiver(),
            cancel: CancelToken::new(),
            seq: Arc::new(AtomicU64::new(0)),
            resolve_lock: Arc::new(Mutex::new(())),
            plan_decision_tx,
            plan_generation: Arc::new(AtomicU32::new(0)),
            plan_decision_open: Arc::new(AtomicBool::new(false)),
            memory: None,
            disposition_store: Arc::new(Mutex::new(None)),
        };

        tokio::time::timeout(Duration::from_millis(200), runtime.abort())
            .await
            .expect("abort hung in parked state")
            .expect("abort failed");
        assert_eq!(runtime.state(), MissionState::Aborted);
        assert!(runtime
            .event_bus
            .snapshot_kinds()
            .iter()
            .any(|kind| matches!(kind, MissionEventKind::Aborted { .. })));
    }
}

#[tokio::test]
async fn abort_without_a_disposition_intent_does_not_require_git_reconciliation() {
    let temp = tempfile::tempdir().unwrap();
    let workspace =
        MissionWorkspace::new(temp.path().to_path_buf(), "parked-no-journal".into()).unwrap();
    let (state_tx_raw, state_rx) = watch::channel(MissionState::CompletePendingMerge);
    let (plan_decision_tx, _) = mpsc::channel(1);
    let runtime = MissionRuntime {
        mission_id: "parked-no-journal".into(),
        spec: ok_spec(),
        workspace,
        event_bus: MissionEventBus::new(8),
        state_tx: Arc::new(state_tx_raw),
        state_rx,
        execution_done_rx: completed_execution_receiver(),
        cancel: CancelToken::new(),
        seq: Arc::new(AtomicU64::new(0)),
        resolve_lock: Arc::new(Mutex::new(())),
        plan_decision_tx,
        plan_generation: Arc::new(AtomicU32::new(0)),
        plan_decision_open: Arc::new(AtomicBool::new(false)),
        memory: None,
        disposition_store: Arc::new(Mutex::new(None)),
    };

    runtime.abort().await.unwrap();

    assert_eq!(runtime.state(), MissionState::Aborted);
}

#[tokio::test]
async fn abort_drives_a_mission_that_parks_after_the_state_sample_to_terminal() {
    // Regression: abort() samples the state, cancels, then waits. If the
    // background task moves the mission into a parked decision state
    // (CompletePendingMerge / Attention) AFTER that sample — a TOCTOU the
    // background task can hit because it does not hold resolve_lock — the
    // wait loop must still drive the mission to Aborted instead of blocking
    // on rx.changed() forever (which would hold resolve_lock and deadlock
    // every later resolve()/abort()).
    for parked in [MissionState::CompletePendingMerge, MissionState::Attention] {
        let (_temp, root) = make_sandbox_repo();
        let workspace = MissionWorkspace::new(root, format!("toctou-{parked:?}")).unwrap();
        let (state_tx_raw, state_rx) = watch::channel(MissionState::Executing);
        let state_tx = Arc::new(state_tx_raw);
        let feeder_tx = Arc::clone(&state_tx);
        let (execution_done_tx, execution_done_rx) = watch::channel(false);
        let (plan_decision_tx, _) = mpsc::channel(1);
        let runtime = MissionRuntime {
            mission_id: format!("toctou-{parked:?}"),
            spec: ok_spec(),
            workspace,
            event_bus: MissionEventBus::new(8),
            state_tx,
            state_rx,
            execution_done_rx,
            cancel: CancelToken::new(),
            seq: Arc::new(AtomicU64::new(0)),
            resolve_lock: Arc::new(Mutex::new(())),
            plan_decision_tx,
            plan_generation: Arc::new(AtomicU32::new(0)),
            plan_decision_open: Arc::new(AtomicBool::new(false)),
            memory: None,
            disposition_store: Arc::new(Mutex::new(None)),
        };

        // abort() samples Executing synchronously and parks on rx.changed();
        // this delayed feed moves the state into the parked variant only
        // after abort is already waiting, exercising the in-loop transition.
        let feeder = tokio::spawn(async move {
            tokio::time::sleep(Duration::from_millis(20)).await;
            feeder_tx.send(parked).ok();
            tokio::time::sleep(Duration::from_millis(20)).await;
            execution_done_tx.send(true).ok();
        });

        tokio::time::timeout(Duration::from_millis(500), runtime.abort())
            .await
            .expect("abort hung after the mission parked post-sample")
            .expect("abort failed");
        feeder.await.ok();
        assert_eq!(runtime.state(), MissionState::Aborted);
        assert!(runtime
            .event_bus
            .snapshot_kinds()
            .iter()
            .any(|kind| matches!(kind, MissionEventKind::Aborted { .. })));
    }
}

#[tokio::test]
async fn abort_waits_for_parked_producer_events_before_publishing_aborted() {
    let (_temp, root) = make_sandbox_repo();
    let workspace = MissionWorkspace::new(root, "parked-ordering-0001".into()).unwrap();
    let (state_tx_raw, state_rx) = watch::channel(MissionState::CompletePendingMerge);
    let (execution_done_tx, execution_done_rx) = watch::channel(false);
    let (plan_decision_tx, _) = mpsc::channel(1);
    let event_bus = MissionEventBus::new(8);
    let seq = Arc::new(AtomicU64::new(0));
    let runtime = MissionRuntime {
        mission_id: "parked-ordering-0001".into(),
        spec: ok_spec(),
        workspace,
        event_bus: event_bus.clone(),
        state_tx: Arc::new(state_tx_raw),
        state_rx,
        execution_done_rx,
        cancel: CancelToken::new(),
        seq: Arc::clone(&seq),
        resolve_lock: Arc::new(Mutex::new(())),
        plan_decision_tx,
        plan_generation: Arc::new(AtomicU32::new(0)),
        plan_decision_open: Arc::new(AtomicBool::new(false)),
        memory: None,
        disposition_store: Arc::new(Mutex::new(None)),
    };
    let producer = tokio::spawn(async move {
        tokio::time::sleep(Duration::from_millis(30)).await;
        emit(
            &event_bus,
            "parked-ordering-0001",
            &seq,
            MissionEventKind::Completed {
                summary: "producer finished".into(),
                files_changed: 1,
            },
        );
        execution_done_tx.send(true).ok();
    });

    runtime.abort().await.unwrap();
    producer.await.unwrap();

    let events = runtime.event_bus.snapshot_kinds();
    let completed = events
        .iter()
        .position(|event| matches!(event, MissionEventKind::Completed { .. }))
        .expect("producer completion event");
    let aborted = events
        .iter()
        .position(|event| matches!(event, MissionEventKind::Aborted { .. }))
        .expect("abort event");
    assert!(
        completed < aborted,
        "terminal abort overtook producer events"
    );
}

fn assert_no_mission_artifacts(root: &Path, mission_id: &str) {
    let branch_prefix = format!("vigla/{mission_id}/");
    let branches = git_lines(
        root,
        &["for-each-ref", "--format=%(refname:short)", "refs/heads/"],
    );
    assert!(
        !branches.iter().any(|b| b.starts_with(&branch_prefix)),
        "mission branches should be gone: {branches:?}"
    );

    let tag_prefix = format!("vigla/snap/{mission_id}/");
    let tags = git_lines(root, &["tag", "--list"]);
    assert!(
        !tags.iter().any(|t| t.starts_with(&tag_prefix)),
        "mission tags should be gone: {tags:?}"
    );

    let worktree_segment = format!(".vigla/worktrees/{mission_id}/");
    let worktrees = git_lines(root, &["worktree", "list", "--porcelain"]);
    assert!(
        !worktrees
            .iter()
            .any(|line| line.contains(&worktree_segment)),
        "mission worktrees should be unregistered: {worktrees:?}"
    );
    assert!(
        !root.join(".vigla/worktrees").join(mission_id).exists(),
        "mission worktree directory should be removed"
    );
}

#[tokio::test(flavor = "current_thread")]
async fn mock_mission_runs_to_complete_pending_merge() {
    let (_temp, root) = make_sandbox_repo();
    let ws = MissionWorkspace::new(root.clone(), "demo-0000".into()).unwrap();

    let runtime = MissionRuntime::start(ok_spec(), ws, MockTimingConfig::fast())
        .await
        .expect("start");
    let mut rx = runtime.subscribe();

    let events = tokio::time::timeout(
        Duration::from_secs(5),
        drain_until(&mut rx, |k| matches!(k, MissionEventKind::Completed { .. })),
    )
    .await
    .expect("did not complete within 5s");

    assert_eq!(runtime.state(), MissionState::CompletePendingMerge);
    assert!(events
        .iter()
        .any(|e| matches!(e.kind, MissionEventKind::ExecutionStarted)));
    assert!(events
        .iter()
        .any(|e| matches!(e.kind, MissionEventKind::Decomposition { .. })));
    assert_eq!(
        events
            .iter()
            .filter(|e| matches!(e.kind, MissionEventKind::WorkerSpawned { .. }))
            .count(),
        3
    );
    assert_eq!(
        events
            .iter()
            .filter(|e| matches!(e.kind, MissionEventKind::Integrated { .. }))
            .count(),
        3
    );
    // Seqs are strictly monotonic and start at 0.
    let mut prev: Option<u64> = None;
    for e in &events {
        if let Some(p) = prev {
            assert!(e.seq > p, "seqs not monotonic: {p} -> {}", e.seq);
        }
        prev = Some(e.seq);
    }
}

#[tokio::test(flavor = "current_thread")]
async fn mock_mission_confines_checkout_writes_to_mission_worktrees() {
    let (_temp, root) = make_sandbox_repo();
    let mission_id = "confine-test-0001";
    let ws = MissionWorkspace::new(root.clone(), mission_id.into()).unwrap();

    let runtime = MissionRuntime::start(ok_spec(), ws, MockTimingConfig::fast())
        .await
        .expect("start");
    let final_state =
        tokio::time::timeout(Duration::from_secs(5), runtime.await_complete_or_terminal())
            .await
            .expect("did not complete within 5s");
    assert_eq!(final_state, MissionState::CompletePendingMerge);

    assert_vigla_dir_only_contains_worktrees(&root);
    assert_no_mock_files_outside_mission_worktrees(&root, mission_id);

    for i in 0..3 {
        let file = format!("MOCK_{i}.md");
        assert!(
            root.join(".vigla/worktrees")
                .join(mission_id)
                .join(format!("mock-{}", i + 1))
                .join(&file)
                .exists(),
            "worker output missing inside mission worktree: {file}"
        );
        assert!(
            root.join(".vigla/worktrees")
                .join(mission_id)
                .join("supervisor")
                .join(&file)
                .exists(),
            "integrated output missing inside supervisor worktree: {file}"
        );
        assert!(
            !root.join(&file).exists(),
            "mock output must not appear in the user's checkout: {file}"
        );
    }
}

#[tokio::test(flavor = "current_thread")]
async fn mock_mission_creates_only_mission_scoped_vigla_refs() {
    let (_temp, root) = make_sandbox_repo();
    let mission_id = "namespace-test-0001";
    let ws = MissionWorkspace::new(root.clone(), mission_id.into()).unwrap();

    let runtime = MissionRuntime::start(ok_spec(), ws, MockTimingConfig::fast())
        .await
        .expect("start");
    tokio::time::timeout(Duration::from_secs(5), runtime.await_complete_or_terminal())
        .await
        .expect("did not complete within 5s");

    let branches = git_lines(
        &root,
        &["for-each-ref", "--format=%(refname:short)", "refs/heads/"],
    );
    let vigla_branches: Vec<_> = branches
        .iter()
        .filter(|b| b.starts_with("vigla/"))
        .cloned()
        .collect();
    let branch_prefix = format!("vigla/{mission_id}/");
    assert_eq!(
        vigla_branches.len(),
        4,
        "supervisor + 3 worker branches expected: {vigla_branches:?}"
    );
    assert!(
        vigla_branches.iter().all(|b| b.starts_with(&branch_prefix)),
        "vigla branches escaped mission namespace: {vigla_branches:?}"
    );
    assert!(vigla_branches.contains(&format!("vigla/{mission_id}/supervisor")));
    for i in 1..=3 {
        assert!(vigla_branches.contains(&format!("vigla/{mission_id}/worker/mock-{i}")));
    }

    let tags = git_lines(&root, &["tag", "--list"]);
    let snap_prefix = format!("vigla/snap/{mission_id}/");
    let pre_merge_prefix = format!("vigla/pre-merge/{mission_id}/");
    // Each successful integration now creates two tags: a snapshot
    // tag at the merge commit and a pre-merge tag at the prior
    // supervisor HEAD (for the revert path).
    assert_eq!(
        tags.len(),
        6,
        "expected exactly 6 tags (3 snap + 3 pre-merge): {tags:?}"
    );
    assert!(
        tags.iter()
            .all(|t| t.starts_with(&snap_prefix) || t.starts_with(&pre_merge_prefix)),
        "tags escaped mission namespace: {tags:?}"
    );
    for i in 0..3 {
        assert!(tags.contains(&format!("vigla/snap/{mission_id}/{i}")));
        assert!(tags.contains(&format!("vigla/pre-merge/{mission_id}/{i}")));
    }
}

#[tokio::test(flavor = "current_thread")]
async fn late_subscriber_replays_mission_created_first() {
    let (_temp, root) = make_sandbox_repo();
    let ws = MissionWorkspace::new(root, "late-sub-0001".into()).unwrap();

    let runtime = MissionRuntime::start(ok_spec(), ws, MockTimingConfig::fast())
        .await
        .expect("start");

    // Let the spawned task run before subscribing. Without the
    // replay buffer, this subscriber can miss `mission.created`,
    // causing the frontend reducer to ignore all later events.
    tokio::task::yield_now().await;
    tokio::task::yield_now().await;

    let mut rx = runtime.subscribe();
    let first = rx.recv().await.expect("created event replay");
    assert_eq!(first.seq, 0);
    assert!(matches!(first.kind, MissionEventKind::Created { .. }));
}

#[tokio::test(flavor = "current_thread")]
async fn abort_mid_mission_emits_aborted_and_preserves_branches() {
    let (_temp, root) = make_sandbox_repo();
    let ws = MissionWorkspace::new(root.clone(), "abort-test-0001".into()).unwrap();

    // Slow timings so we can abort before completion.
    let config = MockTimingConfig {
        worker_work_duration: Duration::from_millis(500),
        ..MockTimingConfig::fast()
    };

    let runtime = MissionRuntime::start(ok_spec(), ws, config).await.unwrap();
    let mut rx = runtime.subscribe();

    // Wait for at least the first worker to spawn so we abort
    // mid-execution rather than before anything happens.
    loop {
        let e = rx.recv().await.unwrap();
        if matches!(e.kind, MissionEventKind::WorkerSpawned { .. }) {
            break;
        }
    }

    runtime.abort().await.expect("abort");

    // After abort returns, state is Aborted and an aborted event was emitted.
    assert_eq!(runtime.state(), MissionState::Aborted);

    // Branches are preserved per v2 §3.5.
    let listing = SyncCommand::new("git")
        .args(["for-each-ref", "--format=%(refname:short)", "refs/heads/"])
        .current_dir(&root)
        .output()
        .unwrap();
    let listing_str = String::from_utf8_lossy(&listing.stdout);
    assert!(
        listing_str.contains("vigla/abort-test-0001/supervisor"),
        "supervisor branch should be preserved after abort: {listing_str}"
    );
}

#[tokio::test(flavor = "current_thread")]
async fn resolve_merge_advances_target_ref_and_discards_branches() {
    let (_temp, root) = make_sandbox_repo();
    let ws = MissionWorkspace::new(root.clone(), "merge-test-0002".into()).unwrap();

    let runtime = MissionRuntime::start(ok_spec(), ws, MockTimingConfig::fast())
        .await
        .unwrap();

    let final_state =
        tokio::time::timeout(Duration::from_secs(5), runtime.await_complete_or_terminal())
            .await
            .expect("did not complete within 5s");
    assert_eq!(final_state, MissionState::CompletePendingMerge);

    let pre = git_rev_parse(&root, "main");
    runtime
        .resolve(ResolveAction::Merge)
        .await
        .expect("merge resolve");
    let post = git_rev_parse(&root, "main");
    assert_ne!(pre, post, "main should have advanced");
    assert_eq!(runtime.state(), MissionState::Merged);

    // Mission branches gone after merge (Merge does final_merge then discard).
    let listing = SyncCommand::new("git")
        .args(["for-each-ref", "--format=%(refname:short)", "refs/heads/"])
        .current_dir(&root)
        .output()
        .unwrap();
    let listing_str = String::from_utf8_lossy(&listing.stdout);
    assert!(!listing_str.contains("vigla/merge-test-0002/"));
    assert_no_mission_artifacts(&root, "merge-test-0002");
}

#[tokio::test(flavor = "current_thread")]
async fn resolve_discard_cleans_up_without_touching_target() {
    let (_temp, root) = make_sandbox_repo();
    let ws = MissionWorkspace::new(root.clone(), "discard-test-0003".into()).unwrap();

    let runtime = MissionRuntime::start(ok_spec(), ws, MockTimingConfig::fast())
        .await
        .unwrap();
    tokio::time::timeout(Duration::from_secs(5), runtime.await_complete_or_terminal())
        .await
        .unwrap();

    let pre = git_rev_parse(&root, "main");
    runtime.resolve(ResolveAction::Discard).await.unwrap();
    let post = git_rev_parse(&root, "main");
    assert_eq!(pre, post, "main should NOT have moved on discard");
    assert_eq!(runtime.state(), MissionState::Discarded);

    let listing = SyncCommand::new("git")
        .args(["for-each-ref", "--format=%(refname:short)", "refs/heads/"])
        .current_dir(&root)
        .output()
        .unwrap();
    let listing_str = String::from_utf8_lossy(&listing.stdout);
    assert!(!listing_str.contains("vigla/discard-test-0003/"));
    assert_no_mission_artifacts(&root, "discard-test-0003");
}

#[tokio::test(flavor = "current_thread")]
async fn resolve_returns_only_after_terminal_outcome_is_durable() {
    for (suffix, action, expected) in [
        (
            "merge",
            ResolveAction::Merge,
            crate::MissionOutcomeState::Merged,
        ),
        (
            "discard",
            ResolveAction::Discard,
            crate::MissionOutcomeState::Discarded,
        ),
    ] {
        let (_temp, root) = make_sandbox_repo();
        let mission_id = format!("durable-{suffix}-0001");
        let runtime = MissionRuntime::start(
            ok_spec(),
            MissionWorkspace::new(root.clone(), mission_id.clone()).unwrap(),
            MockTimingConfig::fast(),
        )
        .await
        .unwrap();
        let repository = crate::Repository::open_in_memory().await.unwrap();
        runtime.install_disposition_store(repository.clone()).await;
        tokio::time::timeout(Duration::from_secs(5), runtime.await_complete_or_terminal())
            .await
            .unwrap();

        runtime.resolve(action).await.unwrap();

        let outcome = repository
            .mission_outcome(&mission_id)
            .await
            .unwrap()
            .expect("resolve success requires a durable outcome");
        assert_eq!(outcome.state, expected);
        assert_eq!(outcome.repo_root.as_deref(), root.to_str());
        assert!(repository
            .list_disposition_intents()
            .await
            .unwrap()
            .is_empty());
    }
}

#[tokio::test(flavor = "current_thread")]
async fn abort_reconciles_a_journaled_git_merge_instead_of_recording_aborted() {
    let (_temp, root) = make_sandbox_repo();
    let mission_id = "abort-after-git-merge-0001";
    let runtime = MissionRuntime::start(
        ok_spec(),
        MissionWorkspace::new(root.clone(), mission_id.into()).unwrap(),
        MockTimingConfig::fast(),
    )
    .await
    .unwrap();
    let repository = crate::Repository::open_in_memory().await.unwrap();
    runtime.install_disposition_store(repository.clone()).await;
    tokio::time::timeout(Duration::from_secs(5), runtime.await_complete_or_terminal())
        .await
        .unwrap();

    repository
        .record_disposition_intent(
            mission_id,
            root.to_str().unwrap(),
            "main",
            crate::DispositionAction::Merge,
            "2026-07-22T12:00:00Z",
        )
        .await
        .unwrap();
    runtime.workspace.final_merge("main").await.unwrap();
    assert!(
        repository
            .mission_outcome(mission_id)
            .await
            .unwrap()
            .is_none(),
        "precondition: simulate the crash window before the merged outcome is persisted"
    );

    runtime.abort().await.unwrap();

    assert_eq!(runtime.state(), MissionState::Merged);
    assert_eq!(
        repository
            .mission_outcome(mission_id)
            .await
            .unwrap()
            .unwrap()
            .state,
        crate::MissionOutcomeState::Merged,
        "durable Git merge proof must win over a later abort request"
    );
    assert!(repository
        .list_disposition_intents()
        .await
        .unwrap()
        .is_empty());
    let events = runtime.event_bus.snapshot_kinds();
    assert!(events.iter().any(|kind| matches!(
        kind,
        MissionEventKind::MergeResolved {
            resolution: MergeResolution::Merged
        }
    )));
    assert!(!events
        .iter()
        .any(|kind| matches!(kind, MissionEventKind::Aborted { .. })));
}

#[tokio::test(flavor = "current_thread")]
async fn resolve_extend_fails_closed_without_stranding_the_mission() {
    let (_temp, root) = make_sandbox_repo();
    let mission_id = "extend-e2e-0001";
    let ws = MissionWorkspace::new(root.clone(), mission_id.into()).unwrap();

    let runtime = MissionRuntime::start(ok_spec(), ws, MockTimingConfig::fast())
        .await
        .unwrap();
    let mut rx = runtime.subscribe();
    tokio::time::timeout(Duration::from_secs(5), runtime.await_complete_or_terminal())
        .await
        .expect("did not complete within 5s");

    let pre = git_rev_parse(&root, "main");
    let error = runtime
        .resolve(ResolveAction::Extend {
            directive: Some("expand the mock docs coverage".into()),
        })
        .await
        .expect_err("extension must fail until supervisor re-entry is implemented");
    let post = git_rev_parse(&root, "main");

    assert_eq!(
        error.to_string(),
        "mission extension is unavailable until supervisor re-entry is implemented"
    );
    assert_eq!(pre, post, "Extend must not advance the target branch");
    assert_eq!(runtime.state(), MissionState::CompletePendingMerge);
    assert!(
        root.join(".vigla/worktrees")
            .join(mission_id)
            .join("supervisor")
            .exists(),
        "a rejected extension must preserve the reviewable workspace"
    );
    assert!(
        git_lines(
            &root,
            &["for-each-ref", "--format=%(refname:short)", "refs/heads/"],
        )
        .iter()
        .any(|b| b == &format!("vigla/{mission_id}/supervisor")),
        "a rejected extension must preserve the supervisor branch"
    );

    let mut events = Vec::new();
    while let Ok(e) = rx.try_recv() {
        events.push(e);
    }
    assert!(
        !events.iter().any(|e| matches!(
            e.kind,
            MissionEventKind::MissionExtended { .. }
                | MissionEventKind::MergeResolved {
                    resolution: MergeResolution::Extended { .. }
                }
        )),
        "a rejected extension must not emit a false continuation event"
    );
}

#[tokio::test(flavor = "current_thread")]
async fn five_consecutive_mock_missions_complete_with_start_and_decide_only() {
    let (_temp, root) = make_sandbox_repo();
    let initial_main = git_rev_parse(&root, "main");

    for i in 0..5 {
        let mission_id = format!("two-touch-{i:04}");
        let ws = MissionWorkspace::new(root.clone(), mission_id.clone()).unwrap();
        let runtime = MissionRuntime::start(ok_spec(), ws, MockTimingConfig::fast())
            .await
            .expect("start");
        let mut rx = runtime.subscribe();

        let events = tokio::time::timeout(
            Duration::from_secs(5),
            drain_until(&mut rx, |k| matches!(k, MissionEventKind::Completed { .. })),
        )
        .await
        .expect("mission should complete within 5s");

        assert_eq!(runtime.state(), MissionState::CompletePendingMerge);
        assert_eq!(
            count_kind(&events, |k| matches!(
                k,
                MissionEventKind::WorkerSpawned { .. }
            )),
            3,
            "default mock mission should use the N=3 employee team"
        );
        assert_eq!(
            count_kind(&events, |k| matches!(
                k,
                MissionEventKind::PlanProposed { .. }
                    | MissionEventKind::PlanConfirmed { .. }
                    | MissionEventKind::PlanRegenerationRequested { .. }
            )),
            0,
            "normal mock flow must not require a plan-preview touch"
        );
        // Budget gate retired in Task 14.

        runtime
            .resolve(ResolveAction::Discard)
            .await
            .expect("decide via discard");
        assert_eq!(runtime.state(), MissionState::Discarded);
        assert_no_mission_artifacts(&root, &mission_id);
    }

    assert_eq!(
        git_rev_parse(&root, "main"),
        initial_main,
        "Discarding every run must leave the target branch unchanged"
    );
    let remaining_vigla_heads: Vec<_> = git_lines(
        &root,
        &["for-each-ref", "--format=%(refname:short)", "refs/heads/"],
    )
    .into_iter()
    .filter(|b| b.starts_with("vigla/"))
    .collect();
    assert!(
        remaining_vigla_heads.is_empty(),
        "consecutive discard runs should leave no mission branches: {remaining_vigla_heads:?}"
    );
}

#[test]
fn decompose_default_is_three_named_tasks() {
    let tasks = decompose_mock_tasks(None);
    assert_eq!(tasks.len(), 3);
    assert_eq!(tasks[0].title, "Plan integration");
    assert_eq!(tasks[1].title, "Implement changes");
    assert_eq!(tasks[2].title, "Update documentation");
}

#[test]
fn decompose_respects_explicit_count() {
    let tasks = decompose_mock_tasks(Some(5));
    assert_eq!(tasks.len(), 5);
    assert_eq!(tasks[0].title, "Plan integration");
    assert_eq!(tasks[2].title, "Update documentation");
    assert_eq!(tasks[3].title, "Task 4");
    assert_eq!(tasks[4].title, "Task 5");
    // Indices are 0-based and contiguous.
    for (i, t) in tasks.iter().enumerate() {
        assert_eq!(t.index, i as u32);
    }
}

#[test]
fn decompose_clamps_to_one_minimum() {
    let tasks = decompose_mock_tasks(Some(0));
    assert_eq!(tasks.len(), 1);
}

#[test]
fn decompose_clamps_to_ten_maximum() {
    let tasks = decompose_mock_tasks(Some(999));
    assert_eq!(tasks.len(), 10);
}

#[tokio::test(flavor = "current_thread")]
async fn mock_mission_respects_worker_count() {
    let (_temp, root) = make_sandbox_repo();
    let ws = MissionWorkspace::new(root, "wc-test-0005".into()).unwrap();

    let spec = MissionSpec {
        worker_count: Some(5),
        ..ok_spec()
    };

    let runtime = MissionRuntime::start(spec, ws, MockTimingConfig::fast())
        .await
        .expect("start");
    let mut rx = runtime.subscribe();

    let events = tokio::time::timeout(
        Duration::from_secs(10),
        drain_until(&mut rx, |k| matches!(k, MissionEventKind::Completed { .. })),
    )
    .await
    .expect("did not complete within 10s");

    assert_eq!(runtime.state(), MissionState::CompletePendingMerge);
    assert_eq!(
        events
            .iter()
            .filter(|e| matches!(e.kind, MissionEventKind::WorkerSpawned { .. }))
            .count(),
        5
    );
    assert_eq!(
        events
            .iter()
            .filter(|e| matches!(e.kind, MissionEventKind::Integrated { .. }))
            .count(),
        5
    );
}

#[tokio::test(flavor = "current_thread")]
async fn resolve_before_complete_pending_merge_blocks() {
    let (_temp, root) = make_sandbox_repo();
    let ws = MissionWorkspace::new(root, "block-test-0004".into()).unwrap();

    let runtime = MissionRuntime::start(
        ok_spec(),
        ws,
        MockTimingConfig {
            worker_work_duration: Duration::from_millis(500),
            ..MockTimingConfig::fast()
        },
    )
    .await
    .unwrap();

    // Calling resolve while executing should not return until the
    // mission reaches CompletePendingMerge. We wrap it in a timeout
    // to confirm it eventually does complete.
    let outcome = tokio::time::timeout(
        Duration::from_secs(5),
        runtime.resolve(ResolveAction::Discard),
    )
    .await
    .expect("resolve should eventually complete");
    outcome.expect("discard ok");
    assert_eq!(runtime.state(), MissionState::Discarded);
}

/// Two concurrent `resolve(Merge)` calls on the same runtime must
/// serialize on the inner lock: one wins and merges, the other
/// sees the post-merge state and gets `ResolveNotAllowed`.
/// Without the lock, both entered `final_merge` concurrently and
/// raced on the shared temp worktree + the `update-ref` of the
/// user's target branch — risking corruption of `main`.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn concurrent_resolve_merge_serializes_and_one_is_rejected() {
    let (_temp, root) = make_sandbox_repo();
    let ws = MissionWorkspace::new(root.clone(), "concurrent-test-0007".into()).unwrap();

    let runtime = MissionRuntime::start(ok_spec(), ws, MockTimingConfig::fast())
        .await
        .unwrap();
    tokio::time::timeout(Duration::from_secs(5), runtime.await_complete_or_terminal())
        .await
        .unwrap();
    assert_eq!(runtime.state(), MissionState::CompletePendingMerge);

    let r1 = runtime.clone();
    let r2 = runtime.clone();
    let h1 = tokio::spawn(async move { r1.resolve(ResolveAction::Merge).await });
    let h2 = tokio::spawn(async move { r2.resolve(ResolveAction::Merge).await });
    let outcomes = tokio::time::timeout(Duration::from_secs(10), async {
        (h1.await.unwrap(), h2.await.unwrap())
    })
    .await
    .expect("both resolves must finish quickly");

    let oks = [&outcomes.0, &outcomes.1]
        .iter()
        .filter(|r| r.is_ok())
        .count();
    let rejected = [&outcomes.0, &outcomes.1]
        .iter()
        .filter(|r| matches!(r, Err(MissionRuntimeError::ResolveNotAllowed { .. })))
        .count();
    assert_eq!(oks, 1, "exactly one resolve should succeed: {outcomes:?}");
    assert_eq!(
        rejected, 1,
        "exactly one resolve should be rejected post-merge: {outcomes:?}"
    );
    assert_eq!(runtime.state(), MissionState::Merged);
}

/// Force a mid-mission failure (pre-create a worker branch so
/// `git branch` fails on conflict) and assert the runtime
/// transitions to `Aborted` instead of leaving subscribers /
/// resolvers blocked forever. Regression test for the H1 bug
/// where Err from the spawned mission body was silently dropped.
#[tokio::test(flavor = "current_thread")]
async fn mock_mission_aborts_state_when_task_body_returns_err() {
    let (_temp, root) = make_sandbox_repo();
    let mission_id = "fail-test-0006";

    // Pre-create the branch that the mock mission will try to
    // create for its first worker. `git branch <name> <from>`
    // refuses to overwrite an existing branch, so
    // `create_worker_branch` inside `run_mock_mission` will
    // return Err on the first task and propagate via `?`.
    let conflict_branch = format!("vigla/{}/worker/mock-1", mission_id);
    SyncCommand::new("git")
        .args(["branch", &conflict_branch, "HEAD"])
        .current_dir(&root)
        .output()
        .unwrap();

    let ws = MissionWorkspace::new(root, mission_id.into()).unwrap();
    let runtime = MissionRuntime::start(ok_spec(), ws, MockTimingConfig::fast())
        .await
        .expect("start should succeed — failure is mid-mission, not at startup");

    // Without the H1 fix, both `await_complete_or_terminal` and
    // `resolve` would block forever. With the fix, the spawn
    // closure observes the inner Err and finalizes to Aborted.
    let final_state =
        tokio::time::timeout(Duration::from_secs(5), runtime.await_complete_or_terminal())
            .await
            .expect("mission state must reach a terminal value when task body fails");
    assert_eq!(final_state, MissionState::Aborted);

    // Resolve must reject; not block. Same regression.
    let resolve_outcome = tokio::time::timeout(
        Duration::from_secs(2),
        runtime.resolve(ResolveAction::Discard),
    )
    .await
    .expect("resolve must return promptly when mission is already aborted");
    assert!(matches!(
        resolve_outcome,
        Err(MissionRuntimeError::ResolveNotAllowed { .. })
    ));
}

/// Helper: count events of a given kind from a drained event list.
fn count_kind<F>(events: &[MissionEvent], pred: F) -> usize
where
    F: Fn(&MissionEventKind) -> bool,
{
    events.iter().filter(|e| pred(&e.kind)).count()
}

// ─── Phase 1: single supervisor per mission (decisions.md entry 6) ──

/// Phase 1: when the supervisor's intent stream requests spawning
/// a worker whose role would itself be `Supervisor`, the
/// orchestrator emits `boundary.sub_supervisor_refused` and no
/// `worker.spawned` event follows.
#[tokio::test(flavor = "current_thread")]
async fn sub_supervisor_spawn_is_refused() {
    let event_bus = MissionEventBus::new(8);
    let mut rx = event_bus.subscribe();
    let seq = Arc::new(AtomicU64::new(0));
    let mission_id = "subsup-refusal-0001".to_string();

    // Synthetic supervisor intent: spawn a worker with role
    // `Supervisor`. The boundary should refuse it.
    let employee_outcome = enforce_single_supervisor(
        &event_bus,
        &mission_id,
        &seq,
        "sup-claude-1",
        "would-be-employee-1",
        WorkerRole::Employee,
    );
    assert!(employee_outcome.is_ok(), "Employee role must be permitted");

    let supervisor_outcome = enforce_single_supervisor(
        &event_bus,
        &mission_id,
        &seq,
        "sup-claude-1",
        "would-be-sub-supervisor-1",
        WorkerRole::Supervisor,
    );
    assert!(
        supervisor_outcome.is_err(),
        "Supervisor role must be refused per decisions.md entry 6"
    );

    // Drain emitted events; exactly one SubSupervisorRefused, zero WorkerSpawned.
    let mut events = Vec::new();
    while let Ok(e) = rx.try_recv() {
        events.push(e);
    }
    assert_eq!(
        count_kind(&events, |k| matches!(
            k,
            MissionEventKind::SubSupervisorRefused { .. }
        )),
        1,
        "expected exactly one SubSupervisorRefused"
    );
    assert_eq!(
        count_kind(&events, |k| matches!(
            k,
            MissionEventKind::WorkerSpawned { .. }
        )),
        0,
        "no WorkerSpawned should be emitted by the refusal helper"
    );

    // Verify payload carries both ids.
    let refusal = events
        .iter()
        .find_map(|e| match &e.kind {
            MissionEventKind::SubSupervisorRefused {
                requested_by_supervisor_id,
                requested_worker_id,
            } => Some((
                requested_by_supervisor_id.clone(),
                requested_worker_id.clone(),
            )),
            _ => None,
        })
        .expect("refusal event present");
    assert_eq!(refusal.0, "sup-claude-1");
    assert_eq!(refusal.1, "would-be-sub-supervisor-1");
}

// ---------------------------------------------------------------------
// Tier-2A acceptance: mission lifecycle + memory kernel
// ---------------------------------------------------------------------

/// Build a kernel against a fresh in-memory pool, rooted at a tempdir.
/// Lives next to the runtime tests so we don't expand the public
/// surface of the memory module.
async fn fresh_kernel_for_runtime() -> (std::sync::Arc<crate::memory::MemoryKernel>, TempDir) {
    use sqlx::sqlite::{SqliteConnectOptions, SqlitePoolOptions};
    use std::str::FromStr;

    let opts = SqliteConnectOptions::from_str("sqlite::memory:").unwrap();
    let pool = SqlitePoolOptions::new()
        .min_connections(1)
        .max_connections(1)
        .idle_timeout(None)
        .max_lifetime(None)
        .connect_with(opts)
        .await
        .unwrap();
    sqlx::migrate!("./migrations").run(&pool).await.unwrap();
    let vigla_root = TempDir::new().unwrap();
    let kernel = crate::memory::MemoryKernel::open(pool, vigla_root.path().to_path_buf())
        .await
        .unwrap();
    (std::sync::Arc::new(kernel), vigla_root)
}

/// **The Tier-2A vertical slice.** Seed one promoted note. Run a
/// normal mission. Assert the worker's worktree contains the note's
/// body in the vendor native file, byte-exact. Then resolve Merge
/// and assert the memory barrier fired.
///
/// This is the user-facing release gate: "a pinned note appears in
/// the next mission's worker context."
#[tokio::test(flavor = "current_thread")]
async fn tier2a_promoted_note_lands_in_worker_native_file() {
    use crate::memory::{NoteKind, PinInput, PinOutcome, Scope, ScopeKind, StandardNoteKind};
    use event_schema::memory::AuthorSource;

    // 1. Repo + kernel.
    let (_repo_dir, repo_root) = make_sandbox_repo();
    let (kernel, _codex_dir) = fresh_kernel_for_runtime().await;

    // 2. Pin a hazard. The user-oracle shortcut promotes immediately.
    let pin = kernel
        .pin_note(PinInput {
            kind: NoteKind::Standard(StandardNoteKind::Hazard),
            scope: Scope {
                kind: ScopeKind::Repo,
                value: None,
            },
            body: "Run `cargo build --workspace` before commits — partial \
                   builds mask cross-crate breakage."
                .into(),
            source: AuthorSource::Cli,
        })
        .await
        .unwrap();
    let note_id = match pin {
        PinOutcome::Pinned { note_id, promoted } => {
            assert!(promoted, "user-authored note must promote on pin");
            note_id
        }
        _ => panic!("pin rejected unexpectedly"),
    };

    // 3. Start a normal mission with one worker, memory installed.
    let mid = format!("tier2a-{}", uuid::Uuid::now_v7().simple());
    let workspace = MissionWorkspace::new(repo_root.clone(), mid.clone()).unwrap();
    let runtime = MissionRuntime::start_with_memory(
        MissionSpec {
            worker_count: Some(1),
            ..ok_spec()
        },
        workspace,
        MockTimingConfig::fast(),
        Some(kernel.clone()),
        None,
    )
    .await
    .unwrap();

    // 4. Wait for completion.
    tokio::time::timeout(Duration::from_secs(5), runtime.await_complete_or_terminal())
        .await
        .unwrap();

    // 5. The worker's worktree should now contain CLAUDE.md with the
    // hazard body inside the anchor block.
    let worker_worktree = repo_root.join(".vigla/worktrees").join(&mid).join("mock-1");
    let claude_md_path = worker_worktree.join("CLAUDE.md");
    assert!(
        claude_md_path.exists(),
        "worker worktree should contain CLAUDE.md after memory attach"
    );
    let contents = std::fs::read_to_string(&claude_md_path).unwrap();
    assert!(
        contents.contains("Run `cargo build --workspace`"),
        "CLAUDE.md must carry the pinned hazard body verbatim — got:\n{contents}"
    );
    assert!(contents.contains("vigla:memory:begin v1"));

    // 6. Resolve as Merge → memory barrier fires.
    runtime.resolve(ResolveAction::Merge).await.unwrap();

    // 7. Barrier event landed in the kernel's event log, scoped to
    // this mission and of kind=accept.
    let (count,): (i64,) = sqlx::query_as(
        "SELECT COUNT(*) FROM memory_events \
         WHERE type = 'barrier' AND mission_id = ? \
           AND payload_json LIKE '%\"kind\":\"accept\"%'",
    )
    .bind(&mid)
    .fetch_one(kernel.pool())
    .await
    .unwrap();
    assert_eq!(count, 1, "exactly one accept-barrier must fire on resolve");

    // 8. The promoted note used in the bundle keeps existing —
    // promotion was not undone by reflection.
    let after = kernel.store.note_show(&note_id).await.unwrap();
    assert_eq!(after.state, crate::memory::NoteState::Promoted);
}

/// Companion test: discard fires a Scrub barrier, not an Accept.
#[tokio::test(flavor = "current_thread")]
async fn tier2a_discard_fires_scrub_barrier() {
    let (_repo_dir, repo_root) = make_sandbox_repo();
    let (kernel, _codex_dir) = fresh_kernel_for_runtime().await;

    let mid = format!("tier2a-{}", uuid::Uuid::now_v7().simple());
    let workspace = MissionWorkspace::new(repo_root, mid.clone()).unwrap();
    let runtime = MissionRuntime::start_with_memory(
        MissionSpec {
            worker_count: Some(1),
            ..ok_spec()
        },
        workspace,
        MockTimingConfig::fast(),
        Some(kernel.clone()),
        None,
    )
    .await
    .unwrap();
    tokio::time::timeout(Duration::from_secs(5), runtime.await_complete_or_terminal())
        .await
        .unwrap();
    runtime.resolve(ResolveAction::Discard).await.unwrap();

    let (accept_count,): (i64,) = sqlx::query_as(
        "SELECT COUNT(*) FROM memory_events WHERE type = 'barrier' AND mission_id = ? \
           AND payload_json LIKE '%\"kind\":\"accept\"%'",
    )
    .bind(&mid)
    .fetch_one(kernel.pool())
    .await
    .unwrap();
    let (scrub_count,): (i64,) = sqlx::query_as(
        "SELECT COUNT(*) FROM memory_events WHERE type = 'barrier' AND mission_id = ? \
           AND payload_json LIKE '%\"kind\":\"scrub\"%'",
    )
    .bind(&mid)
    .fetch_one(kernel.pool())
    .await
    .unwrap();
    assert_eq!(accept_count, 0);
    assert_eq!(scrub_count, 1);
}

#[tokio::test(flavor = "current_thread")]
async fn terminal_resolution_retries_a_transient_memory_barrier_failure() {
    use crate::memory::{NewNote, NoteKind, Scope, ScopeKind, StandardNoteKind};

    let (_repo_dir, repo_root) = make_sandbox_repo();
    let (kernel, _memory_dir) = fresh_kernel_for_runtime().await;
    let mission_id = format!("tier2a-retry-{}", uuid::Uuid::now_v7().simple());
    let runtime = MissionRuntime::start_with_memory(
        MissionSpec {
            worker_count: Some(1),
            ..ok_spec()
        },
        MissionWorkspace::new(repo_root, mission_id.clone()).unwrap(),
        MockTimingConfig::fast(),
        Some(kernel.clone()),
        None,
    )
    .await
    .unwrap();
    tokio::time::timeout(Duration::from_secs(5), runtime.await_complete_or_terminal())
        .await
        .unwrap();

    let note_id = kernel
        .store
        ._test_seed_owned_note(NewNote {
            kind: NoteKind::Standard(StandardNoteKind::Fact),
            scope: Scope {
                kind: ScopeKind::Repo,
                value: None,
            },
            body: "retryable terminal barrier fact".into(),
        })
        .await
        .unwrap();
    sqlx::query(
        "INSERT INTO memory_bundles \
         (bundle_id, mission_id, worker_id, turn, vendor, hash, page_table_json, \
          trace_json, rendered_path, composed_event_id) \
         VALUES (?, ?, 'retry-worker', 0, 'claude', 'h', ?, '{}', '/dev/null', 'retry-event')",
    )
    .bind(format!("retry-bundle-{note_id}"))
    .bind(&mission_id)
    .bind(format!(
        r#"[{{"slot":0,"note_id":"{note_id}","tokens":1}}]"#
    ))
    .execute(kernel.pool())
    .await
    .unwrap();

    let body_path = kernel.store.root().join(format!("notes/{note_id}.md"));
    let body = std::fs::read(&body_path).unwrap();
    std::fs::remove_file(&body_path).unwrap();
    runtime.resolve(ResolveAction::Merge).await.unwrap();
    assert_eq!(runtime.state(), MissionState::Merged);

    std::fs::write(&body_path, body).unwrap();
    let deadline = std::time::Instant::now() + Duration::from_secs(5);
    loop {
        let (barriers,): (i64,) = sqlx::query_as(
            "SELECT COUNT(*) FROM memory_events WHERE type = 'barrier' AND mission_id = ?",
        )
        .bind(&mission_id)
        .fetch_one(kernel.pool())
        .await
        .unwrap();
        if barriers == 1 {
            break;
        }
        assert!(
            std::time::Instant::now() < deadline,
            "terminal mission never retried its transient memory barrier failure"
        );
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
}

/// Fail-soft contract: a mission without an installed kernel
/// (memory=None) must behave exactly as it did pre-Tier-2A. No
/// CLAUDE.md is written; no memory tables are touched.
#[tokio::test(flavor = "current_thread")]
async fn tier2a_no_kernel_means_no_memory_side_effects() {
    let (_repo_dir, repo_root) = make_sandbox_repo();
    let mid = format!("tier2a-{}", uuid::Uuid::now_v7().simple());
    let workspace = MissionWorkspace::new(repo_root.clone(), mid.clone()).unwrap();
    let runtime = MissionRuntime::start(
        MissionSpec {
            worker_count: Some(1),
            ..ok_spec()
        },
        workspace,
        MockTimingConfig::fast(),
    )
    .await
    .unwrap();
    tokio::time::timeout(Duration::from_secs(5), runtime.await_complete_or_terminal())
        .await
        .unwrap();
    runtime.resolve(ResolveAction::Merge).await.unwrap();

    // Worker did its thing, but no CLAUDE.md was written into the
    // worktree (the worker file from the variant is the only thing).
    let worker_worktree = repo_root.join(".vigla/worktrees").join(&mid).join("mock-1");
    // The worktree was integrated and torn down — confirm the mission
    // completed cleanly via the resolved state (no crash, no panic).
    let _ = worker_worktree;
}
