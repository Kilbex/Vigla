//! Endurance CLI — read a live heartbeat or run a compressed soak.
//!
//! Usage:
//!   orchestrator_endurance status --root <path>
//!   orchestrator_endurance simulate [--hours N]
//!
//! `status` classifies the heartbeat under `<path>/.vigla/endurance/`
//! and encodes liveness in the exit code (0 ok, 1 stalled, 2 crashed,
//! 3 absent) so a shell watchdog can branch on it.
//!
//! `simulate` runs a deterministic, time-compressed endurance run (a full
//! day by default) that injects a worker stall, a quota idle, and a
//! crash+restart, then prints the endurance report and the all-day gate
//! verdict. It is the credential-free way to see the subsystem work.

use std::path::PathBuf;
use std::process::ExitCode;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;

use orchestrator::endurance::{
    self, BeatStatus, Clock, EnduranceConfig, EnduranceGate, EnduranceMonitor,
};

/// Shared, advanceable clock for the simulation.
#[derive(Clone)]
struct SimClock(Arc<AtomicU64>);
impl SimClock {
    fn new(start: u64) -> Self {
        Self(Arc::new(AtomicU64::new(start)))
    }
    fn advance(&self, dt: u64) {
        self.0.fetch_add(dt, Ordering::SeqCst);
    }
}
impl Clock for SimClock {
    fn now_ms(&self) -> u64 {
        self.0.load(Ordering::SeqCst)
    }
}

fn print_usage() {
    eprintln!(
        "Usage:\n  \
         orchestrator_endurance status --root <path>\n  \
         orchestrator_endurance simulate [--hours N]"
    );
}

fn flag_value(args: &[String], name: &str) -> Option<String> {
    args.iter()
        .position(|a| a == name)
        .and_then(|i| args.get(i + 1))
        .cloned()
}

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    match args.first().map(String::as_str) {
        Some("status") => cmd_status(&args[1..]),
        Some("simulate") => cmd_simulate(&args[1..]),
        _ => {
            print_usage();
            ExitCode::from(64) // EX_USAGE
        }
    }
}

fn cmd_status(args: &[String]) -> ExitCode {
    let Some(root) = flag_value(args, "--root").map(PathBuf::from) else {
        eprintln!("status requires --root <path>");
        return ExitCode::from(64);
    };
    let cfg = EnduranceConfig::default();
    let path = endurance::heartbeat_path(&root);
    match endurance::read_heartbeat(&path) {
        Ok(Some(hb)) => {
            let now = endurance::SystemClock.now_ms();
            let liveness = endurance::liveness_of(now, &hb, &cfg);
            println!("liveness:    {}", liveness.label());
            println!("beat_seq:    {}", hb.beat_seq);
            println!("restarts:    {}", hb.restarts);
            println!("pid:         {}", hb.orchestrator_pid);
            println!("phase:       {}", hb.phase);
            println!("workers:     {}", hb.workers_active);
            println!("beat_age_ms: {}", now.saturating_sub(hb.last_beat_at_ms));
            println!(
                "prog_age_ms: {}",
                now.saturating_sub(hb.last_progress_at_ms)
            );
            println!(
                "faults:      {}/{} recovered",
                hb.faults_recovered, hb.faults_injected
            );
            match liveness {
                endurance::Liveness::Healthy | endurance::Liveness::Idle { .. } => {
                    ExitCode::from(0)
                }
                endurance::Liveness::Stalled { .. } => ExitCode::from(1),
                endurance::Liveness::Crashed { .. } => ExitCode::from(2),
            }
        }
        Ok(None) => {
            println!("liveness:    absent (no heartbeat at {})", path.display());
            ExitCode::from(3)
        }
        Err(e) => {
            eprintln!("error reading heartbeat: {e}");
            ExitCode::from(74) // EX_IOERR
        }
    }
}

fn cmd_simulate(args: &[String]) -> ExitCode {
    let hours: u64 = flag_value(args, "--hours")
        .and_then(|s| s.parse().ok())
        .unwrap_or(24);

    // Scratch dir under the OS temp; cleaned up best-effort at the end.
    let root = std::env::temp_dir().join(format!(
        "vigla-endurance-sim-{}",
        uuid::Uuid::now_v7().simple()
    ));
    let clock = SimClock::new(0);
    let cfg = EnduranceConfig::default();

    let beat_interval_ms = 60_000; // beat once a simulated minute
    let total_ms = hours * 60 * 60 * 1000;
    let stall_at = total_ms / 4; // a wedged worker at ~1/4 through
    let idle_at = total_ms / 2; // a quota pause at the halfway mark
    let crash_at = (total_ms * 3) / 4; // a crash+restart at ~3/4

    let mut m = match EnduranceMonitor::launch(&root, clock.clone(), cfg) {
        Ok(m) => m,
        Err(e) => {
            eprintln!("launch failed: {e}");
            return ExitCode::from(74);
        }
    };

    let mut crashed_once = false;
    let mut t = 0u64;
    while t < total_ms {
        clock.advance(beat_interval_ms);
        t += beat_interval_ms;

        // Phase shaping around the injected faults.
        let in_stall = t >= stall_at && t < stall_at + 12 * 60_000; // 12 min wedge
        let in_idle = t >= idle_at && t < idle_at + 20 * 60_000; // 20 min quota pause

        let workers = if in_idle { 0 } else { 1 };
        let progressed = !in_stall && !in_idle;
        let phase = if in_idle {
            "paused:quota"
        } else if in_stall {
            "executing:stalled"
        } else {
            "executing"
        };

        if let Err(e) = m.beat(BeatStatus {
            phase: Some(phase.to_string()),
            workers_active: Some(workers),
            progressed,
            ..Default::default()
        }) {
            eprintln!("beat failed: {e}");
            return ExitCode::from(74);
        }

        // Record the fault/recovery transitions exactly once each.
        if t == stall_at {
            let _ = m.note_fault("worker_stall");
        }
        if t == stall_at + 12 * 60_000 {
            let _ = m.note_recovery("worker_stall");
        }

        // Crash + restart: drop the monitor and relaunch from disk.
        if !crashed_once && t >= crash_at {
            crashed_once = true;
            drop(m);
            clock.advance(120_000); // 2 min of downtime (> crash threshold)
            t += 120_000;
            m = match EnduranceMonitor::launch(&root, clock.clone(), cfg) {
                Ok(m) => m,
                Err(e) => {
                    eprintln!("relaunch failed: {e}");
                    return ExitCode::from(74);
                }
            };
        }
    }

    let report = m.report();
    let gate = EnduranceGate {
        min_uptime_ms: total_ms,
        max_beat_gap_ms: 90_000,
        max_unrecovered_faults: 0,
        max_restarts: EnduranceGate::all_day().max_restarts,
    };
    let outcome = report.evaluate(&gate);

    println!("── endurance simulation ({hours}h compressed) ──");
    println!("uptime_ms:        {}", report.uptime_ms);
    println!("beats_total:      {}", report.beats_total);
    println!("restarts:         {}", report.restarts);
    println!("max_beat_gap_ms:  {}", report.max_beat_gap_ms);
    println!("longest_stall_ms: {}", report.longest_stall_ms);
    println!(
        "faults:           {}/{} recovered",
        report.faults_recovered, report.faults_injected
    );
    if !report.faults_by_kind.is_empty() {
        println!("faults_by_kind:   {:?}", report.faults_by_kind);
    }
    println!("progress_events:  {}", report.progress_events);
    println!(
        "all-day gate:     {}",
        if outcome.passed { "PASS" } else { "FAIL" }
    );
    for reason in &outcome.reasons {
        println!("  - {reason}");
    }

    let _ = std::fs::remove_dir_all(&root);

    if outcome.passed {
        ExitCode::SUCCESS
    } else {
        ExitCode::FAILURE
    }
}
