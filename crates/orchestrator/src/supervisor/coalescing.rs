//! `CoalescingSink` — UI/IPC throughput shaping for the user's
//! `WorkerEventSink`. Forwards critical events (state_change,
//! completion, failure, file_activity, test_result, cost,
//! dependency, artifact) immediately. Rate-limits log events at
//! `log_rate` per worker per second (Task 4). Coalesces progress
//! events to "latest per (worker, task)" with a 100ms flush
//! interval (Task 5).

use crate::ids::rfc3339_now;
use crate::parser::WorkerEventSink;
use event_schema::{Event, EventKind, Log, LogLevel, LogStream};
use std::collections::HashMap;
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::{Duration, Instant};
use tokio::task::JoinHandle;
use tokio::time::{interval, MissedTickBehavior};

/// Default per-worker log rate (events per second). Override via
/// `VIGLA_SINK_LOG_RATE` environment variable.
const LOG_RATE_PER_WORKER_PER_SEC_DEFAULT: u32 = 20;

const PROGRESS_FLUSH_INTERVAL: Duration = Duration::from_millis(100);
const LOG_SUMMARY_INTERVAL: Duration = Duration::from_secs(1);

fn log_rate_from_env() -> u32 {
    std::env::var("VIGLA_SINK_LOG_RATE")
        .ok()
        .and_then(|s| s.parse::<u32>().ok())
        .filter(|&v| v > 0)
        .unwrap_or(LOG_RATE_PER_WORKER_PER_SEC_DEFAULT)
}

#[derive(Default)]
struct WorkerSlot {
    /// Tokens remaining in the current 1-second window for log events.
    log_tokens: u32,
    /// Instant the current window began.
    log_window_start: Option<Instant>,
    /// Count of log events dropped since the last summary flush.
    log_dropped: u32,
    /// One pending progress event per task scope.
    pending_progress: HashMap<Option<String>, Event>,
}

#[derive(Default)]
struct CoalescerState {
    per_worker: HashMap<String, WorkerSlot>,
}

pub struct CoalescingSink {
    inner: Arc<dyn WorkerEventSink>,
    state: Arc<Mutex<CoalescerState>>,
    log_rate: u32,
    flush_handle: Option<JoinHandle<()>>,
}

impl CoalescingSink {
    pub fn new(inner: Arc<dyn WorkerEventSink>) -> Self {
        let log_rate = log_rate_from_env();
        let state = Arc::new(Mutex::new(CoalescerState::default()));
        let flush_handle = spawn_flush_loop(state.clone(), inner.clone());
        Self {
            inner,
            state,
            log_rate,
            flush_handle: Some(flush_handle),
        }
    }
}

impl WorkerEventSink for CoalescingSink {
    fn emit(&self, event: &Event) {
        match &event.kind {
            EventKind::Log(_) => self.emit_log(event),
            EventKind::Progress(_) => self.stash_progress(event),
            _ => self.inner.emit(event),
        }
    }
}

impl CoalescingSink {
    fn emit_log(&self, event: &Event) {
        let now = Instant::now();
        let should_forward = {
            let mut s = lock_state(&self.state);
            let slot = s.per_worker.entry(event.worker_id.clone()).or_default();
            // Initialize or roll the window.
            let roll = match slot.log_window_start {
                None => true,
                Some(start) => now.duration_since(start) >= Duration::from_secs(1),
            };
            if roll {
                slot.log_tokens = self.log_rate;
                slot.log_window_start = Some(now);
            }
            if slot.log_tokens > 0 {
                slot.log_tokens -= 1;
                true
            } else {
                slot.log_dropped = slot.log_dropped.saturating_add(1);
                false
            }
        };
        if should_forward {
            self.inner.emit(event); // emit OUTSIDE the lock
        }
    }

    fn stash_progress(&self, event: &Event) {
        let key = event.task_id.clone();
        let mut s = lock_state(&self.state);
        let slot = s.per_worker.entry(event.worker_id.clone()).or_default();
        slot.pending_progress.insert(key, event.clone());
        // Don't emit here — the flush task delivers pending progress
        // on its 100ms tick.
    }
}

impl Drop for CoalescingSink {
    fn drop(&mut self) {
        if let Some(h) = self.flush_handle.take() {
            h.abort();
        }
    }
}

fn spawn_flush_loop(
    state: Arc<Mutex<CoalescerState>>,
    inner: Arc<dyn WorkerEventSink>,
) -> JoinHandle<()> {
    crate::spawn_supervised("coalescing::flush_loop", async move {
        let mut progress_tick = interval(PROGRESS_FLUSH_INTERVAL);
        let mut summary_tick = interval(LOG_SUMMARY_INTERVAL);
        progress_tick.set_missed_tick_behavior(MissedTickBehavior::Skip);
        summary_tick.set_missed_tick_behavior(MissedTickBehavior::Skip);
        // Consume the immediate first fire of each interval so the
        // first flush happens after one full tick, not at spawn time.
        progress_tick.tick().await;
        summary_tick.tick().await;
        loop {
            tokio::select! {
                _ = progress_tick.tick() => flush_progress(&state, &*inner),
                _ = summary_tick.tick()  => flush_log_summaries(&state, &*inner),
            }
        }
    })
}

fn flush_progress(state: &Mutex<CoalescerState>, inner: &dyn WorkerEventSink) {
    let to_send: Vec<Event> = {
        let mut s = lock_state(state);
        s.per_worker
            .values_mut()
            .flat_map(|slot| slot.pending_progress.drain().map(|(_, e)| e))
            .collect()
    };
    for e in to_send {
        inner.emit(&e);
    }
}

fn flush_log_summaries(state: &Mutex<CoalescerState>, inner: &dyn WorkerEventSink) {
    let summaries: Vec<Event> = {
        let mut s = lock_state(state);
        s.per_worker
            .iter_mut()
            .filter_map(|(worker_id, slot)| {
                let n = std::mem::take(&mut slot.log_dropped);
                (n > 0).then(|| synth_drop_summary(worker_id, n))
            })
            .collect()
    };
    for e in summaries {
        inner.emit(&e);
    }
}

/// Keep UI event delivery alive if a prior writer unwinds while holding the
/// state lock. Every mutation below re-establishes its token/progress invariants.
fn lock_state(state: &Mutex<CoalescerState>) -> MutexGuard<'_, CoalescerState> {
    state
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
}

fn synth_drop_summary(worker_id: &str, n: u32) -> Event {
    Event {
        schema_version: "1.0".into(),
        worker_id: worker_id.into(),
        task_id: None,
        seq: 0,
        ts: rfc3339_now(),
        kind: EventKind::Log(Log {
            level: LogLevel::Warn,
            stream: LogStream::Stderr,
            line: format!("[vigla: {n} log events dropped (rate-limited)]"),
            tag: Some("vigla:rate-limit".into()),
        }),
    }
}
