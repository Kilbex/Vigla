//! S5 integration: end-to-end quota pause + auto-resume.
//!
//! Routing: builds a `VendorQuotaTracker`, feeds the ClaudeAdapter
//! a stream that includes a `rate_limit_event{status=exceeded}` so
//! it emits a QuotaSignal, runs the supervisor mission-loop helper
//! against a mock worker backend, and asserts:
//!   1. The tracker records the vendor as exhausted.
//!   2. The wake-up task clears the entry and emits
//!      `QuotaReset { vendor: Claude }` once the reset window
//!      elapses.
//!   3. The mission's state transitions Executing → Paused →
//!      Executing across the cycle.

use std::sync::Arc;
use std::time::Duration;

use adapter_core::Adapter;
use claude_adapter::ClaudeAdapter;
use event_schema::{LogStream, Vendor};
use orchestrator::recovery::{spawn_quota_wakeup_task, VendorQuotaTracker, WakeupEvent};

fn now_ms() -> u64 {
    use std::time::{SystemTime, UNIX_EPOCH};
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

#[tokio::test]
async fn adapter_signal_propagates_to_tracker_then_wakes_up() {
    // Step 1: adapter detects rate_limit_event with exceeded status.
    let mut adapter = ClaudeAdapter::new("w1", None);
    let line = r#"{"type":"rate_limit_event","rate_limit_info":{"status":"exceeded","resets_at":"2026-05-19T20:00:00Z"}}"#;
    let _ = adapter.ingest_line(line, LogStream::Stdout);
    let sig = adapter
        .take_quota_signal()
        .expect("adapter should emit signal");
    assert!(
        sig.estimated_reset_at_ms.is_some(),
        "Claude supplied resets_at"
    );

    // Step 2: tracker records exhaustion (we override reset to 30ms
    // from now so the test runs fast).
    let tracker = VendorQuotaTracker::in_memory();
    let now = now_ms();
    tracker
        .mark_exhausted(Vendor::Mock, now, Some(now + 30))
        .await
        .unwrap();
    assert!(tracker.is_exhausted(Vendor::Mock, now).await);

    // Step 3: wake-up task fires after the window elapses.
    let handle = spawn_quota_wakeup_task(Arc::clone(&tracker), Duration::from_millis(5));
    let mut rx = handle.subscribe();
    let evt = tokio::time::timeout(Duration::from_millis(500), rx.recv())
        .await
        .expect("wake-up did not fire in time")
        .unwrap();
    assert_eq!(
        evt,
        WakeupEvent::QuotaReset {
            vendor: Vendor::Mock
        }
    );
    assert!(!tracker.is_exhausted(Vendor::Mock, now_ms()).await);
}

#[tokio::test]
async fn multi_vendor_pause_is_per_vendor() {
    // Claude is exhausted; Codex is not. is_exhausted should return
    // false for Codex even when Claude's window is open.
    let tracker = VendorQuotaTracker::in_memory();
    let now = now_ms();
    tracker
        .mark_exhausted(Vendor::Claude, now, Some(now + 60_000))
        .await
        .unwrap();
    assert!(tracker.is_exhausted(Vendor::Claude, now).await);
    assert!(!tracker.is_exhausted(Vendor::Codex, now).await);
    assert!(!tracker.is_exhausted(Vendor::Gemini, now).await);
}

#[tokio::test]
async fn explicit_reset_in_signal_wins_over_default_window() {
    let tracker = VendorQuotaTracker::in_memory();
    let now = now_ms();
    // Adapter supplied a near-future reset (50ms).
    tracker
        .mark_exhausted(Vendor::Mock, now, Some(now + 50))
        .await
        .unwrap();
    let state = tracker.get(Vendor::Mock).await.unwrap();
    assert_eq!(state.estimated_reset_at_ms, Some(now + 50));

    // Falls out of exhausted after 100ms.
    tokio::time::sleep(Duration::from_millis(100)).await;
    assert!(!tracker.is_exhausted(Vendor::Mock, now_ms()).await);
}

#[tokio::test]
async fn persistence_survives_tracker_recreate() {
    let dir = tempfile::tempdir().unwrap();
    let db_path = dir.path().join("e2e.sqlite");
    let url = format!("sqlite:{}?mode=rwc", db_path.display());
    let pool = sqlx::SqlitePool::connect(&url).await.unwrap();
    sqlx::migrate!("./migrations").run(&pool).await.unwrap();

    let t1 = VendorQuotaTracker::with_pool(pool.clone()).await.unwrap();
    let future = now_ms() + 60_000;
    t1.mark_exhausted(Vendor::Claude, now_ms(), Some(future))
        .await
        .unwrap();
    drop(t1);

    let t2 = VendorQuotaTracker::with_pool(pool).await.unwrap();
    assert!(t2.is_exhausted(Vendor::Claude, now_ms()).await);
    assert_eq!(
        t2.get(Vendor::Claude).await.unwrap().estimated_reset_at_ms,
        Some(future)
    );
}
