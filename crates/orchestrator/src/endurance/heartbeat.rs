//! Durable liveness heartbeat for unattended ("all-day") operation.
//!
//! [`crate::health_check`] reports *in-process* uptime via an `Instant`;
//! it tells you nothing after the process dies. The endurance heartbeat
//! is the crash-surviving complement: one small JSON file written
//! atomically (same-dir tmp + rename, mirroring
//! `memory::store::write_atomic`) on every supervisor beat. A separate
//! watchdog, the `orchestrator_endurance` CLI, or the next orchestrator
//! launch can read it to answer the three questions an all-day operator
//! actually has when they walk back to the machine:
//!
//!   * Is the orchestrator still alive, or did it wedge/crash? (beat age)
//!   * Is the active mission making progress, or stalled?      (progress age)
//!   * Did it survive its faults across the day?               (counters)
//!
//! Forward/backward compatible (WS-E spirit): unknown fields are ignored
//! on read and missing newer fields fall back to `Default`, so an older
//! build can inspect a newer heartbeat and vice-versa.

use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::io::Write;
use std::path::{Path, PathBuf};

use super::EnduranceError;

/// On-disk schema version for the heartbeat record. Bumped only on a
/// breaking field change; readers tolerate older/newer files via serde
/// defaults plus ignored-unknown-fields.
pub const HEARTBEAT_SCHEMA_VERSION: u32 = 1;

/// File name under `<root>/.vigla/endurance/`.
pub const HEARTBEAT_FILE: &str = "heartbeat.json";

/// The durable record. All timestamps are unix-epoch milliseconds, to
/// match the orchestrator's existing `*_at_ms` convention
/// (`recovery::quota`, `mission_supervisor_run`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Heartbeat {
    #[serde(default = "default_schema_version")]
    pub schema_version: u32,
    /// PID of the process that wrote this beat. Lets a watchdog cross-check
    /// liveness against the OS when the beat age is ambiguous.
    pub orchestrator_pid: u32,
    /// When the *first ever* launch happened (carried across restarts).
    /// This is the cumulative-service clock the all-day gate measures.
    #[serde(default)]
    pub service_started_at_ms: u64,
    /// When the *current* process started.
    pub process_started_at_ms: u64,
    /// Wall-clock of the most recent beat. Beat age = now − this.
    pub last_beat_at_ms: u64,
    /// Wall-clock of the most recent forward progress (worker submit,
    /// integration, completed turn). Progress age = now − this.
    pub last_progress_at_ms: u64,
    /// Monotonic beat counter, continued across restarts.
    pub beat_seq: u64,
    /// Number of times the monitor resumed from a pre-existing heartbeat.
    #[serde(default)]
    pub restarts: u32,
    #[serde(default)]
    pub mission_id: Option<String>,
    /// Free-form phase label, e.g. `executing`, `paused:quota`, `reviewing`.
    #[serde(default)]
    pub phase: String,
    #[serde(default)]
    pub workers_active: u32,
    #[serde(default)]
    pub events_total: u64,
    #[serde(default)]
    pub faults_injected: u64,
    #[serde(default)]
    pub faults_recovered: u64,
    /// Per-kind breakdown of injected faults (e.g. `worker_panic`,
    /// `task_error`, `recovery`) for post-hoc forensics. Sums to
    /// `faults_injected` in aggregate. Persisted so the breakdown survives
    /// restarts like every other counter.
    #[serde(default)]
    pub faults_by_kind: BTreeMap<String, u64>,
    // ── Cumulative run metrics (persisted so they survive restarts; a
    // gate must not be able to reset its worst-case numbers by crashing).
    /// Longest gap between consecutive beats within any single process
    /// lifetime. The restart gap is excluded — booked as a fault instead.
    #[serde(default)]
    pub max_beat_gap_ms: u64,
    /// Longest stretch of no-progress while work was in flight.
    #[serde(default)]
    pub longest_stall_ms: u64,
    /// Count of recorded forward-progress events.
    #[serde(default)]
    pub progress_events: u64,
}

fn default_schema_version() -> u32 {
    HEARTBEAT_SCHEMA_VERSION
}

/// Directory holding the heartbeat, mirroring the `memory` kernel's
/// `<repo>/.vigla/...` layout.
pub fn heartbeat_dir(root: &Path) -> PathBuf {
    root.join(".vigla").join("endurance")
}

/// Full path to the heartbeat file under `root`.
pub fn heartbeat_path(root: &Path) -> PathBuf {
    heartbeat_dir(root).join(HEARTBEAT_FILE)
}

/// Read the heartbeat at `path`.
///
/// `Ok(None)` if the file is absent (first launch). `Err` if it exists
/// but cannot be read or parsed — the monitor treats that as corruption
/// and starts fresh rather than crashing (see
/// [`super::EnduranceMonitor::launch`]).
pub fn read(path: &Path) -> Result<Option<Heartbeat>, EnduranceError> {
    match std::fs::read(path) {
        Ok(bytes) => Ok(Some(serde_json::from_slice(&bytes)?)),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(e) => Err(EnduranceError::Io(e)),
    }
}

/// Atomically persist `hb` to `path` via same-dir tmp + rename, so a
/// concurrent reader (watchdog/CLI) never observes a partial write — it
/// sees either the old complete file or the new complete file.
pub fn write_atomic(path: &Path, hb: &Heartbeat) -> Result<(), EnduranceError> {
    let parent = path.parent().ok_or_else(|| {
        EnduranceError::Io(std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            "heartbeat path has no parent dir",
        ))
    })?;
    std::fs::create_dir_all(parent)?;

    let bytes = serde_json::to_vec_pretty(hb)?;
    let tmp_name = format!(
        ".{}.tmp.{}",
        path.file_name()
            .and_then(|s| s.to_str())
            .unwrap_or(HEARTBEAT_FILE),
        uuid::Uuid::now_v7().simple()
    );
    let tmp_path = parent.join(tmp_name);

    {
        let mut f = std::fs::File::create(&tmp_path)?;
        f.write_all(&bytes)?;
        f.flush()?;
        f.sync_all()?;
    }

    match std::fs::rename(&tmp_path, path) {
        Ok(()) => Ok(()),
        Err(e) => {
            let _ = std::fs::remove_file(&tmp_path);
            Err(EnduranceError::Io(e))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    fn sample() -> Heartbeat {
        Heartbeat {
            schema_version: HEARTBEAT_SCHEMA_VERSION,
            orchestrator_pid: 4242,
            service_started_at_ms: 1_000,
            process_started_at_ms: 1_000,
            last_beat_at_ms: 5_000,
            last_progress_at_ms: 4_000,
            beat_seq: 7,
            restarts: 1,
            mission_id: Some("mission-abc".into()),
            phase: "executing".into(),
            workers_active: 2,
            events_total: 128,
            faults_injected: 3,
            faults_recovered: 3,
            faults_by_kind: BTreeMap::from([
                (String::from("recovery"), 2),
                (String::from("worker_panic"), 1),
            ]),
            max_beat_gap_ms: 60_000,
            longest_stall_ms: 120_000,
            progress_events: 99,
        }
    }

    #[test]
    fn roundtrip_through_atomic_write() {
        let dir = TempDir::new().unwrap();
        let path = heartbeat_path(dir.path());
        let hb = sample();
        write_atomic(&path, &hb).unwrap();
        let back = read(&path).unwrap().expect("present");
        assert_eq!(back, hb);
    }

    #[test]
    fn missing_file_reads_as_none() {
        let dir = TempDir::new().unwrap();
        let path = heartbeat_path(dir.path());
        assert!(read(&path).unwrap().is_none());
    }

    #[test]
    fn corrupt_file_is_an_error_not_a_panic() {
        let dir = TempDir::new().unwrap();
        let path = heartbeat_path(dir.path());
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(&path, b"{ this is not valid json").unwrap();
        assert!(read(&path).is_err());
    }

    #[test]
    fn forward_compatible_ignores_unknown_fields() {
        // A newer build wrote a field this build doesn't know about.
        let dir = TempDir::new().unwrap();
        let path = heartbeat_path(dir.path());
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        let json = r#"{
            "schema_version": 99,
            "orchestrator_pid": 1,
            "service_started_at_ms": 1,
            "process_started_at_ms": 1,
            "last_beat_at_ms": 2,
            "last_progress_at_ms": 2,
            "beat_seq": 1,
            "future_field_we_do_not_know": {"nested": true}
        }"#;
        std::fs::write(&path, json).unwrap();
        let hb = read(&path).unwrap().expect("parses despite unknown field");
        assert_eq!(hb.beat_seq, 1);
        assert_eq!(hb.schema_version, 99);
    }

    #[test]
    fn backward_compatible_defaults_missing_newer_fields() {
        // An older build wrote a record lacking the fault counters and
        // restart/service fields this build added.
        let dir = TempDir::new().unwrap();
        let path = heartbeat_path(dir.path());
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        let json = r#"{
            "orchestrator_pid": 1,
            "process_started_at_ms": 10,
            "last_beat_at_ms": 20,
            "last_progress_at_ms": 15,
            "beat_seq": 3
        }"#;
        std::fs::write(&path, json).unwrap();
        let hb = read(&path).unwrap().expect("parses with defaults");
        assert_eq!(hb.beat_seq, 3);
        assert_eq!(hb.faults_injected, 0);
        assert_eq!(hb.restarts, 0);
        assert_eq!(hb.service_started_at_ms, 0);
        assert_eq!(hb.schema_version, HEARTBEAT_SCHEMA_VERSION);
    }
}
