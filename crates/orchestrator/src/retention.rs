//! Periodic event-tier retention sweeper.
//!
//! Owns a spawned tokio task that calls
//! [`Repository::archive_excess_for_all`] every
//! `VIGLA_RETENTION_TICK_SECS` seconds (default 60). Constructed
//! at orchestrator startup, held for the orchestrator's lifetime,
//! cancelled on `Drop`.

use crate::repository::{retention_tick_from_env, Repository};
use tokio::task::JoinHandle;

/// Background trimmer. Drop the guard to stop the task.
///
/// The task body is tolerant of repository errors — it logs and
/// re-loops rather than terminating, so a transient SQLite failure
/// (e.g. a locked DB during another write) doesn't permanently
/// stop retention.
#[derive(Debug)]
pub struct RetentionGuard {
    handle: JoinHandle<()>,
}

impl RetentionGuard {
    /// Spawn the sweeper and return a guard. The sweeper begins
    /// after one full tick (no immediate-on-spawn trim — that would
    /// double-fire with `mark_worker_ended` if a worker just ended).
    pub fn spawn(repo: Repository) -> Self {
        let tick = retention_tick_from_env();
        let cap = repo.live_cap();
        let handle = crate::spawn_supervised("retention::sweeper", async move {
            let mut interval = tokio::time::interval(tick);
            // Skip the initial fire (`interval` would fire immediately
            // by default). Wait one tick before the first sweep.
            interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
            interval.tick().await;
            loop {
                interval.tick().await;
                if let Err(e) = repo.archive_excess_for_all(cap).await {
                    tracing::warn!("orchestrator: retention sweep error: {e}");
                }
            }
        });
        Self { handle }
    }
}

impl Drop for RetentionGuard {
    fn drop(&mut self) {
        self.handle.abort();
    }
}
