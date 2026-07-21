//! Day-scale endurance harness (U10 Pillar A, E-A3).
//!
//! Drives the [`EnduranceMonitor`] through a deterministic, time-compressed
//! 24-hour run on an injectable clock — a full simulated day in
//! milliseconds of wall-clock — while injecting the failures an all-day
//! fleet actually meets:
//!
//!   * a **wedged worker** (work in flight, progress halts) — must read Stalled;
//!   * a **quota pause** (no work in flight, long quiet) — must read Idle, NOT Stalled;
//!   * a **crash + restart** (process dies, relaunches from disk) — beat_seq
//!     must carry forward and the unclean restart must be booked as a
//!     recovered fault;
//!   * **heartbeat corruption** — relaunch must start fresh, never panic.
//!
//! The run only "passes" if the endurance gate
//! ([`EnduranceGate::all_day`]) holds at the end — that gate is what
//! substantiates the "all-day" claim.

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;

use orchestrator::endurance::{
    BeatStatus, Clock, EnduranceConfig, EnduranceGate, EnduranceMonitor, Liveness,
};
use tempfile::TempDir;

/// Shared, advanceable clock so a simulated day runs instantly and
/// deterministically.
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

const MIN: u64 = 60_000;
const HOUR: u64 = 60 * MIN;

#[test]
fn soak_24h_with_injected_faults_passes_all_day_gate() {
    let dir = TempDir::new().unwrap();
    let clock = ManualClock::new(0);
    let cfg = EnduranceConfig::default();

    let beat = MIN; // beat once per simulated minute
    let total = 24 * HOUR;
    let stall_start = 6 * HOUR;
    let stall_len = 12 * MIN; // exceeds the 10-min stall threshold
    let idle_start = 12 * HOUR;
    let idle_len = 20 * MIN; // exceeds the 5-min idle grace
    let crash_at = 18 * HOUR;

    let mut m = EnduranceMonitor::launch(dir.path(), clock.clone(), cfg).unwrap();

    let mut saw_stalled = false;
    let mut saw_idle = false;
    let mut crashed = false;

    let mut t = 0u64;
    while t < total {
        clock.advance(beat);
        t += beat;

        let in_stall = t >= stall_start && t < stall_start + stall_len;
        let in_idle = t >= idle_start && t < idle_start + idle_len;
        let workers = if in_idle { 0 } else { 1 };
        let progressed = !in_stall && !in_idle;

        m.beat(BeatStatus {
            phase: Some(
                if in_idle {
                    "paused:quota"
                } else if in_stall {
                    "executing:stalled"
                } else {
                    "executing"
                }
                .to_string(),
            ),
            workers_active: Some(workers),
            progressed,
            ..Default::default()
        })
        .unwrap();

        // Near the end of each window, the classifier should have made up
        // its mind. These are the load-bearing assertions: a wedged worker
        // is Stalled, but a quota pause is benign Idle — never confused.
        if in_stall && t == stall_start + stall_len - beat {
            assert!(
                matches!(m.liveness(), Liveness::Stalled { .. }),
                "wedged worker should read Stalled, got {:?}",
                m.liveness()
            );
            saw_stalled = true;
        }
        if in_idle && t == idle_start + idle_len - beat {
            assert!(
                matches!(m.liveness(), Liveness::Idle { .. }),
                "quota pause should read Idle, got {:?}",
                m.liveness()
            );
            saw_idle = true;
        }

        if t == stall_start {
            m.note_fault("worker_stall").unwrap();
        }
        if t == stall_start + stall_len {
            m.note_recovery("worker_stall").unwrap();
        }

        // Crash: drop the live monitor and relaunch from the persisted
        // heartbeat after a downtime longer than the crash threshold.
        if !crashed && t == crash_at {
            crashed = true;
            let beat_seq_before = m.heartbeat().beat_seq;
            drop(m);
            clock.advance(2 * MIN); // > 90s crash threshold
            t += 2 * MIN;
            m = EnduranceMonitor::launch(dir.path(), clock.clone(), cfg).unwrap();
            assert_eq!(
                m.heartbeat().beat_seq,
                beat_seq_before,
                "beat_seq must carry across the restart"
            );
            assert_eq!(m.heartbeat().restarts, 1);
            assert_eq!(
                m.heartbeat().faults_injected,
                2,
                "stall fault + unclean restart"
            );
            assert_eq!(m.heartbeat().faults_recovered, 2);
        }
    }

    assert!(saw_stalled, "never classified the wedged worker as Stalled");
    assert!(saw_idle, "never classified the quota pause as Idle");

    let report = m.report();
    assert!(
        report.uptime_ms >= total,
        "uptime {} < {}",
        report.uptime_ms,
        total
    );
    assert_eq!(report.restarts, 1);
    assert_eq!(report.unrecovered_faults(), 0);

    let outcome = report.evaluate(&EnduranceGate::all_day());
    assert!(outcome.passed, "all-day gate failed: {:?}", outcome.reasons);
}

#[test]
fn quota_idle_is_never_misread_as_a_stall() {
    // Focused regression: the single most important distinction in the
    // subsystem. Same long no-progress gap, the only difference is whether
    // work is in flight.
    let dir = TempDir::new().unwrap();
    let clock = ManualClock::new(0);
    let cfg = EnduranceConfig::default();
    let mut m = EnduranceMonitor::launch(dir.path(), clock.clone(), cfg).unwrap();

    // Establish progress, then go quiet for 30 minutes with NO workers.
    clock.advance(MIN);
    m.beat(BeatStatus {
        workers_active: Some(1),
        progressed: true,
        ..Default::default()
    })
    .unwrap();
    for _ in 0..30 {
        clock.advance(MIN);
        m.beat(BeatStatus {
            workers_active: Some(0),
            progressed: false,
            ..Default::default()
        })
        .unwrap();
    }
    assert!(
        matches!(m.liveness(), Liveness::Idle { .. }),
        "no work in flight ⇒ Idle, got {:?}",
        m.liveness()
    );
}

#[test]
fn heartbeat_corruption_midflight_recovers_to_fresh_without_panic() {
    let dir = TempDir::new().unwrap();
    let clock = ManualClock::new(1_000);
    let cfg = EnduranceConfig::default();

    {
        let mut m = EnduranceMonitor::launch(dir.path(), clock.clone(), cfg).unwrap();
        clock.advance(MIN);
        m.beat(BeatStatus {
            workers_active: Some(1),
            progressed: true,
            ..Default::default()
        })
        .unwrap();
        assert_eq!(m.heartbeat().beat_seq, 1);
    }

    // Simulate disk/file corruption of the heartbeat.
    let path = orchestrator::endurance::heartbeat_path(dir.path());
    std::fs::write(&path, b"\x00\x00 garbage not json \xff").unwrap();

    // Relaunch must not panic and must start fresh.
    clock.advance(MIN);
    let m2 = EnduranceMonitor::launch(dir.path(), clock.clone(), cfg).unwrap();
    assert_eq!(m2.heartbeat().beat_seq, 0, "corrupt prior ⇒ fresh start");
    assert_eq!(m2.heartbeat().restarts, 0);
}
