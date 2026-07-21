//! Per-vendor quota state. Owned by `MissionRuntime` and shared
//! across missions — quota windows are vendor-scoped, not
//! mission-scoped.
//!
//! Persists to `vendor_quota_state` (migration 0009) so a host
//! restart does not double-charge against the window: the wake-up
//! task reads `estimated_reset_at_ms` on startup and either
//! resumes paused work immediately (if the time has already passed)
//! or schedules itself to fire at the persisted timestamp.

use std::collections::HashMap;
use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};

use event_schema::Vendor;
use sqlx::SqlitePool;
use tokio::sync::RwLock;

/// Default rolling window per vendor. Source: vendor docs as of
/// 2026-05-19. Claude's 5-hour window is the headline case; the
/// others use 1h as a conservative fallback because their
/// rate-limit windows are typically shorter and re-checking
/// sooner is cheap.
pub fn default_window_ms(vendor: Vendor) -> u64 {
    match vendor {
        Vendor::Claude => 5 * 60 * 60 * 1000,
        Vendor::Codex => 60 * 60 * 1000,
        Vendor::Gemini => 60 * 60 * 1000,
        Vendor::Antigravity => 60 * 60 * 1000,
        Vendor::Kiro => 60 * 60 * 1000,
        Vendor::Copilot => 60 * 60 * 1000,
        Vendor::Opencode => 60 * 60 * 1000,
        Vendor::Mock => 100, // tests want fast resets
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum QuotaSignalSource {
    /// The adapter parsed a vendor-specific quota error and
    /// supplied an explicit reset timestamp.
    AdapterParsed,
    /// The adapter detected exhaustion but did not provide a reset
    /// time; the tracker filled in `now + default_window_ms`.
    Inferred,
}

impl QuotaSignalSource {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::AdapterParsed => "adapter_parsed",
            Self::Inferred => "inferred",
        }
    }

    pub fn parse(s: &str) -> Option<Self> {
        match s {
            "adapter_parsed" => Some(Self::AdapterParsed),
            "inferred" => Some(Self::Inferred),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VendorQuotaState {
    pub vendor: Vendor,
    pub last_exhausted_at_ms: Option<u64>,
    pub estimated_reset_at_ms: Option<u64>,
    pub source: QuotaSignalSource,
}

/// Thread-safe registry. Cloning the `Arc<VendorQuotaTracker>` is
/// how missions share state.
#[derive(Debug)]
pub struct VendorQuotaTracker {
    states: RwLock<HashMap<Vendor, VendorQuotaState>>,
    pool: Option<SqlitePool>,
}

/// Process-wide persistent tracker, installed once by the host at
/// startup via [`install_shared_tracker`]. The vendor quota window is a
/// per-host resource (one account per machine per vendor), so a single
/// shared instance is correct: every mission's event bus reads it,
/// quota pauses are visible across concurrent missions, and the
/// sqlite-backed state survives a host restart. Tests never install it,
/// so they transparently fall back to a per-bus in-memory tracker.
static SHARED_TRACKER: std::sync::OnceLock<Arc<VendorQuotaTracker>> = std::sync::OnceLock::new();

/// Install the process-wide persistent tracker. Set-once — the first
/// writer wins and later calls are no-ops. Call exactly once at host
/// startup, after the DB pool is open and migrations have run.
pub fn install_shared_tracker(tracker: Arc<VendorQuotaTracker>) {
    let _ = SHARED_TRACKER.set(tracker);
}

/// The installed shared tracker, or a fresh in-memory one when none is
/// installed (tests and any non-host caller). Backs the per-mission
/// event-bus quota tracker in [`crate::mission_runtime`].
pub(crate) fn shared_or_in_memory() -> Arc<VendorQuotaTracker> {
    SHARED_TRACKER
        .get()
        .cloned()
        .unwrap_or_else(VendorQuotaTracker::in_memory)
}

impl VendorQuotaTracker {
    /// In-memory only — for unit tests. `mark_exhausted` and
    /// `load_from_db` will skip the persistence side.
    pub fn in_memory() -> Arc<Self> {
        Arc::new(Self {
            states: RwLock::new(HashMap::new()),
            pool: None,
        })
    }

    /// Persistent — rehydrates from the `vendor_quota_state` table
    /// on construction. `mark_exhausted` will write through.
    pub async fn with_pool(pool: SqlitePool) -> Result<Arc<Self>, sqlx::Error> {
        let me = Arc::new(Self {
            states: RwLock::new(HashMap::new()),
            pool: Some(pool),
        });
        me.load_from_db().await?;
        Ok(me)
    }

    /// Mark a vendor as exhausted at `now_unix_ms`. If
    /// `explicit_reset` is `Some`, use it; otherwise fill in
    /// `now + default_window_ms(vendor)`.
    pub async fn mark_exhausted(
        &self,
        vendor: Vendor,
        now_unix_ms: u64,
        explicit_reset: Option<u64>,
    ) -> Result<(), sqlx::Error> {
        let (reset, source) = match explicit_reset {
            Some(r) => (r, QuotaSignalSource::AdapterParsed),
            None => (
                now_unix_ms.saturating_add(default_window_ms(vendor)),
                QuotaSignalSource::Inferred,
            ),
        };
        let state = VendorQuotaState {
            vendor,
            last_exhausted_at_ms: Some(now_unix_ms),
            estimated_reset_at_ms: Some(reset),
            source,
        };
        self.states.write().await.insert(vendor, state);
        if let Some(pool) = &self.pool {
            persist_state(pool, vendor, now_unix_ms, reset, source).await?;
        }
        Ok(())
    }

    /// Clear a vendor's exhaustion record. Called by the wake-up
    /// task after the reset window has elapsed and the mission
    /// loop is about to re-dispatch.
    pub async fn clear(&self, vendor: Vendor) -> Result<(), sqlx::Error> {
        self.states.write().await.remove(&vendor);
        if let Some(pool) = &self.pool {
            sqlx::query("DELETE FROM vendor_quota_state WHERE vendor = ?")
                .bind(vendor_key(vendor))
                .execute(pool)
                .await?;
        }
        Ok(())
    }

    /// True if `vendor` is exhausted AND its reset time is in the
    /// future. Stale entries (reset already passed) return false.
    pub async fn is_exhausted(&self, vendor: Vendor, now_unix_ms: u64) -> bool {
        let states = self.states.read().await;
        let Some(state) = states.get(&vendor) else {
            return false;
        };
        let Some(reset) = state.estimated_reset_at_ms else {
            return false;
        };
        reset > now_unix_ms
    }

    /// Earliest reset across all exhausted vendors, in Unix ms.
    /// `None` if no vendor is exhausted.
    pub async fn next_reset(&self) -> Option<u64> {
        self.states
            .read()
            .await
            .values()
            .filter_map(|s| s.estimated_reset_at_ms)
            .min()
    }

    /// Snapshot a copy of the current state for the given vendor.
    pub async fn get(&self, vendor: Vendor) -> Option<VendorQuotaState> {
        self.states.read().await.get(&vendor).cloned()
    }

    /// Snapshot the set of vendors that currently have tracked state.
    /// The wake-up task iterates this instead of a hand-maintained
    /// vendor list, so its poll set can never drift out of sync with
    /// the `Vendor` enum (e.g. omitting Antigravity/Kiro/Copilot).
    pub async fn tracked_vendors(&self) -> Vec<Vendor> {
        self.states.read().await.keys().copied().collect()
    }

    /// Re-read all rows from disk into the in-memory map. Discards
    /// rows whose `estimated_reset_at_ms` has already passed.
    pub async fn load_from_db(&self) -> Result<(), sqlx::Error> {
        let Some(pool) = &self.pool else {
            return Ok(());
        };
        let rows: Vec<(String, Option<i64>, Option<i64>, String)> = sqlx::query_as(
            "SELECT vendor, last_exhausted_at_ms, estimated_reset_at_ms, source
             FROM vendor_quota_state",
        )
        .fetch_all(pool)
        .await?;
        let mut map = self.states.write().await;
        map.clear();
        let now = now_unix_ms_default();
        for (v, last, reset, src) in rows {
            let Some(vendor) = parse_vendor_key(&v) else {
                continue;
            };
            let reset_u = reset.and_then(|r| u64::try_from(r).ok());
            if let Some(r) = reset_u {
                if r <= now {
                    // Stale: don't rehydrate.
                    continue;
                }
            }
            let source = QuotaSignalSource::parse(&src).unwrap_or(QuotaSignalSource::Inferred);
            map.insert(
                vendor,
                VendorQuotaState {
                    vendor,
                    last_exhausted_at_ms: last.and_then(|l| u64::try_from(l).ok()),
                    estimated_reset_at_ms: reset_u,
                    source,
                },
            );
        }
        Ok(())
    }
}

fn now_unix_ms_default() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

async fn persist_state(
    pool: &SqlitePool,
    vendor: Vendor,
    now_unix_ms: u64,
    reset_unix_ms: u64,
    source: QuotaSignalSource,
) -> Result<(), sqlx::Error> {
    sqlx::query(
        "INSERT INTO vendor_quota_state
             (vendor, last_exhausted_at_ms, estimated_reset_at_ms, source)
         VALUES (?, ?, ?, ?)
         ON CONFLICT(vendor) DO UPDATE SET
             last_exhausted_at_ms = excluded.last_exhausted_at_ms,
             estimated_reset_at_ms = excluded.estimated_reset_at_ms,
             source = excluded.source",
    )
    .bind(vendor_key(vendor))
    .bind(now_unix_ms as i64)
    .bind(reset_unix_ms as i64)
    .bind(source.as_str())
    .execute(pool)
    .await?;
    Ok(())
}

fn vendor_key(vendor: Vendor) -> &'static str {
    match vendor {
        Vendor::Claude => "claude",
        Vendor::Codex => "codex",
        Vendor::Gemini => "gemini",
        Vendor::Antigravity => "antigravity",
        Vendor::Kiro => "kiro",
        Vendor::Copilot => "copilot",
        Vendor::Opencode => "opencode",
        Vendor::Mock => "mock",
    }
}

fn parse_vendor_key(s: &str) -> Option<Vendor> {
    match s {
        "claude" => Some(Vendor::Claude),
        "codex" => Some(Vendor::Codex),
        "gemini" => Some(Vendor::Gemini),
        "antigravity" => Some(Vendor::Antigravity),
        "kiro" => Some(Vendor::Kiro),
        "copilot" => Some(Vendor::Copilot),
        "opencode" => Some(Vendor::Opencode),
        "mock" => Some(Vendor::Mock),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use sqlx::SqlitePool;
    use tempfile::tempdir;

    async fn fresh_pool() -> (SqlitePool, tempfile::TempDir) {
        let dir = tempdir().unwrap();
        let db_path = dir.path().join("quota.sqlite");
        let url = format!("sqlite:{}?mode=rwc", db_path.display());
        let pool = SqlitePool::connect(&url).await.unwrap();
        sqlx::migrate!("./migrations").run(&pool).await.unwrap();
        (pool, dir)
    }

    #[tokio::test]
    async fn in_memory_mark_and_query() {
        let t = VendorQuotaTracker::in_memory();
        assert!(!t.is_exhausted(Vendor::Claude, 0).await);
        t.mark_exhausted(Vendor::Claude, 1_000, None).await.unwrap();
        // 5h window after now=1_000 → reset = 1_000 + 18_000_000.
        assert!(t.is_exhausted(Vendor::Claude, 1_001).await);
        assert!(!t.is_exhausted(Vendor::Claude, 1_000 + 18_000_000 + 1).await);
    }

    #[tokio::test]
    async fn explicit_reset_overrides_window() {
        let t = VendorQuotaTracker::in_memory();
        t.mark_exhausted(Vendor::Codex, 1_000, Some(2_000))
            .await
            .unwrap();
        let state = t.get(Vendor::Codex).await.unwrap();
        assert_eq!(state.estimated_reset_at_ms, Some(2_000));
        assert_eq!(state.source, QuotaSignalSource::AdapterParsed);
    }

    #[tokio::test]
    async fn inferred_source_when_no_explicit_reset() {
        let t = VendorQuotaTracker::in_memory();
        t.mark_exhausted(Vendor::Gemini, 1_000, None).await.unwrap();
        let state = t.get(Vendor::Gemini).await.unwrap();
        assert_eq!(state.source, QuotaSignalSource::Inferred);
    }

    #[tokio::test]
    async fn next_reset_picks_earliest() {
        let t = VendorQuotaTracker::in_memory();
        t.mark_exhausted(Vendor::Claude, 0, Some(10_000))
            .await
            .unwrap();
        t.mark_exhausted(Vendor::Codex, 0, Some(5_000))
            .await
            .unwrap();
        t.mark_exhausted(Vendor::Gemini, 0, Some(20_000))
            .await
            .unwrap();
        assert_eq!(t.next_reset().await, Some(5_000));
    }

    #[tokio::test]
    async fn clear_drops_state() {
        let t = VendorQuotaTracker::in_memory();
        t.mark_exhausted(Vendor::Claude, 0, Some(5_000))
            .await
            .unwrap();
        t.clear(Vendor::Claude).await.unwrap();
        assert!(t.get(Vendor::Claude).await.is_none());
        assert!(!t.is_exhausted(Vendor::Claude, 1_000).await);
    }

    #[tokio::test]
    async fn persistence_round_trip() {
        let (pool, _dir) = fresh_pool().await;
        let t = VendorQuotaTracker::with_pool(pool.clone()).await.unwrap();
        t.mark_exhausted(Vendor::Claude, 100, Some(9_999_999_999_999))
            .await
            .unwrap();

        // Build a fresh tracker over the same pool → reloads from disk.
        let t2 = VendorQuotaTracker::with_pool(pool).await.unwrap();
        let state = t2.get(Vendor::Claude).await.unwrap();
        assert_eq!(state.estimated_reset_at_ms, Some(9_999_999_999_999));
        assert_eq!(state.source, QuotaSignalSource::AdapterParsed);
    }

    #[tokio::test]
    async fn stale_rows_are_discarded_on_load() {
        let (pool, _dir) = fresh_pool().await;
        let t = VendorQuotaTracker::with_pool(pool.clone()).await.unwrap();
        t.mark_exhausted(Vendor::Claude, 0, Some(1)).await.unwrap();

        // Reload; the row's reset is 1 ms (already past now) → drop.
        let t2 = VendorQuotaTracker::with_pool(pool).await.unwrap();
        assert!(t2.get(Vendor::Claude).await.is_none());
    }
}
