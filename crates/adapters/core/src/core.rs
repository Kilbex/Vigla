//! Shared adapter scaffolding embedded by every vendor adapter.
//! Owns the canonical-event envelope (worker_id/task_id/seq), the
//! idle→executing lifecycle gates, and the three drain buffers
//! (session id, memory intents, quota signal). Vendor-specific
//! parsing and text accumulation stay on the vendor struct.

use crate::{MemoryIntent, QuotaSignal};
use event_schema::{Event, EventKind, StateChange, WorkerState, SCHEMA_VERSION};

/// Shared adapter scaffolding. Embedded as `core` by each vendor
/// adapter; the vendor struct keeps only its parsing-specific state.
#[derive(Debug)]
pub struct AdapterCore {
    pub worker_id: String,
    pub task_id: Option<String>,
    pub seq: u64,
    pub initial_idle_emitted: bool,
    pub started: bool,
    /// Set once a terminal (`Done`/`Failed`) state is emitted so
    /// `finalize` doesn't synthesize a second terminal event. (Codex
    /// previously called this `finalized`.)
    pub terminal_emitted: bool,
    pub pending_session_id: Option<String>,
    pub pending_memory_intents: Vec<MemoryIntent>,
    pub pending_quota_signal: Option<QuotaSignal>,
}

impl AdapterCore {
    pub fn new(worker_id: impl Into<String>, task_id: Option<String>) -> Self {
        Self::with_starting_seq(worker_id, task_id, 0)
    }

    /// Begin numbering events at `starting_seq`. Used by Claude's
    /// resume path (the events table's `(worker_id, seq)` PK means a
    /// resumed run must not restart seq at 0).
    pub fn with_starting_seq(
        worker_id: impl Into<String>,
        task_id: Option<String>,
        starting_seq: u64,
    ) -> Self {
        Self {
            worker_id: worker_id.into(),
            task_id,
            seq: starting_seq,
            initial_idle_emitted: false,
            started: false,
            terminal_emitted: false,
            pending_session_id: None,
            pending_memory_intents: Vec::new(),
            pending_quota_signal: None,
        }
    }

    pub fn make(&mut self, kind: EventKind) -> Event {
        let event = Event {
            schema_version: SCHEMA_VERSION.to_string(),
            worker_id: self.worker_id.clone(),
            task_id: self.task_id.clone(),
            seq: self.seq,
            ts: now_rfc3339(),
            kind,
        };
        self.seq += 1;
        event
    }

    pub fn ensure_idle(&mut self, out: &mut Vec<Event>) {
        if !self.initial_idle_emitted {
            out.push(self.make(EventKind::StateChange(StateChange {
                state: WorkerState::Idle,
                from: None,
                note: None,
            })));
            self.initial_idle_emitted = true;
        }
    }

    pub fn ensure_started(&mut self, out: &mut Vec<Event>) {
        self.ensure_idle(out);
        if !self.started {
            out.push(self.make(EventKind::StateChange(StateChange {
                state: WorkerState::Executing,
                from: Some(WorkerState::Idle),
                note: None,
            })));
            self.started = true;
        }
    }

    pub fn take_session_id(&mut self) -> Option<String> {
        self.pending_session_id.take()
    }

    pub fn take_memory_intents(&mut self) -> Vec<MemoryIntent> {
        std::mem::take(&mut self.pending_memory_intents)
    }

    pub fn take_quota_signal(&mut self) -> Option<QuotaSignal> {
        self.pending_quota_signal.take()
    }
}

fn now_rfc3339() -> String {
    use std::time::{SystemTime, UNIX_EPOCH};
    let ms = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0);
    event_schema::time::rfc3339_from_unix_ms(ms)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn first_ensure_idle_emits_one_idle_state_change() {
        let mut core = AdapterCore::new("w1", None);
        let mut out = Vec::new();
        core.ensure_idle(&mut out);
        core.ensure_idle(&mut out); // idempotent
        assert_eq!(out.len(), 1);
        match &out[0].kind {
            EventKind::StateChange(sc) => assert_eq!(sc.state, WorkerState::Idle),
            other => panic!("expected idle state_change, got {other:?}"),
        }
    }

    #[test]
    fn ensure_started_emits_idle_then_executing_once() {
        let mut core = AdapterCore::new("w1", None);
        let mut out = Vec::new();
        core.ensure_started(&mut out);
        core.ensure_started(&mut out); // idempotent
        assert_eq!(out.len(), 2);
        assert!(matches!(
            out[1].kind,
            EventKind::StateChange(StateChange {
                state: WorkerState::Executing,
                ..
            })
        ));
    }

    #[test]
    fn make_stamps_envelope_and_increments_seq() {
        let mut core = AdapterCore::with_starting_seq("w1", Some("t1".into()), 7);
        let e = core.make(EventKind::StateChange(StateChange {
            state: WorkerState::Idle,
            from: None,
            note: None,
        }));
        assert_eq!(e.worker_id, "w1");
        assert_eq!(e.task_id.as_deref(), Some("t1"));
        assert_eq!(e.seq, 7);
        assert_eq!(e.schema_version, SCHEMA_VERSION);
        assert_eq!(core.seq, 8);
    }

    #[test]
    fn take_session_id_drains_once() {
        let mut core = AdapterCore::new("w1", None);
        core.pending_session_id = Some("sid".into());
        assert_eq!(core.take_session_id().as_deref(), Some("sid"));
        assert!(core.take_session_id().is_none());
    }
}
