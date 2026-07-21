use super::dispatch::retry_backoff;
use super::*;
use std::time::Duration;

#[derive(Debug)]
struct NoopSink;

impl crate::parser::WorkerEventSink for NoopSink {
    fn emit(&self, _event: &event_schema::Event) {}
}

#[test]
fn retry_backoff_doubles_until_shift_cap() {
    assert_eq!(retry_backoff(1, 1).as_millis(), 1);
    assert_eq!(retry_backoff(1, 2).as_millis(), 2);
    assert_eq!(retry_backoff(1, 4).as_millis(), 8);
    assert_eq!(retry_backoff(10, 5).as_millis(), 160);
}

#[test]
fn retry_backoff_clamps_shift_at_63() {
    // 1u64 << 63 = 2^63
    let max_shift = retry_backoff(1, 64).as_millis();
    assert_eq!(max_shift, 1u128 << 63);
    // attempt 65, 100, u32::MAX must not panic and must not exceed
    // the clamp.
    assert_eq!(retry_backoff(1, 65).as_millis(), 1u128 << 63);
    assert_eq!(retry_backoff(1, 100).as_millis(), 1u128 << 63);
    assert_eq!(retry_backoff(1, u32::MAX).as_millis(), 1u128 << 63);
}

#[test]
fn retry_backoff_saturates_multiplication() {
    // base_ms = u64::MAX overflows when multiplied by any 2^N > 0;
    // saturating_mul caps at u64::MAX.
    assert_eq!(retry_backoff(u64::MAX, 5).as_millis(), u64::MAX as u128);
}

#[test]
fn retry_backoff_attempt_zero_does_not_underflow() {
    // saturating_sub(1) on 0 yields 0; shift = 0; 1 << 0 = 1.
    assert_eq!(retry_backoff(7, 0).as_millis(), 7);
}

#[test]
fn locate_mock_harness_finds_sibling_in_dev_layout() {
    // Dev mode: target/debug/vigla-host with target/debug/mock-harness
    // alongside it. The sibling check resolves first.
    let dir = tempfile::tempdir().unwrap();
    let exe = dir.path().join("vigla-host");
    let sibling = dir.path().join("mock-harness");
    std::fs::write(&exe, b"fake host").unwrap();
    std::fs::write(&sibling, b"fake mock").unwrap();
    let found = locate_mock_harness_from_exe(&exe).unwrap();
    assert_eq!(found, sibling);
}

#[test]
fn locate_mock_harness_finds_macos_app_resources_dir() {
    // Bundled .app: Contents/MacOS/vigla-host with mock-harness
    // copied into Contents/Resources/ by tauri's bundle.resources
    // entry. Without this lookup the packaged app cannot spawn
    // mock workers.
    let dir = tempfile::tempdir().unwrap();
    let macos = dir.path().join("Contents").join("MacOS");
    let resources = dir.path().join("Contents").join("Resources");
    std::fs::create_dir_all(&macos).unwrap();
    std::fs::create_dir_all(&resources).unwrap();
    let exe = macos.join("vigla-host");
    let in_resources = resources.join("mock-harness");
    std::fs::write(&exe, b"fake host").unwrap();
    std::fs::write(&in_resources, b"fake mock").unwrap();
    // No sibling in Contents/MacOS — must fall through to Resources.
    assert!(!macos.join("mock-harness").exists());
    let found = locate_mock_harness_from_exe(&exe).unwrap();
    assert_eq!(found, in_resources);
}

#[test]
fn locate_mock_harness_errors_with_sibling_path_when_nothing_found() {
    let dir = tempfile::tempdir().unwrap();
    let macos = dir.path().join("Contents").join("MacOS");
    std::fs::create_dir_all(&macos).unwrap();
    let exe = macos.join("vigla-host");
    std::fs::write(&exe, b"fake host").unwrap();
    // Neither sibling nor Resources entry exists.
    let err = locate_mock_harness_from_exe(&exe).unwrap_err();
    match err {
        SupervisorError::MockHarnessMissing(p) => {
            assert_eq!(p, macos.join("mock-harness"));
        }
        other => panic!("expected MockHarnessMissing, got {other:?}"),
    }
}

#[tokio::test]
async fn failed_real_spawn_rolls_back_worker_and_task_rows() {
    let repo = Repository::open_in_memory().await.unwrap();
    let supervisor = Supervisor::new(
        repo.clone(),
        Arc::new(NoopSink),
        PathBuf::from("/unused/mock-harness"),
    );
    let missing = PathBuf::from(format!(
        "/definitely/missing/vigla-spawn-{}",
        uuid::Uuid::now_v7().simple()
    ));

    assert!(supervisor
        .spawn_claude("test prompt".into(), missing, 1)
        .await
        .is_err());

    assert!(repo.list_recent_workers(10).await.unwrap().is_empty());
    let (task_count,): (i64,) = sqlx::query_as("SELECT COUNT(*) FROM tasks")
        .fetch_one(repo.pool_for_test())
        .await
        .unwrap();
    assert_eq!(task_count, 0);
    assert!(supervisor.is_quiescent().await);
}

#[tokio::test]
async fn failed_mock_spawn_rolls_back_worker_and_task_rows() {
    let repo = Repository::open_in_memory().await.unwrap();
    let supervisor = Supervisor::new(
        repo.clone(),
        Arc::new(NoopSink),
        PathBuf::from("/definitely/missing/vigla-mock-harness"),
    );

    assert!(supervisor
        .spawn_mock(SpawnRequest::realtime("claude_happy"))
        .await
        .is_err());

    assert!(repo.list_recent_workers(10).await.unwrap().is_empty());
    let (task_count,): (i64,) = sqlx::query_as("SELECT COUNT(*) FROM tasks")
        .fetch_one(repo.pool_for_test())
        .await
        .unwrap();
    assert_eq!(task_count, 0);
    assert!(supervisor.is_quiescent().await);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn stop_waits_for_a_preparing_worker_to_activate_and_exit() {
    let repo = Repository::open_in_memory().await.unwrap();
    let supervisor = Supervisor::new(
        repo,
        Arc::new(NoopSink),
        PathBuf::from("/unused/mock-harness"),
    );
    let worker_id = "preparing-worker".to_string();
    let reservation = supervisor.reserve_worker_slot(&worker_id).await.unwrap();

    let stopping = {
        let supervisor = Arc::clone(&supervisor);
        let worker_id = worker_id.clone();
        tokio::spawn(async move { supervisor.stop(&worker_id).await })
    };

    // Prove stop has acquired the preparing entry and consumed its cancel
    // sender before activation. This is the former resume race window.
    tokio::time::timeout(Duration::from_secs(1), async {
        loop {
            let cancel_taken = supervisor
                .workers
                .lock()
                .await
                .get(&worker_id)
                .is_some_and(|entry| entry.cancel.is_none());
            if cancel_taken {
                break;
            }
            tokio::task::yield_now().await;
        }
    })
    .await
    .expect("stop should claim the preparing reservation");

    let (start_tx, start_rx) = oneshot::channel();
    let worker_supervisor = Arc::clone(&supervisor);
    let worker_for_task = worker_id.clone();
    let generation = reservation.generation;
    let join = tokio::spawn(async move {
        start_rx.await.expect("reservation activated");
        let _ = reservation.cancel_rx.await;
        worker_supervisor
            .fail_worker_slot(&worker_for_task, generation)
            .await;
    });
    supervisor
        .activate_worker_slot(&worker_id, generation, join, start_tx)
        .await
        .unwrap();

    tokio::time::timeout(Duration::from_secs(1), stopping)
        .await
        .expect("stop must not hang across activation")
        .expect("stop task joins")
        .expect("stop succeeds");
    assert!(!supervisor.is_running(&worker_id).await);
}
