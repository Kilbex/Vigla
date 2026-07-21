//! Fail-soft async sink that bridges adapter-extracted memory intents
//! to the Memory Kernel.
//!
//! The parser side (`adapter_core::extract_intents`) and the kernel
//! side (`route_intent`) are both pure async functions. This module is
//! the glue that the per-vendor worker pipeline calls without owning
//! a `tokio::runtime::Handle` or knowing about the kernel. The
//! orchestrator builds a `KernelIntentSink` once per worker dispatch
//! and hands it to the parser as `&dyn MemoryIntentSink`.
//!
//! ## Fail-soft contract (release gate)
//!
//! Routing errors are logged to stderr; they NEVER propagate up into
//! the mission lifecycle. A worker proposal that hits a kernel-side
//! taxonomy mismatch or scanner rejection is treated the same way:
//! we log + drop, the worker continues, the mission completes.

use std::fmt;
use std::sync::Arc;

use tokio::sync::Semaphore;

use adapter_core::MemoryIntent;

use super::intent_router::route_intent;
use super::kernel::{MemoryKernel, ProposalOutcome};

/// Trait implemented by anything that wants to receive worker memory
/// intents extracted by the parser. The orchestrator-side
/// [`process_with_adapter`] path drains intents after each
/// `ingest_line` and calls `emit` for each.
///
/// Implementations must be `Send + Sync` because the parser runs on
/// the supervisor's task; intents may be processed asynchronously on
/// the same runtime.
pub trait MemoryIntentSink: Send + Sync + fmt::Debug {
    /// Process one intent. Fire-and-forget: the sink is responsible
    /// for any async work; the parser does not await.
    fn emit(&self, intent: MemoryIntent);
}

/// Maximum number of intent-routing tasks in flight at once (shared
/// across all clones of a sink). Beyond this, new intents are dropped
/// with a warning rather than spawned without limit — a runaway worker
/// could otherwise pile up tasks (and concurrent SQLite writes) without
/// bound, and a runtime shutdown could abort many of them mid-write.
/// Dropping is consistent with the fail-soft memory contract (F-12).
const MAX_INFLIGHT_INTENTS: usize = 64;

/// Concrete sink that routes intents through the Memory Kernel. Spawns
/// a tokio task per intent (up to [`MAX_INFLIGHT_INTENTS`] concurrently)
/// so the parser loop never blocks on the kernel's SQLite write. Failures
/// are logged and dropped — the fail-soft contract above.
#[derive(Clone)]
pub struct KernelIntentSink {
    kernel: Arc<MemoryKernel>,
    mission_id: Arc<str>,
    worker_id: Arc<str>,
    /// Shared across clones (Arc) so the ceiling is per-sink, not
    /// per-clone.
    inflight: Arc<Semaphore>,
}

impl fmt::Debug for KernelIntentSink {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("KernelIntentSink")
            .field("mission_id", &self.mission_id)
            .field("worker_id", &self.worker_id)
            .finish()
    }
}

impl KernelIntentSink {
    pub fn new(
        kernel: Arc<MemoryKernel>,
        mission_id: impl Into<Arc<str>>,
        worker_id: impl Into<Arc<str>>,
    ) -> Self {
        Self {
            kernel,
            mission_id: mission_id.into(),
            worker_id: worker_id.into(),
            inflight: Arc::new(Semaphore::new(MAX_INFLIGHT_INTENTS)),
        }
    }
}

impl MemoryIntentSink for KernelIntentSink {
    fn emit(&self, intent: MemoryIntent) {
        // Bound concurrent in-flight routing tasks. try_acquire never
        // blocks the synchronous parser loop: if MAX_INFLIGHT_INTENTS are
        // already routing, drop this intent with a warning rather than
        // spawning without limit. Memory proposals are best-effort, so a
        // drop under extreme backpressure is acceptable and logged (F-12).
        let permit = match Arc::clone(&self.inflight).try_acquire_owned() {
            Ok(permit) => permit,
            Err(_) => {
                tracing::warn!(
                    "vigla: dropping worker memory intent for mission {}, worker {} — \
                     {MAX_INFLIGHT_INTENTS} routing tasks already in flight (backpressure)",
                    self.mission_id,
                    self.worker_id
                );
                return;
            }
        };
        let kernel = self.kernel.clone();
        let mission_id = self.mission_id.clone();
        let worker_id = self.worker_id.clone();
        // Spawn so the synchronous emit() returns immediately and the
        // parser loop continues. The permit is released when this task
        // ends, freeing a slot for the next intent.
        tokio::spawn(async move {
            let _permit = permit;
            match route_intent(&kernel, &mission_id, &worker_id, intent).await {
                Ok(ProposalOutcome::Accepted { proposal_id }) => {
                    // The frontend learns about new pending proposals
                    // via memory events on the next event-list query;
                    // no additional emission is needed here.
                    let _ = proposal_id;
                }
                Ok(ProposalOutcome::Rejected {
                    proposal_id,
                    reason,
                }) => {
                    tracing::error!(
                        "vigla: worker proposal {proposal_id} rejected ({reason:?}) for \
                         mission {mission_id}, worker {worker_id}"
                    );
                }
                Err(e) => {
                    tracing::error!(
                        "vigla: worker memory intent routing failed for mission \
                         {mission_id}, worker {worker_id}: {e}"
                    );
                }
            }
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use adapter_core::{ProposeIntent, ScopeIntent};
    use sqlx::sqlite::{SqliteConnectOptions, SqlitePoolOptions};
    use std::str::FromStr;
    use tempfile::TempDir;

    async fn fresh_kernel() -> (Arc<MemoryKernel>, TempDir) {
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
        let dir = TempDir::new().unwrap();
        let kernel = MemoryKernel::open(pool, dir.path().to_path_buf())
            .await
            .unwrap();
        (Arc::new(kernel), dir)
    }

    fn propose() -> MemoryIntent {
        MemoryIntent::Propose(ProposeIntent {
            kind: "hazard".into(),
            scope: ScopeIntent {
                kind: "repo".into(),
                value: None,
            },
            body: "Resume tokens are host-bound.".into(),
            derived_from: vec!["worktree:src/x.rs:42".into()],
            evidence_event_ids: vec![],
        })
    }

    /// emit() spawns; we poll the kernel to observe the side effect
    /// landing. A bounded retry loop keeps the test deterministic
    /// even under heavy load.
    async fn wait_for_pending(kernel: &MemoryKernel) {
        for _ in 0..50 {
            let (count,): (i64,) = sqlx::query_as("SELECT COUNT(*) FROM memory_pending")
                .fetch_one(kernel.pool())
                .await
                .unwrap();
            if count > 0 {
                return;
            }
            tokio::time::sleep(std::time::Duration::from_millis(5)).await;
        }
        panic!("kernel never observed the routed intent");
    }

    #[tokio::test]
    async fn sink_routes_intent_to_kernel_via_spawned_task() {
        let (kernel, _dir) = fresh_kernel().await;
        let sink = KernelIntentSink::new(kernel.clone(), "mid", "wid");
        sink.emit(propose());
        wait_for_pending(&kernel).await;
        let (mid, wid): (String, String) =
            sqlx::query_as("SELECT mission_id, worker_id FROM memory_pending")
                .fetch_one(kernel.pool())
                .await
                .unwrap();
        assert_eq!(mid, "mid");
        assert_eq!(wid, "wid");
    }

    #[tokio::test]
    async fn sink_swallows_routing_errors_without_panicking() {
        let (kernel, _dir) = fresh_kernel().await;
        let sink = KernelIntentSink::new(kernel.clone(), "mid", "wid");
        // Unknown kind — router returns Err. The sink must log + drop.
        sink.emit(MemoryIntent::Propose(ProposeIntent {
            kind: "totally-not-real".into(),
            scope: ScopeIntent {
                kind: "repo".into(),
                value: None,
            },
            body: "x".into(),
            derived_from: vec![],
            evidence_event_ids: vec![],
        }));
        // Give the spawned task time to error out, then verify the
        // kernel saw no pending rows.
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;
        let (count,): (i64,) = sqlx::query_as("SELECT COUNT(*) FROM memory_pending")
            .fetch_one(kernel.pool())
            .await
            .unwrap();
        assert_eq!(count, 0);
    }
}
