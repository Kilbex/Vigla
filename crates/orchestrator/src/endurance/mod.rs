//! Endurance: durable liveness + stall detection + the all-day gate.
//!
//! Pillar A of the "All-Day Fleet" initiative (U10). It complements the
//! existing `recovery::quota` pause/resume and SQLite persistence with
//! the one thing they lack: a **crash-surviving** answer to "is the
//! orchestrator alive and making progress *right now*?", plus the
//! metrics that let us *prove* an unattended 24-hour run actually held.
//!
//! Three pieces:
//!   * [`heartbeat`] — the durable record + atomic file IO.
//!   * [`stall`] — pure liveness classification
//!     (healthy / idle / stalled / crashed).
//!   * [`metrics`] — the [`EnduranceReport`] + [`EnduranceGate`] all-day gate.
//!
//! The [`EnduranceMonitor`] ties them together over an injectable
//! [`Clock`], so a full simulated day compresses into a millisecond test
//! (see `tests/endurance_soak.rs`).
//!
//! ## Restart semantics
//!
//! `EnduranceMonitor::launch` reads any prior heartbeat at the same path:
//!   * none / unreadable → fresh start (corruption never crashes launch);
//!   * present → resume. `beat_seq`, the fault counters, and the
//!     first-launch service clock all carry forward. If the prior beat is
//!     older than the crash threshold the restart was *unclean*, so we
//!     record it as one fault injected **and** recovered — keeping the
//!     gate honest without needing the dying process to have cooperated.

mod heartbeat;
mod metrics;
mod stall;

pub use heartbeat::{
    heartbeat_dir, heartbeat_path, read as read_heartbeat, Heartbeat, HEARTBEAT_FILE,
    HEARTBEAT_SCHEMA_VERSION,
};
pub use metrics::{EnduranceGate, EnduranceReport, GateOutcome};
pub use stall::{classify, Liveness};

use std::path::{Path, PathBuf};
use thiserror::Error;

/// Failure surface for the endurance subsystem. Mirrors the shape of
/// [`crate::RepositoryError`] (io + json) but stays a distinct type so
/// the persistence and endurance concerns don't bleed into one enum.
#[derive(Debug, Error)]
pub enum EnduranceError {
    #[error("io: {0}")]
    Io(#[from] std::io::Error),
    #[error("json: {0}")]
    Json(#[from] serde_json::Error),
}

/// Injectable clock so a 24-hour endurance run is deterministic and fast
/// in tests. Production uses [`SystemClock`]; tests/simulation drive a
/// manual clock.
pub trait Clock: Send + Sync {
    /// Unix-epoch milliseconds.
    fn now_ms(&self) -> u64;
}

/// Wall-clock implementation of [`Clock`].
#[derive(Debug, Clone, Copy, Default)]
pub struct SystemClock;

impl Clock for SystemClock {
    fn now_ms(&self) -> u64 {
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| u64::try_from(d.as_millis()).unwrap_or(u64::MAX))
            .unwrap_or(0)
    }
}

/// Thresholds for liveness classification. Defaults are tuned for an
/// orchestrator that beats roughly once a minute.
#[derive(Debug, Clone, Copy)]
pub struct EnduranceConfig {
    /// No beat within this long ⇒ presumed crashed/hung.
    pub crash_threshold_ms: u64,
    /// Work in flight but no progress this long ⇒ stalled.
    pub stall_threshold_ms: u64,
    /// No work in flight + quiet this long ⇒ idle (expected, not an alert).
    pub idle_grace_ms: u64,
}

impl Default for EnduranceConfig {
    fn default() -> Self {
        Self {
            crash_threshold_ms: 90_000,  // 90s without a beat = dead
            stall_threshold_ms: 600_000, // 10min no progress w/ work = stalled
            idle_grace_ms: 300_000,      // 5min quiet w/o work = idle
        }
    }
}

/// Delta applied to the heartbeat on a [`EnduranceMonitor::beat`]. Every
/// field is optional; `None` leaves the prior value untouched. A beat
/// that advances `events_total` counts as progress automatically.
#[derive(Debug, Clone, Default)]
pub struct BeatStatus {
    pub phase: Option<String>,
    /// `Some(Some(id))` sets the mission; `Some(None)` clears it.
    pub mission_id: Option<Option<String>>,
    pub workers_active: Option<u32>,
    pub events_total: Option<u64>,
    /// Force a progress timestamp update even if `events_total` is unset.
    pub progressed: bool,
}

/// Owns the live heartbeat for one orchestrator process and the
/// run-scoped metrics the all-day gate consumes.
pub struct EnduranceMonitor<C: Clock = SystemClock> {
    path: PathBuf,
    clock: C,
    cfg: EnduranceConfig,
    // The heartbeat carries all run state — including the cumulative
    // metrics — so it stays consistent across restarts.
    hb: Heartbeat,
}

impl<C: Clock> EnduranceMonitor<C> {
    /// Launch (or resume) the monitor rooted at `root`, persisting the
    /// initial beat immediately so a watchdog observes liveness promptly.
    pub fn launch(root: &Path, clock: C, cfg: EnduranceConfig) -> Result<Self, EnduranceError> {
        let path = heartbeat::heartbeat_path(root);
        let now = clock.now_ms();
        let pid = std::process::id();

        let prior = match heartbeat::read(&path) {
            Ok(p) => p,
            Err(e) => {
                tracing::warn!(
                    target: "vigla::endurance",
                    error = %e,
                    "heartbeat unreadable/corrupt; starting fresh"
                );
                None
            }
        };

        let hb = match prior {
            Some(mut prev) => {
                let gap = now.saturating_sub(prev.last_beat_at_ms);
                prev.restarts = prev.restarts.saturating_add(1);
                // Carry the first-launch service clock forward; backfill
                // it for heartbeats written before the field existed.
                if prev.service_started_at_ms == 0 {
                    prev.service_started_at_ms = prev.process_started_at_ms;
                }
                if gap > cfg.crash_threshold_ms {
                    // The prior process died without a clean stop: count
                    // it as a fault we recovered from by resuming.
                    prev.faults_injected = prev.faults_injected.saturating_add(1);
                    prev.faults_recovered = prev.faults_recovered.saturating_add(1);
                    tracing::info!(
                        target: "vigla::endurance",
                        gap_ms = gap,
                        restarts = prev.restarts,
                        "resumed after unclean shutdown"
                    );
                }
                prev.orchestrator_pid = pid;
                prev.process_started_at_ms = now;
                prev.last_beat_at_ms = now;
                prev
            }
            None => Heartbeat {
                schema_version: HEARTBEAT_SCHEMA_VERSION,
                orchestrator_pid: pid,
                service_started_at_ms: now,
                process_started_at_ms: now,
                last_beat_at_ms: now,
                last_progress_at_ms: now,
                beat_seq: 0,
                restarts: 0,
                mission_id: None,
                phase: "starting".to_string(),
                workers_active: 0,
                events_total: 0,
                faults_injected: 0,
                faults_recovered: 0,
                faults_by_kind: Default::default(),
                max_beat_gap_ms: 0,
                longest_stall_ms: 0,
                progress_events: 0,
            },
        };

        let mut monitor = Self {
            path,
            clock,
            cfg,
            hb,
        };
        monitor.persist()?;
        Ok(monitor)
    }

    /// Emit a beat, applying `status`, updating progress/stall tracking,
    /// and persisting atomically.
    pub fn beat(&mut self, status: BeatStatus) -> Result<(), EnduranceError> {
        let now = self.clock.now_ms();
        let gap = now.saturating_sub(self.hb.last_beat_at_ms);
        self.hb.max_beat_gap_ms = self.hb.max_beat_gap_ms.max(gap);

        let advanced = status
            .events_total
            .map(|e| e > self.hb.events_total)
            .unwrap_or(false);
        let progressed = status.progressed || advanced;

        self.hb.beat_seq = self.hb.beat_seq.saturating_add(1);
        self.hb.last_beat_at_ms = now;
        if let Some(p) = status.phase {
            self.hb.phase = p;
        }
        if let Some(m) = status.mission_id {
            self.hb.mission_id = m;
        }
        if let Some(w) = status.workers_active {
            self.hb.workers_active = w;
        }
        if let Some(e) = status.events_total {
            self.hb.events_total = e;
        }

        if progressed {
            self.hb.last_progress_at_ms = now;
            self.hb.progress_events = self.hb.progress_events.saturating_add(1);
        } else if self.hb.workers_active > 0 {
            // No progress while work is in flight — track stall length.
            let stalled = now.saturating_sub(self.hb.last_progress_at_ms);
            self.hb.longest_stall_ms = self.hb.longest_stall_ms.max(stalled);
        }

        self.persist()
    }

    /// Note that a fault was injected/observed (worker kill, quota
    /// exhaustion, corruption, …). Pair with [`Self::note_recovery`].
    pub fn note_fault(&mut self, kind: &str) -> Result<(), EnduranceError> {
        self.hb.faults_injected = self.hb.faults_injected.saturating_add(1);
        let entry = self.hb.faults_by_kind.entry(kind.to_string()).or_insert(0);
        *entry = entry.saturating_add(1);
        tracing::warn!(target: "vigla::endurance", kind, "fault recorded");
        self.persist()
    }

    /// Note that a previously-injected fault was recovered from.
    pub fn note_recovery(&mut self, kind: &str) -> Result<(), EnduranceError> {
        self.hb.faults_recovered = self.hb.faults_recovered.saturating_add(1);
        tracing::info!(target: "vigla::endurance", kind, "fault recovered");
        self.persist()
    }

    /// Classify current liveness as of the clock's `now`.
    pub fn liveness(&self) -> Liveness {
        stall::classify(self.clock.now_ms(), &self.hb, &self.cfg)
    }

    /// Snapshot the run's endurance metrics for the gate.
    pub fn report(&self) -> EnduranceReport {
        let now = self.clock.now_ms();
        EnduranceReport {
            uptime_ms: now.saturating_sub(self.hb.service_started_at_ms),
            beats_total: self.hb.beat_seq,
            restarts: self.hb.restarts,
            max_beat_gap_ms: self.hb.max_beat_gap_ms,
            longest_stall_ms: self.hb.longest_stall_ms,
            faults_injected: self.hb.faults_injected,
            faults_recovered: self.hb.faults_recovered,
            progress_events: self.hb.progress_events,
            faults_by_kind: self.hb.faults_by_kind.clone(),
        }
    }

    /// Borrow the current heartbeat (for tests / introspection).
    pub fn heartbeat(&self) -> &Heartbeat {
        &self.hb
    }

    fn persist(&mut self) -> Result<(), EnduranceError> {
        heartbeat::write_atomic(&self.path, &self.hb)
    }
}

/// Classify a heartbeat read from disk without a live monitor — the
/// watchdog / CLI path.
pub fn liveness_of(now_ms: u64, hb: &Heartbeat, cfg: &EnduranceConfig) -> Liveness {
    stall::classify(now_ms, hb, cfg)
}

/// Process-wide endurance monitor, installed once by the host at startup
/// and shared across every mission — the orchestrator process's liveness
/// is a per-process resource, not a per-mission one. This mirrors
/// `recovery::quota`'s shared-tracker idiom: the per-mission runtime reads
/// it ([`shared_monitor`]) and injects it into the supervisor loop, while
/// tests and any non-host caller never install it, so missions simply
/// don't beat.
static SHARED_MONITOR: std::sync::OnceLock<std::sync::Arc<std::sync::Mutex<EnduranceMonitor>>> =
    std::sync::OnceLock::new();

/// Launch a process-level monitor rooted at `root` and install it as the
/// process-wide shared monitor. Set-once — the first install wins and
/// later calls are no-ops (so a second call does not even launch). Call
/// exactly once at host startup, after the data directory exists. Returns
/// the launch error so the host can fail soft: missions then run without a
/// heartbeat (the pre-integration behavior), they are not blocked.
pub fn install_process_monitor(root: &Path) -> Result<(), EnduranceError> {
    // Serialize install so the side-effectful `launch` (it mutates the
    // on-disk heartbeat: restarts, fault counters, last-beat) runs at most
    // once even under a concurrent call. `OnceLock::set` alone would let a
    // losing racer launch-then-discard, briefly desyncing the on-disk
    // counters from the installed monitor's in-memory state.
    static INSTALL_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());
    let _guard = INSTALL_LOCK
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    if SHARED_MONITOR.get().is_some() {
        // Already installed — first writer wins; don't launch again.
        return Ok(());
    }
    let monitor = EnduranceMonitor::launch(root, SystemClock, EnduranceConfig::default())?;
    let _ = SHARED_MONITOR.set(std::sync::Arc::new(std::sync::Mutex::new(monitor)));
    Ok(())
}

/// The installed process-wide monitor handle, or `None` if the host never
/// installed one (tests, mock missions, or before startup). Read by the
/// mission runtime to inject into `run_supervisor_mission`.
pub fn shared_monitor() -> Option<std::sync::Arc<std::sync::Mutex<EnduranceMonitor>>> {
    SHARED_MONITOR.get().cloned()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicU64, Ordering};
    use std::sync::Arc;
    use tempfile::TempDir;

    #[derive(Clone)]
    struct ManualClock(Arc<AtomicU64>);
    impl ManualClock {
        fn new(start: u64) -> Self {
            Self(Arc::new(AtomicU64::new(start)))
        }
        fn advance(&self, dt: u64) {
            self.0.fetch_add(dt, Ordering::SeqCst);
        }
    }
    impl Clock for ManualClock {
        fn now_ms(&self) -> u64 {
            self.0.load(Ordering::SeqCst)
        }
    }

    #[test]
    fn fresh_launch_writes_initial_beat() {
        let dir = TempDir::new().unwrap();
        let clock = ManualClock::new(1_000);
        let m = EnduranceMonitor::launch(dir.path(), clock, EnduranceConfig::default()).unwrap();
        assert_eq!(m.heartbeat().beat_seq, 0);
        assert_eq!(m.heartbeat().restarts, 0);
        assert_eq!(m.heartbeat().service_started_at_ms, 1_000);
        // File exists and round-trips.
        let on_disk = read_heartbeat(&heartbeat_path(dir.path()))
            .unwrap()
            .unwrap();
        assert_eq!(on_disk, *m.heartbeat());
    }

    #[test]
    fn resume_continues_beat_seq_and_counts_restart() {
        let dir = TempDir::new().unwrap();
        let clock = ManualClock::new(1_000);

        {
            let mut m =
                EnduranceMonitor::launch(dir.path(), clock.clone(), EnduranceConfig::default())
                    .unwrap();
            clock.advance(30_000);
            m.beat(BeatStatus {
                workers_active: Some(1),
                progressed: true,
                ..Default::default()
            })
            .unwrap();
            assert_eq!(m.heartbeat().beat_seq, 1);
        } // drop = "process exit"

        // Clean restart (within crash threshold): no extra fault.
        clock.advance(30_000);
        let m2 = EnduranceMonitor::launch(dir.path(), clock.clone(), EnduranceConfig::default())
            .unwrap();
        assert_eq!(m2.heartbeat().beat_seq, 1, "beat_seq carries forward");
        assert_eq!(m2.heartbeat().restarts, 1);
        assert_eq!(m2.heartbeat().faults_injected, 0, "clean restart");
        assert_eq!(m2.heartbeat().service_started_at_ms, 1_000);
    }

    #[test]
    fn unclean_restart_is_counted_as_recovered_fault() {
        let dir = TempDir::new().unwrap();
        let clock = ManualClock::new(1_000);
        {
            let _m =
                EnduranceMonitor::launch(dir.path(), clock.clone(), EnduranceConfig::default())
                    .unwrap();
        }
        // Gap exceeds the 90s crash threshold → unclean.
        clock.advance(120_000);
        let m2 = EnduranceMonitor::launch(dir.path(), clock.clone(), EnduranceConfig::default())
            .unwrap();
        assert_eq!(m2.heartbeat().restarts, 1);
        assert_eq!(m2.heartbeat().faults_injected, 1);
        assert_eq!(m2.heartbeat().faults_recovered, 1);
        assert_eq!(m2.report().unrecovered_faults(), 0);
    }

    #[test]
    fn corrupt_prior_heartbeat_starts_fresh_without_panic() {
        let dir = TempDir::new().unwrap();
        let path = heartbeat_path(dir.path());
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(&path, b"not json at all").unwrap();

        let clock = ManualClock::new(5_000);
        let m = EnduranceMonitor::launch(dir.path(), clock, EnduranceConfig::default()).unwrap();
        assert_eq!(m.heartbeat().beat_seq, 0);
        assert_eq!(m.heartbeat().restarts, 0);
    }

    #[test]
    fn beat_advancing_events_counts_as_progress() {
        let dir = TempDir::new().unwrap();
        let clock = ManualClock::new(0);
        let mut m = EnduranceMonitor::launch(dir.path(), clock.clone(), EnduranceConfig::default())
            .unwrap();
        clock.advance(60_000);
        m.beat(BeatStatus {
            workers_active: Some(1),
            events_total: Some(5),
            ..Default::default()
        })
        .unwrap();
        assert_eq!(m.heartbeat().last_progress_at_ms, 60_000);
        assert_eq!(m.report().progress_events, 1);
    }

    #[test]
    fn stall_then_recover_is_visible_in_liveness() {
        let dir = TempDir::new().unwrap();
        let clock = ManualClock::new(0);
        let cfg = EnduranceConfig::default();
        let mut m = EnduranceMonitor::launch(dir.path(), clock.clone(), cfg).unwrap();

        // Work starts, makes progress.
        clock.advance(60_000);
        m.beat(BeatStatus {
            workers_active: Some(1),
            progressed: true,
            ..Default::default()
        })
        .unwrap();
        assert_eq!(m.liveness(), Liveness::Healthy);

        // 11 minutes of beats with no progress while work is in flight.
        for _ in 0..11 {
            clock.advance(60_000);
            m.beat(BeatStatus {
                workers_active: Some(1),
                progressed: false,
                ..Default::default()
            })
            .unwrap();
        }
        assert!(matches!(m.liveness(), Liveness::Stalled { .. }));
        assert!(m.report().longest_stall_ms >= 600_000);

        // Progress resumes → healthy again.
        clock.advance(60_000);
        m.beat(BeatStatus {
            workers_active: Some(1),
            progressed: true,
            ..Default::default()
        })
        .unwrap();
        assert_eq!(m.liveness(), Liveness::Healthy);
    }

    #[test]
    fn note_fault_records_per_kind_breakdown() {
        let dir = TempDir::new().unwrap();
        let clock = ManualClock::new(0);
        let mut m =
            EnduranceMonitor::launch(dir.path(), clock, EnduranceConfig::default()).unwrap();
        m.note_fault("worker_panic").unwrap();
        m.note_fault("recovery").unwrap();
        m.note_fault("recovery").unwrap();
        assert_eq!(m.heartbeat().faults_injected, 3, "aggregate count");
        assert_eq!(m.heartbeat().faults_by_kind.get("recovery"), Some(&2));
        assert_eq!(m.heartbeat().faults_by_kind.get("worker_panic"), Some(&1));
        // The breakdown is surfaced in the report and survives the
        // atomic write (it's a persisted heartbeat field).
        assert_eq!(m.report().faults_by_kind.get("recovery"), Some(&2));
        let on_disk = read_heartbeat(&heartbeat_path(dir.path()))
            .unwrap()
            .unwrap();
        assert_eq!(on_disk.faults_by_kind.get("recovery"), Some(&2));
    }
}
