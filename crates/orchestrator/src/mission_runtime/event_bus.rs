use crate::mission_event::MissionEvent;
use std::collections::{HashSet, VecDeque};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex as StdMutex};
use tokio::sync::broadcast;

/// R5 — upper bound on the per-mission broadcast-replay buffer.
/// Without a cap, long verbose missions (real-worker stdout,
/// multi-task DAGs) push `history` into tens of MB. On an 8 GB MacBook
/// with three parallel missions this previously risked an OOM with no
/// diagnostic.
///
/// 2048 is `8 × broadcast capacity` (broadcast is 256); chosen so
/// the cap is comfortably larger than any C1-style burst while still
/// keeping the buffer bounded by a few MB worst-case.
///
/// This cap bounds ONLY the late-subscriber replay window. The
/// completion-verdict source is the separate, complete [`verdict_log`]
/// (which sheds only the high-volume `WorkerProgress` stream), so
/// capping replay can never silently truncate a verdict.
///
/// [`verdict_log`]: MissionEventBus::verdict_log
const MAX_HISTORY: usize = 2048;

/// Mission event broadcaster with replay for late subscribers.
///
/// `tokio::sync::broadcast` only delivers events emitted after a
/// receiver subscribes. The host starts the mission task before it can
/// install the frontend forwarder, so the first `mission.created`
/// event could be lost and the reducer would ignore every later event
/// as stale. Keep a tiny in-memory history per runtime and replay it
/// before live events for every subscriber.
///
/// The history is a `VecDeque` capped at [`MAX_HISTORY`]; once full,
/// the oldest event is dropped on each new emit. Late subscribers
/// that missed events outside that window will simply not see them —
/// the C1 broadcast-Lagged tolerance is the primary defence; this
/// cap exists only to bound memory.
#[derive(Debug, Clone)]
pub(crate) struct MissionEventBus {
    tx: broadcast::Sender<MissionEvent>,
    /// Serializes sequence allocation with history/broadcast publication.
    /// Without this lock parallel tasks can publish seq N+1 before seq N.
    publish_lock: Arc<StdMutex<()>>,
    history: Arc<StdMutex<VecDeque<MissionEvent>>>,
    /// S9: complete, ordered record of every emitted event kind that a
    /// completion verdict may need — i.e. everything except the
    /// high-volume `WorkerProgress` stream (see [`kind_is_sheddable`]).
    /// Unlike `history` this is NOT capped: it grows only with
    /// structural/decision events (bounded by tasks × rework attempts),
    /// never with per-line worker output, so it stays small while
    /// guaranteeing the verdict source is never silently truncated on a
    /// long mission. Read by [`MissionEventBus::snapshot_kinds`].
    verdict_log: Arc<StdMutex<Vec<crate::mission_event::MissionEventKind>>>,
    /// S5: shared per-host tracker. Cloned into each mission's
    /// per-task loop via the existing event-bus indirection so we
    /// don't have to thread it as a separate arg through
    /// `run_supervisor_mission`. Resolved from
    /// [`crate::recovery::quota::shared_or_in_memory`]: production
    /// installs one sqlite-backed instance at startup (so quota pauses
    /// survive a host restart and are shared across concurrent
    /// missions); tests fall back to a fresh in-memory tracker.
    pub(crate) quota_tracker: Arc<crate::recovery::quota::VendorQuotaTracker>,
}

impl MissionEventBus {
    pub(crate) fn new(capacity: usize) -> Self {
        let (tx, _) = broadcast::channel(capacity);
        Self {
            tx,
            publish_lock: Arc::new(StdMutex::new(())),
            history: Arc::new(StdMutex::new(VecDeque::with_capacity(MAX_HISTORY))),
            verdict_log: Arc::new(StdMutex::new(Vec::new())),
            quota_tracker: crate::recovery::quota::shared_or_in_memory(),
        }
    }

    pub(crate) fn subscribe(&self) -> MissionEventReceiver {
        // Subscribe first, then snapshot. Anything emitted after the
        // subscribe call is available from `live`; if it also appears
        // in the snapshot, `max_replayed_seq` de-duplicates it.
        let live = self.tx.subscribe();
        let mut backlog: Vec<MissionEvent> = {
            let guard = self.history.lock().unwrap_or_else(|p| p.into_inner());
            guard.iter().cloned().collect()
        };
        backlog.sort_by_key(|event| event.seq);
        let replayed_seqs = backlog.iter().map(|event| event.seq).collect();
        MissionEventReceiver {
            backlog: backlog.into(),
            live,
            replayed_seqs,
        }
    }

    #[cfg(test)]
    pub(crate) fn emit(&self, event: MissionEvent) {
        let _publish = self.publish_lock.lock().unwrap_or_else(|p| p.into_inner());
        self.emit_locked(event);
    }

    pub(crate) fn emit_kind(
        &self,
        mission_id: &str,
        seq: &AtomicU64,
        kind: crate::mission_event::MissionEventKind,
    ) {
        let _publish = self.publish_lock.lock().unwrap_or_else(|p| p.into_inner());
        let event = MissionEvent {
            mission_id: mission_id.to_string(),
            seq: seq.fetch_add(1, Ordering::SeqCst),
            ts: crate::ids::rfc3339_now(),
            kind,
        };
        self.emit_locked(event);
    }

    fn emit_locked(&self, event: MissionEvent) {
        {
            let mut guard = self.history.lock().unwrap_or_else(|p| p.into_inner());
            if guard.len() >= MAX_HISTORY {
                guard.pop_front();
            }
            guard.push_back(event.clone());
        }
        // Mirror every verdict-relevant kind into the uncapped
        // `verdict_log` so a long mission's verdict source survives even
        // after `history` has evicted these events.
        if !kind_is_sheddable(&event.kind) {
            let mut log = self.verdict_log.lock().unwrap_or_else(|p| p.into_inner());
            log.push(event.kind.clone());
        }
        let _ = self.tx.send(event);
    }

    /// Snapshot the complete, ordered list of verdict-relevant event
    /// kinds for S9 mission-loop verdict assembly. Reads the uncapped
    /// `verdict_log` (not the capped replay `history`), so every
    /// `AssembleInputs` field is derived from the *whole* mission —
    /// no scrub, escalation, audit, or touched-file record can be lost
    /// to the replay cap on a long run. Still keeps the per-task
    /// `run_task` futures decoupled from the verdict path.
    pub(crate) fn snapshot_kinds(&self) -> Vec<crate::mission_event::MissionEventKind> {
        let log = self.verdict_log.lock().unwrap_or_else(|p| p.into_inner());
        log.clone()
    }
}

/// Whether an event kind may be dropped from the verdict source
/// ([`MissionEventBus::verdict_log`]). Only `WorkerProgress` qualifies:
/// it is the sole per-output-line, high-volume kind (the driver of the
/// "tens of MB" growth [`MAX_HISTORY`] guards) and NO completion-verdict
/// input reads it. Everything else is retained in full so adding a new
/// verdict-relevant kind is safe by default — the failure mode is
/// bounded extra memory, never a silently truncated verdict.
fn kind_is_sheddable(kind: &crate::mission_event::MissionEventKind) -> bool {
    matches!(
        kind,
        crate::mission_event::MissionEventKind::WorkerProgress { .. }
    )
}

/// Receiver returned by [`MissionRuntime::subscribe`]. It first drains
/// the replay snapshot, then continues with live broadcast events.
#[derive(Debug)]
pub struct MissionEventReceiver {
    backlog: VecDeque<MissionEvent>,
    live: broadcast::Receiver<MissionEvent>,
    replayed_seqs: HashSet<u64>,
}

impl MissionEventReceiver {
    pub async fn recv(&mut self) -> Result<MissionEvent, broadcast::error::RecvError> {
        if let Some(event) = self.backlog.pop_front() {
            return Ok(event);
        }
        loop {
            let event = self.live.recv().await?;
            if self.replayed_seqs.contains(&event.seq) {
                continue;
            }
            return Ok(event);
        }
    }

    pub fn try_recv(&mut self) -> Result<MissionEvent, broadcast::error::TryRecvError> {
        if let Some(event) = self.backlog.pop_front() {
            return Ok(event);
        }
        loop {
            let event = self.live.try_recv()?;
            if self.replayed_seqs.contains(&event.seq) {
                continue;
            }
            return Ok(event);
        }
    }

    /// Test-only constructor: wrap a raw broadcast receiver as a
    /// `MissionEventReceiver` with an empty backlog. Lets downstream
    /// crates (e.g. the Tauri host's `forward_mission_events`
    /// regression test) drive lag/close paths without standing up a
    /// full `MissionRuntime`.
    #[doc(hidden)]
    pub fn for_testing(live: broadcast::Receiver<MissionEvent>) -> Self {
        Self {
            backlog: VecDeque::new(),
            live,
            replayed_seqs: HashSet::new(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::mission_event::MissionEventKind;

    fn ev(seq: u64) -> MissionEvent {
        MissionEvent {
            mission_id: "m".into(),
            seq,
            ts: "1970-01-01T00:00:00.000Z".into(),
            kind: MissionEventKind::ExecutionStarted,
        }
    }

    fn ev_kind(seq: u64, kind: MissionEventKind) -> MissionEvent {
        MissionEvent {
            mission_id: "m".into(),
            seq,
            ts: "1970-01-01T00:00:00.000Z".into(),
            kind,
        }
    }

    /// The completion verdict is derived from `snapshot_kinds`. That
    /// source must stay COMPLETE for structural/decision events even
    /// when a long, progress-heavy mission has pushed far past the
    /// broadcast-replay cap — otherwise an early scrub/escalation is
    /// silently evicted and the verdict reports a clean Accept on a
    /// mission that actually had an unresolved subtask.
    #[test]
    fn snapshot_kinds_retains_decisions_past_history_cap() {
        let bus = MissionEventBus::new(16);
        // An early decision, emitted before a flood of progress.
        bus.emit(ev_kind(
            0,
            MissionEventKind::ArbiterDecided {
                worker_id: "mock-1".into(),
                decision_json: "{}".into(),
                audit_overall: 0.0,
                bound: None,
            },
        ));
        // High-volume progress well past the replay cap.
        for s in 1..=(MAX_HISTORY as u64 + 50) {
            bus.emit(ev_kind(
                s,
                MissionEventKind::WorkerProgress {
                    worker_id: "mock-1".into(),
                    note: "working".into(),
                },
            ));
        }

        let kinds = bus.snapshot_kinds();
        assert!(
            kinds
                .iter()
                .any(|k| matches!(k, MissionEventKind::ArbiterDecided { .. })),
            "early ArbiterDecided must survive in the verdict source past the history cap"
        );
        assert!(
            !kinds
                .iter()
                .any(|k| matches!(k, MissionEventKind::WorkerProgress { .. })),
            "high-volume WorkerProgress must not bloat the verdict source"
        );
    }

    /// R5 regression: emitting beyond MAX_HISTORY drops the oldest
    /// events; a fresh subscriber sees exactly the most-recent
    /// MAX_HISTORY entries in their original order.
    #[tokio::test]
    async fn history_caps_at_max_history_and_drops_oldest() {
        let bus = MissionEventBus::new(16);
        let total = MAX_HISTORY * 2;
        for s in 0..total as u64 {
            bus.emit(ev(s));
        }

        let mut rx = bus.subscribe();
        let first_expected = (total - MAX_HISTORY) as u64;
        let last_expected = (total - 1) as u64;

        // Drain the replay snapshot only — live channel is empty.
        for expected in first_expected..=last_expected {
            let got = rx.try_recv().expect("backlog event");
            assert_eq!(got.seq, expected, "first dropped event leaked into replay");
        }
        // Backlog must be exhausted now (live channel has nothing).
        assert!(matches!(
            rx.try_recv(),
            Err(tokio::sync::broadcast::error::TryRecvError::Empty)
        ));
    }

    #[tokio::test]
    async fn subscribe_deduplicates_exact_replay_entries_without_dropping_late_lower_sequences() {
        let bus = MissionEventBus::new(16);
        bus.emit(ev(6));
        let mut rx = bus.subscribe();
        // Models the old reserve-then-publish race: seq 5 was not in the
        // subscriber's snapshot even though a higher sequence was.
        bus.emit(ev(5));

        assert_eq!(rx.recv().await.unwrap().seq, 6);
        let late = tokio::time::timeout(std::time::Duration::from_secs(1), rx.recv())
            .await
            .expect("late lower sequence must not be discarded")
            .unwrap();
        assert_eq!(late.seq, 5);
    }
}
