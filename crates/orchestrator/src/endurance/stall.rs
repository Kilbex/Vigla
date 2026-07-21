//! Liveness classification — the piece that lets an unattended run
//! distinguish "quietly waiting" from "wedged" from "dead".
//!
//! Pure function over `(now, Heartbeat, EnduranceConfig)`. No IO, no
//! clock of its own, so it is trivially testable and reusable by a
//! watchdog that only has the on-disk heartbeat.

use super::heartbeat::Heartbeat;
use super::EnduranceConfig;

/// What the most recent heartbeat says about the orchestrator's health.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Liveness {
    /// Beating, and either progressing or legitimately between beats,
    /// within thresholds. Nothing to do.
    Healthy,
    /// Beating but not progressing, with **no work in flight** — a quota
    /// pause or a wait-on-user. Expected; not an alert.
    Idle { idle_ms: u64 },
    /// Beating, work **is** in flight, but progress has not advanced past
    /// the stall threshold. The "wedged worker" signal.
    Stalled { stalled_ms: u64 },
    /// No beat within the crash threshold — the process is presumed dead
    /// or hung at its event loop. The strongest signal; a watchdog acts
    /// (restart, notify).
    Crashed { since_last_beat_ms: u64 },
}

impl Liveness {
    /// True for states an async operator should be pinged about.
    pub fn needs_attention(&self) -> bool {
        matches!(self, Liveness::Stalled { .. } | Liveness::Crashed { .. })
    }

    /// Stable lowercase label for logs / CLI / telemetry.
    pub fn label(&self) -> &'static str {
        match self {
            Liveness::Healthy => "healthy",
            Liveness::Idle { .. } => "idle",
            Liveness::Stalled { .. } => "stalled",
            Liveness::Crashed { .. } => "crashed",
        }
    }
}

/// Classify a heartbeat as of `now_ms`.
///
/// Order matters: a missing beat (Crashed) dominates everything, because
/// if the process is gone the progress timestamp is meaningless. Only
/// once we know it is still beating do we look at progress, and only
/// when work is actually in flight does a progress gap mean a stall —
/// otherwise it is benign idle time.
pub fn classify(now_ms: u64, hb: &Heartbeat, cfg: &EnduranceConfig) -> Liveness {
    let beat_gap = now_ms.saturating_sub(hb.last_beat_at_ms);
    if beat_gap > cfg.crash_threshold_ms {
        return Liveness::Crashed {
            since_last_beat_ms: beat_gap,
        };
    }

    let progress_gap = now_ms.saturating_sub(hb.last_progress_at_ms);

    if hb.workers_active == 0 {
        // No work in flight: quiet is fine until it exceeds the idle
        // grace, at which point we surface it as (benign) Idle.
        if progress_gap > cfg.idle_grace_ms {
            return Liveness::Idle {
                idle_ms: progress_gap,
            };
        }
        return Liveness::Healthy;
    }

    if progress_gap > cfg.stall_threshold_ms {
        return Liveness::Stalled {
            stalled_ms: progress_gap,
        };
    }

    Liveness::Healthy
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::endurance::heartbeat::HEARTBEAT_SCHEMA_VERSION;

    fn hb(last_beat: u64, last_progress: u64, workers: u32) -> Heartbeat {
        Heartbeat {
            schema_version: HEARTBEAT_SCHEMA_VERSION,
            orchestrator_pid: 1,
            service_started_at_ms: 0,
            process_started_at_ms: 0,
            last_beat_at_ms: last_beat,
            last_progress_at_ms: last_progress,
            beat_seq: 1,
            restarts: 0,
            mission_id: None,
            phase: String::new(),
            workers_active: workers,
            events_total: 0,
            faults_injected: 0,
            faults_recovered: 0,
            faults_by_kind: std::collections::BTreeMap::new(),
            max_beat_gap_ms: 0,
            longest_stall_ms: 0,
            progress_events: 0,
        }
    }

    fn cfg() -> EnduranceConfig {
        EnduranceConfig {
            crash_threshold_ms: 90_000,
            stall_threshold_ms: 600_000,
            idle_grace_ms: 300_000,
        }
    }

    #[test]
    fn healthy_when_recently_beating_and_progressing() {
        let h = hb(100_000, 100_000, 1);
        assert_eq!(classify(110_000, &h, &cfg()), Liveness::Healthy);
    }

    #[test]
    fn crashed_dominates_even_with_recent_progress_timestamp() {
        // Beat is ancient but progress ts is "recent" — crash still wins,
        // because a dead process can't have made that progress.
        let h = hb(0, 100_000, 1);
        let now = 200_000; // beat_gap = 200s > 90s
        match classify(now, &h, &cfg()) {
            Liveness::Crashed { since_last_beat_ms } => {
                assert_eq!(since_last_beat_ms, 200_000)
            }
            other => panic!("expected Crashed, got {other:?}"),
        }
    }

    #[test]
    fn stalled_only_when_work_in_flight() {
        // 11 min since progress, beating fine, workers active → stalled.
        let h = hb(660_000, 0, 1);
        match classify(660_000, &h, &cfg()) {
            Liveness::Stalled { stalled_ms } => assert_eq!(stalled_ms, 660_000),
            other => panic!("expected Stalled, got {other:?}"),
        }
    }

    #[test]
    fn idle_not_stalled_when_no_work_in_flight() {
        // Same long progress gap, but zero workers → benign Idle.
        let h = hb(660_000, 0, 0);
        match classify(660_000, &h, &cfg()) {
            Liveness::Idle { idle_ms } => assert_eq!(idle_ms, 660_000),
            other => panic!("expected Idle, got {other:?}"),
        }
    }

    #[test]
    fn quiet_within_grace_is_healthy_not_idle() {
        // No workers, only 2 min since progress (< 5 min grace).
        let h = hb(120_000, 0, 0);
        assert_eq!(classify(120_000, &h, &cfg()), Liveness::Healthy);
    }

    #[test]
    fn boundaries_are_strict_greater_than() {
        // Exactly at the crash threshold is not yet crashed.
        let h = hb(0, 0, 0);
        assert_eq!(classify(90_000, &h, &cfg()), Liveness::Healthy);
        assert!(matches!(
            classify(90_001, &h, &cfg()),
            Liveness::Crashed { .. }
        ));
    }

    #[test]
    fn needs_attention_flags() {
        assert!(!Liveness::Healthy.needs_attention());
        assert!(!Liveness::Idle { idle_ms: 1 }.needs_attention());
        assert!(Liveness::Stalled { stalled_ms: 1 }.needs_attention());
        assert!(Liveness::Crashed {
            since_last_beat_ms: 1
        }
        .needs_attention());
    }
}
