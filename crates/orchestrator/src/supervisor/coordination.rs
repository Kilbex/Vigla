use super::Supervisor;
use crate::parser::WorkerEventSink;
use event_schema::{Event, EventKind, WorkerState};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Weak};
use std::time::Duration;
use tokio::sync::mpsc;

/// Bounded capacity for the per-sink coordination channel. Sized
/// well above any realistic burst of state_change/completion/failure
/// events (the queue receives only those three kinds — see
/// `is_coordination_relevant`).
const COORDINATION_QUEUE_CAP: usize = 256;

/// Maximum time the blocking-send fallback waits for a slot before
/// dropping the event and incrementing `drop_count`. Should never
/// fire in practice since the queue rarely contains more than a
/// handful of events at once.
const COORDINATION_SEND_TIMEOUT: Duration = Duration::from_millis(10);

/// `WorkerEventSink` wrapper that observes coordination-relevant
/// events (state_change / completion / failure) and applies the
/// supervisor's downstream side effects, then forwards every event
/// to the user's sink.
///
/// Chatty nonterminal state updates flow through a bounded channel.
/// Structural terminal events use a separate lossless lane so queue
/// pressure can never strand dependants or retry bookkeeping. Events
/// not in the coordination set (log, progress, cost, file_activity,
/// test_result, dependency, artifact) never enter either queue.
pub struct CoordinatingSink {
    sender: mpsc::Sender<Event>,
    terminal_sender: mpsc::UnboundedSender<Event>,
    inner: Arc<dyn WorkerEventSink>,
    drop_count: Arc<AtomicU64>,
}

impl CoordinatingSink {
    pub fn new(supervisor: Weak<Supervisor>, inner: Arc<dyn WorkerEventSink>) -> Self {
        let (sender, mut receiver) = mpsc::channel::<Event>(COORDINATION_QUEUE_CAP);
        let (terminal_sender, mut terminal_receiver) = mpsc::unbounded_channel::<Event>();
        let terminal_supervisor = supervisor.clone();
        crate::spawn_supervised("coordination::consumer", async move {
            while let Some(event) = receiver.recv().await {
                let Some(sup) = terminal_supervisor.upgrade() else {
                    continue;
                };
                apply_coordination_side_effects(&sup, &event).await;
            }
        });
        crate::spawn_supervised("coordination::terminal_consumer", async move {
            while let Some(event) = terminal_receiver.recv().await {
                let Some(sup) = supervisor.upgrade() else {
                    continue;
                };
                apply_coordination_side_effects(&sup, &event).await;
            }
        });
        Self {
            sender,
            terminal_sender,
            inner,
            drop_count: Arc::new(AtomicU64::new(0)),
        }
    }

    /// Total number of coordination events dropped because the
    /// channel was full for longer than `COORDINATION_SEND_TIMEOUT`.
    /// Not exposed on the public crate surface; reachable through
    /// `Supervisor::coordinating_sink_for_test`.
    #[doc(hidden)]
    pub fn dropped_coordination_events_for_test(&self) -> u64 {
        self.drop_count.load(Ordering::Relaxed)
    }
}

impl WorkerEventSink for CoordinatingSink {
    fn emit(&self, event: &Event) {
        // Forward to the user's sink first so the UI sees events
        // before any synthetic effects.
        self.inner.emit(event);

        // Filter at send-time: only events the coordination consumer
        // actually acts on enter the queue.
        if !is_coordination_relevant(&event.kind) {
            return;
        }

        // Completion/failure/terminal state transitions release dependants or
        // close retry paths. They must never be dropped under backpressure.
        // The unbounded lane contains only these structural events; chatty
        // nonterminal state updates remain on the bounded lane below.
        if is_terminal_coordination(&event.kind) {
            let _ = self.terminal_sender.send(event.clone());
            return;
        }

        match self.sender.try_send(event.clone()) {
            Ok(()) => {}
            Err(mpsc::error::TrySendError::Full(ev)) => {
                let sender = self.sender.clone();
                let dropped = self.drop_count.clone();
                tokio::spawn(async move {
                    if tokio::time::timeout(COORDINATION_SEND_TIMEOUT, sender.send(ev))
                        .await
                        .is_err()
                    {
                        let n = dropped.fetch_add(1, Ordering::Relaxed) + 1;
                        tracing::warn!(
                            "orchestrator: coordination queue full, dropped coordination event ({n} total)"
                        );
                    }
                });
            }
            Err(mpsc::error::TrySendError::Closed(_)) => {
                // Receiver dropped — sink outlived supervisor. Harmless.
            }
        }
    }
}

fn is_terminal_coordination(kind: &EventKind) -> bool {
    match kind {
        EventKind::Completion(_) | EventKind::Failure(_) => true,
        EventKind::StateChange(change) => {
            matches!(change.state, WorkerState::Done | WorkerState::Failed)
        }
        _ => false,
    }
}

fn is_coordination_relevant(kind: &EventKind) -> bool {
    matches!(
        kind,
        EventKind::StateChange(_) | EventKind::Completion(_) | EventKind::Failure(_)
    )
}

/// Coordination effects derived from a single canonical event. Runs
/// inside the CoordinatingSink consumer task so consecutive events
/// from one worker are processed in arrival order.
async fn apply_coordination_side_effects(sup: &Arc<Supervisor>, event: &Event) {
    if let EventKind::StateChange(sc) = &event.kind {
        if let Err(e) = sup
            .repo
            .update_worker_state(&event.worker_id, sc.state)
            .await
        {
            tracing::error!("orchestrator: update_worker_state failed: {e}");
        }
        if matches!(sc.state, WorkerState::Done) {
            if let Some(task_id) = event.task_id.clone() {
                sup.on_task_completed(&task_id).await;
            }
        }
    }
    if let EventKind::Completion(_) = &event.kind {
        if let Some(task_id) = event.task_id.clone() {
            sup.on_task_completed(&task_id).await;
        }
    }
    if let EventKind::Failure(f) = &event.kind {
        if !f.retryable {
            if let Some(task_id) = event.task_id.clone() {
                sup.on_task_failed_terminal(&task_id).await;
            }
        }
    }
}
