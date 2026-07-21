//! Endurance metrics and the gate that substantiates the "all-day"
//! claim.
//!
//! The roadmap asks for "endurance metrics gating the all-day claim."
//! This is that gate, as data: an [`EnduranceReport`] aggregated by the
//! monitor over a run, and an [`EnduranceGate`] of targets it must meet.
//! `report.evaluate(&gate)` returns a pass/fail with human-readable
//! reasons, so a CI job or the `simulate` CLI can assert "yes, this
//! actually held for a day" instead of asserting hope.

use serde::{Deserialize, Serialize};

/// Aggregated endurance numbers for a run (possibly spanning restarts).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct EnduranceReport {
    /// Cumulative service time since the first launch (across restarts).
    pub uptime_ms: u64,
    /// Total beats emitted (monotonic across restarts).
    pub beats_total: u64,
    /// How many times the monitor resumed from a prior heartbeat.
    pub restarts: u32,
    /// Longest gap between two consecutive beats *within a single process
    /// lifetime*. The restart gap is excluded — it is accounted as a
    /// recovered fault, not an unexplained hang.
    pub max_beat_gap_ms: u64,
    /// Longest observed stretch of no-progress while work was in flight.
    pub longest_stall_ms: u64,
    pub faults_injected: u64,
    pub faults_recovered: u64,
    /// Count of recorded forward-progress events.
    pub progress_events: u64,
    /// Per-kind breakdown of injected faults, mirrored from the heartbeat
    /// for forensics. Not gated — purely informational.
    pub faults_by_kind: std::collections::BTreeMap<String, u64>,
}

impl EnduranceReport {
    /// Faults that were injected but never recovered. The headline
    /// correctness signal: an all-day run is only a success if it
    /// recovered from everything thrown at it.
    pub fn unrecovered_faults(&self) -> u64 {
        self.faults_injected.saturating_sub(self.faults_recovered)
    }
}

/// Targets that define "ran all day, unattended, and stayed correct."
#[derive(Debug, Clone, Copy)]
pub struct EnduranceGate {
    pub min_uptime_ms: u64,
    pub max_beat_gap_ms: u64,
    pub max_unrecovered_faults: u64,
    /// Max restarts tolerated for a *bounded* all-day evaluation (the soak
    /// / `simulate` CLI). Guards against a crash-loop the fault counters
    /// miss — every unclean restart self-recovers, so `unrecovered_faults`
    /// alone never catches looping. A long-lived host monitor accumulates
    /// restarts across launches, so this criterion is for bounded runs,
    /// not the persistent process monitor.
    pub max_restarts: u32,
}

impl EnduranceGate {
    /// The headline gate: a full day of cumulative uptime, no in-process
    /// beat gap longer than the crash threshold (the watchdog should
    /// never have had cause to fire without a recorded restart), and
    /// every injected fault recovered.
    pub fn all_day() -> Self {
        Self {
            min_uptime_ms: 24 * 60 * 60 * 1000,
            max_beat_gap_ms: 90_000,
            max_unrecovered_faults: 0,
            // Generous: a clean bounded run restarts a handful of times at
            // most; 25+ in one evaluation window is a crash-loop.
            max_restarts: 24,
        }
    }
}

/// Result of checking a report against a gate.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GateOutcome {
    pub passed: bool,
    /// One line per failed criterion; empty when `passed`.
    pub reasons: Vec<String>,
}

impl EnduranceReport {
    /// Check this report against `gate`, collecting a reason for every
    /// criterion that failed.
    pub fn evaluate(&self, gate: &EnduranceGate) -> GateOutcome {
        let mut reasons = Vec::new();
        if self.uptime_ms < gate.min_uptime_ms {
            reasons.push(format!(
                "uptime {}ms < required {}ms",
                self.uptime_ms, gate.min_uptime_ms
            ));
        }
        if self.max_beat_gap_ms > gate.max_beat_gap_ms {
            reasons.push(format!(
                "max beat gap {}ms > allowed {}ms",
                self.max_beat_gap_ms, gate.max_beat_gap_ms
            ));
        }
        if self.unrecovered_faults() > gate.max_unrecovered_faults {
            reasons.push(format!(
                "{} unrecovered fault(s) > allowed {}",
                self.unrecovered_faults(),
                gate.max_unrecovered_faults
            ));
        }
        if self.restarts > gate.max_restarts {
            reasons.push(format!(
                "{} restart(s) > allowed {}",
                self.restarts, gate.max_restarts
            ));
        }
        GateOutcome {
            passed: reasons.is_empty(),
            reasons,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn passing() -> EnduranceReport {
        EnduranceReport {
            uptime_ms: 24 * 60 * 60 * 1000 + 5,
            beats_total: 1500,
            restarts: 1,
            max_beat_gap_ms: 60_000,
            longest_stall_ms: 120_000,
            faults_injected: 4,
            faults_recovered: 4,
            progress_events: 900,
            faults_by_kind: std::collections::BTreeMap::new(),
        }
    }

    #[test]
    fn clean_all_day_run_passes() {
        let out = passing().evaluate(&EnduranceGate::all_day());
        assert!(out.passed, "reasons: {:?}", out.reasons);
        assert!(out.reasons.is_empty());
    }

    #[test]
    fn short_run_fails_on_uptime() {
        let mut r = passing();
        r.uptime_ms = 60_000;
        let out = r.evaluate(&EnduranceGate::all_day());
        assert!(!out.passed);
        assert!(out.reasons.iter().any(|s| s.contains("uptime")));
    }

    #[test]
    fn long_beat_gap_fails() {
        let mut r = passing();
        r.max_beat_gap_ms = 120_000;
        let out = r.evaluate(&EnduranceGate::all_day());
        assert!(!out.passed);
        assert!(out.reasons.iter().any(|s| s.contains("beat gap")));
    }

    #[test]
    fn unrecovered_fault_fails() {
        let mut r = passing();
        r.faults_recovered = 3; // one short
        assert_eq!(r.unrecovered_faults(), 1);
        let out = r.evaluate(&EnduranceGate::all_day());
        assert!(!out.passed);
        assert!(out.reasons.iter().any(|s| s.contains("unrecovered")));
    }

    #[test]
    fn excessive_restarts_fail() {
        let mut r = passing();
        r.restarts = 25; // > all_day()'s max_restarts (24) ⇒ crash-loop
        let out = r.evaluate(&EnduranceGate::all_day());
        assert!(!out.passed);
        assert!(out.reasons.iter().any(|s| s.contains("restart")));
    }

    #[test]
    fn multiple_failures_all_reported() {
        let mut r = passing();
        r.uptime_ms = 1;
        r.max_beat_gap_ms = 999_999;
        r.faults_recovered = 0;
        let out = r.evaluate(&EnduranceGate::all_day());
        assert!(!out.passed);
        assert_eq!(out.reasons.len(), 3);
    }
}
