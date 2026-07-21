use crate::{Adapter, AdapterExit, QuotaSignal};
use event_schema::{
    Completion, Event, EventKind, Failure, FailureCategory, Log, LogLevel, LogStream, StateChange,
    WorkerState, SCHEMA_VERSION,
};

/// Generic adapter for CLIs that do not yet expose a structured event stream.
#[derive(Debug)]
pub struct RawLogAdapter {
    worker_id: String,
    task_id: Option<String>,
    completion_summary: &'static str,
    seq: u64,
    started: bool,
    terminal_emitted: bool,
    pending_quota_signal: Option<QuotaSignal>,
}

impl RawLogAdapter {
    pub fn new(
        worker_id: impl Into<String>,
        task_id: Option<String>,
        completion_summary: &'static str,
    ) -> Self {
        Self {
            worker_id: worker_id.into(),
            task_id,
            completion_summary,
            seq: 0,
            started: false,
            terminal_emitted: false,
            pending_quota_signal: None,
        }
    }

    fn make(&mut self, kind: EventKind) -> Event {
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

    fn ensure_started(&mut self, out: &mut Vec<Event>) {
        if !self.started {
            // event-schema §5: the first event of any worker stream MUST be
            // state_change → idle. Emit idle→executing as the opening pair
            // (matching `AdapterCore::ensure_started`) so the raw-log
            // vendors (antigravity/kiro/copilot) don't open their stream on
            // `Executing` and violate the contract (F-5).
            out.push(self.make(EventKind::StateChange(StateChange {
                state: WorkerState::Idle,
                from: None,
                note: None,
            })));
            out.push(self.make(EventKind::StateChange(StateChange {
                state: WorkerState::Executing,
                from: Some(WorkerState::Idle),
                note: None,
            })));
            self.started = true;
        }
    }
}

impl Adapter for RawLogAdapter {
    fn ingest_line(&mut self, line: &str, stream: LogStream) -> Vec<Event> {
        let trimmed = line.trim_end_matches(['\r', '\n']);
        if trimmed.is_empty() {
            return Vec::new();
        }

        let mut out = Vec::new();
        self.ensure_started(&mut out);
        // Vendors with no structured stream (antigravity/kiro/copilot)
        // still hit quota walls; surface the canonical signal so the
        // recovery engine can pause them like the structured adapters.
        if crate::is_quota_exhaustion_line(trimmed) {
            self.pending_quota_signal = Some(QuotaSignal {
                estimated_reset_at_ms: None,
            });
        }
        let (level, log_stream) = match stream {
            LogStream::Stderr => (LogLevel::Error, LogStream::Stderr),
            LogStream::Stdout => (LogLevel::Info, LogStream::Stdout),
        };
        out.push(self.make(EventKind::Log(Log {
            level,
            stream: log_stream,
            line: trimmed.to_string(),
            tag: None,
        })));
        out
    }

    fn finalize(&mut self, exit: AdapterExit) -> Vec<Event> {
        if self.terminal_emitted {
            return Vec::new();
        }
        self.terminal_emitted = true;
        let mut out = Vec::new();
        // §5: the first event of any worker stream MUST be state_change →
        // idle. A worker that produced no stdout/stderr and then exited (or
        // was killed instantly) reaches finalize without ever having run
        // `ensure_started`, so without this the terminal below would be the
        // stream's opening event with `from: None` — a §5 violation. Open
        // the stream (idle→executing) first, exactly as `ingest_line` does;
        // this is a no-op once any line was ingested (F-5).
        self.ensure_started(&mut out);

        match exit {
            AdapterExit::Clean => {
                out.push(self.make(EventKind::StateChange(StateChange {
                    state: WorkerState::Done,
                    from: Some(WorkerState::Executing),
                    note: None,
                })));
                out.push(self.make(EventKind::Completion(Completion {
                    summary: self.completion_summary.to_string(),
                    artifacts: None,
                    duration_ms: None,
                })));
            }
            AdapterExit::Failed { code } => {
                out.push(self.make(EventKind::StateChange(StateChange {
                    state: WorkerState::Failed,
                    from: Some(WorkerState::Executing),
                    note: None,
                })));
                out.push(self.make(EventKind::Failure(Failure {
                    error: format!("exit code {:?}", code),
                    retryable: false,
                    suggestion: None,
                    exit_code: code,
                    category: Some(FailureCategory::Internal),
                })));
            }
            AdapterExit::Killed => {
                out.push(self.make(EventKind::StateChange(StateChange {
                    state: WorkerState::Failed,
                    from: Some(WorkerState::Executing),
                    note: Some("worker stopped".into()),
                })));
                out.push(self.make(EventKind::Failure(Failure {
                    error: "killed".to_string(),
                    retryable: false,
                    suggestion: None,
                    exit_code: None,
                    category: Some(FailureCategory::Internal),
                })));
            }
        }
        out
    }

    fn take_quota_signal(&mut self) -> Option<QuotaSignal> {
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
    fn non_empty_stdout_starts_and_logs() {
        let mut adapter = RawLogAdapter::new("worker-1", Some("task-1".into()), "done");
        let events = adapter.ingest_line("hello\n", LogStream::Stdout);

        // §5: stream opens idle → executing, then the log line (F-5).
        assert_eq!(events.len(), 3);
        assert_eq!(events[0].seq, 0);
        assert_eq!(events[2].seq, 2);
        assert!(matches!(
            &events[0].kind,
            EventKind::StateChange(StateChange {
                state: WorkerState::Idle,
                ..
            })
        ));
        assert!(matches!(
            &events[1].kind,
            EventKind::StateChange(StateChange {
                state: WorkerState::Executing,
                ..
            })
        ));
        assert!(matches!(
            &events[2].kind,
            EventKind::Log(Log {
                level: LogLevel::Info,
                stream: LogStream::Stdout,
                line,
                ..
            }) if line == "hello"
        ));
    }

    #[test]
    fn stderr_logs_as_error() {
        let mut adapter = RawLogAdapter::new("worker-1", None, "done");
        let events = adapter.ingest_line("boom", LogStream::Stderr);

        // idle@0, executing@1, then the stderr log@2 (F-5).
        assert!(matches!(
            &events[2].kind,
            EventKind::Log(Log {
                level: LogLevel::Error,
                stream: LogStream::Stderr,
                line,
                ..
            }) if line == "boom"
        ));
    }

    #[test]
    fn clean_finalize_uses_summary_and_is_idempotent() {
        let mut adapter = RawLogAdapter::new("worker-1", None, "worker finished");
        let events = adapter.finalize(AdapterExit::Clean);
        let second = adapter.finalize(AdapterExit::Clean);

        // §5: a zero-output finalize still opens idle→executing before the
        // terminal, so the stream never starts on a terminal event.
        assert_eq!(events.len(), 4);
        assert!(second.is_empty());
        assert!(matches!(
            &events[0].kind,
            EventKind::StateChange(StateChange {
                state: WorkerState::Idle,
                ..
            })
        ));
        assert!(matches!(
            &events[3].kind,
            EventKind::Completion(Completion { summary, .. }) if summary == "worker finished"
        ));
    }

    #[test]
    fn failed_finalize_carries_exit_code() {
        let mut adapter = RawLogAdapter::new("worker-1", None, "done");
        let events = adapter.finalize(AdapterExit::Failed { code: Some(75) });

        assert!(matches!(
            &events[3].kind,
            EventKind::Failure(Failure {
                exit_code: Some(75),
                retryable: false,
                ..
            })
        ));
    }

    #[test]
    fn zero_output_finalize_opens_with_idle_not_a_terminal() {
        // §5 regression: a silent worker (no lines ingested) that is
        // killed must still emit `idle` as its FIRST event, not a terminal
        // state_change with `from: None`.
        for exit in [
            AdapterExit::Clean,
            AdapterExit::Failed { code: Some(1) },
            AdapterExit::Killed,
        ] {
            let mut adapter = RawLogAdapter::new("w", None, "done");
            let events = adapter.finalize(exit);
            assert!(
                matches!(
                    &events[0].kind,
                    EventKind::StateChange(StateChange {
                        state: WorkerState::Idle,
                        from: None,
                        ..
                    })
                ),
                "first event must be idle, got {:?}",
                events[0].kind
            );
        }
    }

    #[test]
    fn quota_exhaustion_line_emits_drainable_signal() {
        let mut adapter = RawLogAdapter::new("worker-1", None, "done");
        let _ = adapter.ingest_line("Error: 429 Too Many Requests", LogStream::Stderr);
        assert!(
            adapter.take_quota_signal().is_some(),
            "raw-log adapter must surface a quota signal so quota-only vendors can pause"
        );
        assert!(
            adapter.take_quota_signal().is_none(),
            "quota signal must be drained after the first take"
        );
    }

    #[test]
    fn benign_line_with_429_substring_emits_no_signal() {
        let mut adapter = RawLogAdapter::new("worker-1", None, "done");
        let _ = adapter.ingest_line("wrote build/artifact-429.bin", LogStream::Stdout);
        assert!(adapter.take_quota_signal().is_none());
    }
}
