//! Golden-transcript conformance harness for Vigla vendor adapters.
//!
//! A *transcript* is a recorded sequence of CLI output lines (each tagged
//! with the stream it arrived on) plus an optional process-exit signal.
//! The harness drives an adapter through the transcript, drains every
//! channel of the `Adapter` trait, and snapshots the whole result to
//! deterministic JSON. The committed `<case>.golden.json` is the contract;
//! any drift fails the test. Regenerate goldens with `UPDATE_GOLDEN=1`.

use adapter_core::{Adapter, AdapterExit};
use event_schema::{Event, LogStream};
use serde::Deserialize;
use serde_json::json;
use std::path::Path;

const NORMALIZED_TS: &str = "<ts>";

/// One recorded output line and the stream it arrived on.
#[derive(Debug, Deserialize)]
pub struct TranscriptLine {
    pub stream: Stream,
    pub text: String,
}

#[derive(Debug, Clone, Copy, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Stream {
    Stdout,
    Stderr,
}

impl From<Stream> for LogStream {
    fn from(s: Stream) -> Self {
        match s {
            Stream::Stdout => LogStream::Stdout,
            Stream::Stderr => LogStream::Stderr,
        }
    }
}

/// A recorded CLI run. `finalize` is `null` (the CLI emitted its own
/// terminal line), or `"Clean"` / `"Killed"` / `"Failed"` / `"Failed:<code>"`.
#[derive(Debug, Deserialize)]
pub struct Transcript {
    pub lines: Vec<TranscriptLine>,
    #[serde(default)]
    pub finalize: Option<String>,
}

impl Transcript {
    pub fn load(path: &Path) -> Transcript {
        let raw = std::fs::read_to_string(path)
            .unwrap_or_else(|e| panic!("read transcript {}: {e}", path.display()));
        serde_json::from_str(&raw)
            .unwrap_or_else(|e| panic!("parse transcript {}: {e}", path.display()))
    }

    fn exit(&self) -> Option<AdapterExit> {
        let spec = self.finalize.as_deref()?;
        Some(match spec {
            "Clean" => AdapterExit::Clean,
            "Killed" => AdapterExit::Killed,
            "Failed" => AdapterExit::Failed { code: None },
            other if other.starts_with("Failed:") => AdapterExit::Failed {
                code: Some(other["Failed:".len()..].parse::<i32>().unwrap_or_else(|_| {
                    panic!("invalid Failed exit code in finalize spec: {other:?}")
                })),
            },
            other => panic!("unknown finalize spec: {other:?}"),
        })
    }
}

/// The full observable output of an adapter run. Side-channel drains are
/// rendered via `Debug` so the harness needs no `Serialize` impls on the
/// signal types — Debug is stable and human-reviewable in the golden.
#[derive(Debug)]
pub struct Snapshot {
    pub events: Vec<Event>,
    pub session_id: Option<String>,
    pub quota_signal: Option<String>,
    pub memory_intents: Vec<String>,
    pub context_requests: Vec<String>,
}

impl Snapshot {
    /// Deterministic JSON. The wall-clock `ts` envelope field is redacted
    /// so goldens are stable across runs.
    pub fn to_golden(&self) -> String {
        let events: Vec<serde_json::Value> = self
            .events
            .iter()
            .map(|e| {
                let mut v = serde_json::to_value(e).expect("event serializes");
                if let Some(obj) = v.as_object_mut() {
                    obj.insert("ts".into(), json!(NORMALIZED_TS));
                }
                v
            })
            .collect();
        let snapshot = json!({
            "events": events,
            "session_id": self.session_id,
            "quota_signal": self.quota_signal,
            "memory_intents": self.memory_intents,
            "context_requests": self.context_requests,
        });
        let mut s = serde_json::to_string_pretty(&snapshot).expect("snapshot serializes");
        s.push('\n');
        s
    }
}

/// Drive `adapter` through the transcript, then drain every trait channel.
pub fn run(adapter: &mut dyn Adapter, t: &Transcript) -> Snapshot {
    let mut events = Vec::new();
    for entry in &t.lines {
        events.extend(adapter.ingest_line(&entry.text, entry.stream.into()));
    }
    if let Some(exit) = t.exit() {
        events.extend(adapter.finalize(exit));
    }
    Snapshot {
        events,
        session_id: adapter.take_session_id(),
        quota_signal: adapter.take_quota_signal().map(|q| format!("{q:?}")),
        memory_intents: adapter
            .take_memory_intents()
            .iter()
            .map(|m| format!("{m:?}"))
            .collect(),
        context_requests: adapter
            .take_context_requests()
            .iter()
            .map(|c| format!("{c:?}"))
            .collect(),
    }
}

/// Load `<manifest_dir>/tests/conformance/<case>.transcript.json`, run it
/// through `adapter`, and compare the snapshot to the committed
/// `<case>.golden.json`. `UPDATE_GOLDEN=1` (re)writes the golden instead.
pub fn assert_conformance(manifest_dir: &str, case: &str, adapter: &mut dyn Adapter) {
    let base = Path::new(manifest_dir).join("tests/conformance");
    let transcript = Transcript::load(&base.join(format!("{case}.transcript.json")));
    let actual = run(adapter, &transcript).to_golden();
    let golden_path = base.join(format!("{case}.golden.json"));

    if std::env::var_os("UPDATE_GOLDEN").is_some() {
        std::fs::create_dir_all(&base).expect("create conformance dir");
        std::fs::write(&golden_path, &actual).expect("write golden");
        eprintln!("UPDATE_GOLDEN: wrote {}", golden_path.display());
        return;
    }

    let expected = std::fs::read_to_string(&golden_path).unwrap_or_else(|_| {
        panic!(
            "golden {} missing — run with UPDATE_GOLDEN=1 to create it",
            golden_path.display()
        )
    });
    assert_eq!(
        actual, expected,
        "conformance drift for case '{case}': adapter output changed. \
         If this change is intentional, re-run with UPDATE_GOLDEN=1 and review the diff."
    );
}

#[cfg(test)]
mod tests {
    use super::*;
    use event_schema::{EventKind, Log, LogLevel, StateChange, WorkerState};

    #[test]
    fn finalize_spec_parses_all_forms() {
        let mk = |f: Option<&str>| Transcript {
            lines: vec![],
            finalize: f.map(str::to_string),
        };
        assert!(mk(None).exit().is_none());
        assert!(matches!(mk(Some("Clean")).exit(), Some(AdapterExit::Clean)));
        assert!(matches!(
            mk(Some("Killed")).exit(),
            Some(AdapterExit::Killed)
        ));
        assert!(matches!(
            mk(Some("Failed")).exit(),
            Some(AdapterExit::Failed { code: None })
        ));
        assert!(matches!(
            mk(Some("Failed:2")).exit(),
            Some(AdapterExit::Failed { code: Some(2) })
        ));
    }

    #[test]
    fn to_golden_redacts_ts() {
        let snap = Snapshot {
            events: vec![Event {
                schema_version: "x".into(),
                worker_id: "w".into(),
                task_id: None,
                seq: 0,
                ts: "2026-01-01T00:00:00Z".into(),
                kind: EventKind::Log(Log {
                    level: LogLevel::Info,
                    stream: LogStream::Stdout,
                    line: "hi".into(),
                    tag: None,
                }),
            }],
            session_id: Some("sid".into()),
            quota_signal: None,
            memory_intents: vec![],
            context_requests: vec![],
        };
        let golden = snap.to_golden();
        assert!(golden.contains("<ts>"), "ts must be redacted: {golden}");
        assert!(
            !golden.contains("2026-01-01T00:00:00Z"),
            "raw ts must not leak: {golden}"
        );
        assert!(golden.contains("\"session_id\": \"sid\""));
    }

    #[test]
    fn run_collects_events_and_drains() {
        // A minimal adapter that emits one idle state_change per line.
        #[derive(Debug, Default)]
        struct OneShot {
            emitted: bool,
        }
        impl Adapter for OneShot {
            fn ingest_line(&mut self, _line: &str, _stream: LogStream) -> Vec<Event> {
                self.emitted = true;
                vec![Event {
                    schema_version: "x".into(),
                    worker_id: "w".into(),
                    task_id: None,
                    seq: 0,
                    ts: "t".into(),
                    kind: EventKind::StateChange(StateChange {
                        state: WorkerState::Idle,
                        from: None,
                        note: None,
                    }),
                }]
            }
            fn take_session_id(&mut self) -> Option<String> {
                Some("sess".into())
            }
        }
        let t = Transcript {
            lines: vec![TranscriptLine {
                stream: Stream::Stdout,
                text: "anything".into(),
            }],
            finalize: None,
        };
        let mut a = OneShot::default();
        let snap = run(&mut a, &t);
        assert_eq!(snap.events.len(), 1);
        assert_eq!(snap.session_id.as_deref(), Some("sess"));
    }
}
