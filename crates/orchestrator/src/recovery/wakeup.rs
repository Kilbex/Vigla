//! Wake-up task. Watches
//! [`crate::recovery::VendorQuotaTracker::next_reset`] and, when
//! the earliest reset time elapses, broadcasts a
//! [`WakeupEvent::QuotaReset { vendor }`] on the supplied channel.
//! The mission runtime consumes these to transition paused missions
//! back to `Executing`.
//!
//! The task is a single tokio future; cloning the returned
//! [`QuotaWakeupHandle`] is how callers stop or query it.

use std::sync::Arc;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use event_schema::Vendor;
use tokio::sync::broadcast;
use tokio::task::JoinHandle;
use tokio::time::sleep;

use crate::recovery::quota::VendorQuotaTracker;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WakeupEvent {
    /// The named vendor's quota window has elapsed; mission runtime
    /// should resume any missions paused on this vendor.
    QuotaReset { vendor: Vendor },
}

#[derive(Debug)]
pub struct QuotaWakeupHandle {
    pub events: broadcast::Sender<WakeupEvent>,
    join: Option<JoinHandle<()>>,
}

impl QuotaWakeupHandle {
    /// Subscribe to wake-up events. Each receiver gets an
    /// independent buffered queue.
    pub fn subscribe(&self) -> broadcast::Receiver<WakeupEvent> {
        self.events.subscribe()
    }

    /// Cancel the background task. Idempotent.
    pub fn shutdown(&mut self) {
        if let Some(h) = self.join.take() {
            h.abort();
        }
    }
}

impl Drop for QuotaWakeupHandle {
    fn drop(&mut self) {
        self.shutdown();
    }
}

/// Spawn the wake-up task. Polls `tracker.next_reset()` every
/// `poll_interval`; when an exhausted vendor's reset time has
/// passed, clears the tracker entry and broadcasts a
/// `QuotaReset { vendor }` event.
pub fn spawn_quota_wakeup_task(
    tracker: Arc<VendorQuotaTracker>,
    poll_interval: Duration,
) -> QuotaWakeupHandle {
    let (tx, _rx_keep) = broadcast::channel::<WakeupEvent>(64);
    let tx_for_task = tx.clone();
    let join = tokio::spawn(async move {
        loop {
            let now = now_unix_ms();
            // Snapshot all exhausted vendors and check each.
            let states_to_clear = exhausted_at_or_before(&tracker, now).await;
            for vendor in states_to_clear {
                let _ = tracker.clear(vendor).await;
                let _ = tx_for_task.send(WakeupEvent::QuotaReset { vendor });
            }
            sleep(poll_interval).await;
        }
    });
    QuotaWakeupHandle {
        events: tx,
        join: Some(join),
    }
}

async fn exhausted_at_or_before(tracker: &VendorQuotaTracker, now_ms: u64) -> Vec<Vendor> {
    let mut out = Vec::new();
    // Poll exactly the vendors that currently have tracked state. This
    // can never drift out of sync with the `Vendor` enum the way a
    // hand-maintained list did (it previously omitted Antigravity,
    // Kiro, and Copilot, leaving missions paused on those vendors
    // unable to ever auto-resume).
    for vendor in tracker.tracked_vendors().await {
        if let Some(state) = tracker.get(vendor).await {
            if let Some(reset) = state.estimated_reset_at_ms {
                if reset <= now_ms {
                    out.push(vendor);
                }
            }
        }
    }
    out
}

fn now_unix_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    #[tokio::test(start_paused = false)]
    async fn fires_quota_reset_after_window_elapses() {
        let tracker = VendorQuotaTracker::in_memory();
        let handle = spawn_quota_wakeup_task(Arc::clone(&tracker), Duration::from_millis(10));
        let mut rx = handle.subscribe();

        // Mark Mock vendor exhausted with a reset 30ms from now.
        let now = now_unix_ms();
        tracker
            .mark_exhausted(Vendor::Mock, now, Some(now + 30))
            .await
            .unwrap();
        assert!(tracker.is_exhausted(Vendor::Mock, now).await);

        // Wait for the wake-up; the poll loop should fire within
        // ~50ms.
        let ev = tokio::time::timeout(Duration::from_millis(500), rx.recv())
            .await
            .expect("wake-up did not arrive")
            .expect("channel closed");
        assert_eq!(
            ev,
            WakeupEvent::QuotaReset {
                vendor: Vendor::Mock
            }
        );
        assert!(!tracker.is_exhausted(Vendor::Mock, now_unix_ms()).await);
    }

    #[tokio::test(start_paused = false)]
    async fn fires_quota_reset_for_every_vendor() {
        // Regression: the poll list must cover every `Vendor`, not a
        // hand-maintained subset. Antigravity/Kiro/Copilot were
        // omitted, so a mission paused on those vendors would never
        // see a `QuotaReset` and would hang forever. Copilot stands in
        // for the previously-missing tail.
        let tracker = VendorQuotaTracker::in_memory();
        let handle = spawn_quota_wakeup_task(Arc::clone(&tracker), Duration::from_millis(10));
        let mut rx = handle.subscribe();

        let now = now_unix_ms();
        tracker
            .mark_exhausted(Vendor::Copilot, now, Some(now + 30))
            .await
            .unwrap();

        let ev = tokio::time::timeout(Duration::from_millis(500), rx.recv())
            .await
            .expect("wake-up did not arrive for Copilot")
            .expect("channel closed");
        assert_eq!(
            ev,
            WakeupEvent::QuotaReset {
                vendor: Vendor::Copilot
            }
        );
        assert!(!tracker.is_exhausted(Vendor::Copilot, now_unix_ms()).await);
    }

    #[tokio::test(start_paused = false)]
    async fn does_not_fire_for_future_resets() {
        let tracker = VendorQuotaTracker::in_memory();
        let handle = spawn_quota_wakeup_task(Arc::clone(&tracker), Duration::from_millis(10));
        let mut rx = handle.subscribe();

        // Reset 2 seconds out — definitely won't elapse during the
        // 100ms timeout below.
        let now = now_unix_ms();
        tracker
            .mark_exhausted(Vendor::Mock, now, Some(now + 2_000))
            .await
            .unwrap();

        let res = tokio::time::timeout(Duration::from_millis(100), rx.recv()).await;
        assert!(res.is_err(), "should not have received an event");
    }

    #[tokio::test(start_paused = false)]
    async fn handle_shutdown_stops_polling() {
        let tracker = VendorQuotaTracker::in_memory();
        let mut handle = spawn_quota_wakeup_task(Arc::clone(&tracker), Duration::from_millis(10));
        handle.shutdown();
        // Re-shutdown is a no-op.
        handle.shutdown();
    }
}
