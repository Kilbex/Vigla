//! Codex CLI adapter.
//!
//! Translates OpenAI Codex CLI's `--json` output into Vigla
//! canonical events. Codex emits one JSON line per agent event:
//! `thread.started`, `turn.started`, `item.started`, `item.completed`,
//! `turn.completed`.
//!
//! Mapping to canonical events:
//!   * `thread.started`           → state_change idle (always first)
//!     followed by state_change executing
//!   * `turn.started`             → log{debug, tag=turn}
//!   * `item.started{command_execution}` → log{info, tag=tool:Bash}
//!   * `item.completed{command_execution}` → either:
//!       - `exit_code=0` → log{info, tag=tool:Bash}
//!       - else         → log{warn, tag=tool:Bash} + counts non-zero
//!   * `item.completed{agent_message}` → log{info, tag=assistant}
//!     (the last agent_message also drives the completion summary)
//!   * `item.completed{file_change}` (when codex emits one) →
//!     file_activity
//!   * `turn.completed`           → cost (usage)
//!
//! On `finalize()` (process EOF), if no terminal state has been
//! emitted yet, synthesise a `completion` + `state_change → done`
//! using the most recent agent_message text.

#![deny(missing_debug_implementations)]

use adapter_core::{Adapter, AdapterExit};
use event_schema::{
    Completion, Cost, Event, EventKind, Failure, FailureCategory, FileActivity, FileOp, Log,
    LogLevel, LogStream, StateChange, WorkerState,
};
use serde_json::Value;

#[derive(Debug)]
pub struct CodexAdapter {
    core: adapter_core::AdapterCore,
    /// Vendor-specific: the most recent agent_message text, used to
    /// drive the completion summary on finalize.
    last_agent_message: Option<String>,
}

impl CodexAdapter {
    pub fn new(worker_id: impl Into<String>, task_id: Option<String>) -> Self {
        Self {
            core: adapter_core::AdapterCore::new(worker_id, task_id),
            last_agent_message: None,
        }
    }

    fn handle_thread_started(&mut self, line_value: &Value, out: &mut Vec<Event>) {
        self.core.ensure_started(out);
        if self.core.pending_session_id.is_none() {
            if let Some(tid) = line_value.get("thread_id").and_then(|v| v.as_str()) {
                self.core.pending_session_id = Some(tid.to_owned());
            }
        }
    }

    fn handle_turn_started(&mut self, out: &mut Vec<Event>) {
        self.core.ensure_started(out);
        out.push(self.core.make(EventKind::Log(Log {
            level: LogLevel::Debug,
            stream: LogStream::Stdout,
            line: "turn started".into(),
            tag: Some("turn".into()),
        })));
    }

    fn handle_item_started(&mut self, item: &Value, out: &mut Vec<Event>) {
        self.core.ensure_started(out);
        let item_type = item.get("type").and_then(|v| v.as_str()).unwrap_or("");
        match item_type {
            "command_execution" => {
                let command = item
                    .get("command")
                    .and_then(|v| v.as_str())
                    .unwrap_or("(no command)");
                out.push(self.core.make(EventKind::Log(Log {
                    level: LogLevel::Info,
                    stream: LogStream::Stdout,
                    line: format!("running: {command}"),
                    tag: Some("tool:Bash".into()),
                })));
            }
            other if !other.is_empty() => {
                out.push(self.core.make(EventKind::Log(Log {
                    level: LogLevel::Trace,
                    stream: LogStream::Stdout,
                    line: format!("item started: {other}"),
                    tag: Some(format!("item:{other}")),
                })));
            }
            _ => {}
        }
    }

    fn handle_item_completed(&mut self, item: &Value, out: &mut Vec<Event>) {
        self.core.ensure_started(out);
        let item_type = item.get("type").and_then(|v| v.as_str()).unwrap_or("");
        match item_type {
            "command_execution" => {
                let command = item
                    .get("command")
                    .and_then(|v| v.as_str())
                    .unwrap_or("(no command)");
                let exit_code = item.get("exit_code").and_then(|v| v.as_i64());
                let success = matches!(exit_code, Some(0));
                let level = if success {
                    LogLevel::Info
                } else {
                    LogLevel::Warn
                };
                let aggregated = item
                    .get("aggregated_output")
                    .and_then(|v| v.as_str())
                    .unwrap_or("");
                let summary = if aggregated.is_empty() {
                    format!(
                        "completed: {command}{}",
                        exit_code
                            .map(|c| format!(" (exit {c})"))
                            .unwrap_or_default()
                    )
                } else {
                    let snippet = aggregated.lines().next().unwrap_or("");
                    format!("completed: {command} → {snippet}")
                };
                out.push(self.core.make(EventKind::Log(Log {
                    level,
                    stream: LogStream::Stdout,
                    line: summary,
                    tag: Some("tool:Bash".into()),
                })));
            }
            "agent_message" => {
                if let Some(text) = item.get("text").and_then(|v| v.as_str()) {
                    if !text.trim().is_empty() {
                        // Tier-2D: scan the extracted text for
                        // vigla_memory JSON lines (see
                        // ClaudeAdapter for the design rationale).
                        self.core
                            .pending_memory_intents
                            .extend(adapter_core::extract_intents(text));
                        self.last_agent_message = Some(text.to_string());
                        out.push(self.core.make(EventKind::Log(Log {
                            level: LogLevel::Info,
                            stream: LogStream::Stdout,
                            line: text.to_string(),
                            tag: Some("assistant".into()),
                        })));
                    }
                }
            }
            "file_change" | "file_edit" => {
                // Codex CLI variants — emit file_activity if present.
                let path = item
                    .get("path")
                    .or_else(|| item.get("file_path"))
                    .and_then(|v| v.as_str())
                    .map(str::to_string);
                let op = item
                    .get("op")
                    .and_then(|v| v.as_str())
                    .map(map_codex_op)
                    .unwrap_or(FileOp::Modified);
                if let Some(path) = path {
                    out.push(self.core.make(EventKind::FileActivity(FileActivity {
                        path,
                        op,
                        from_path: None,
                        lines_added: None,
                        lines_removed: None,
                        bytes: None,
                    })));
                }
            }
            other if !other.is_empty() => {
                out.push(self.core.make(EventKind::Log(Log {
                    level: LogLevel::Trace,
                    stream: LogStream::Stdout,
                    line: format!("item completed: {other}"),
                    tag: Some(format!("item:{other}")),
                })));
            }
            _ => {}
        }
    }

    fn handle_turn_completed(&mut self, line_value: &Value, out: &mut Vec<Event>) {
        let usage = line_value.get("usage");
        let input_tokens = usage
            .and_then(|u| u.get("input_tokens"))
            .and_then(|v| v.as_u64())
            .unwrap_or(0);
        let output_tokens = usage
            .and_then(|u| u.get("output_tokens"))
            .and_then(|v| v.as_u64())
            .unwrap_or(0);
        let cache_read = usage
            .and_then(|u| u.get("cached_input_tokens"))
            .and_then(|v| v.as_u64());
        // Codex reports tokens but not USD. We leave usd=0; integrators
        // can compute from a price book per model.
        out.push(self.core.make(EventKind::Cost(Cost {
            input_tokens,
            output_tokens,
            usd: 0.0,
            cache_read_tokens: cache_read,
            cache_write_tokens: None,
            model: None,
        })));

        // S5: quota detection from the error sub-object.
        let status = line_value
            .get("status")
            .and_then(|v| v.as_str())
            .unwrap_or("");
        if status == "error" {
            let code = line_value
                .get("error")
                .and_then(|e| e.get("code"))
                .and_then(|v| v.as_str())
                .unwrap_or("");
            let msg = line_value
                .get("error")
                .and_then(|e| e.get("message"))
                .and_then(|v| v.as_str())
                .unwrap_or("");
            if is_codex_quota_signal(code, msg) {
                self.core.pending_quota_signal = Some(adapter_core::QuotaSignal {
                    estimated_reset_at_ms: None,
                });
            }
        }
    }
}

fn map_codex_op(op: &str) -> FileOp {
    match op {
        "create" | "created" | "write" => FileOp::Created,
        "delete" | "deleted" | "remove" => FileOp::Deleted,
        "rename" | "renamed" => FileOp::Renamed,
        "read" => FileOp::Read,
        _ => FileOp::Modified,
    }
}

fn is_codex_quota_signal(code: &str, msg: &str) -> bool {
    // Structured codes are exact and reliable; free-text goes through
    // the shared, token-anchored matcher so detection is uniform across
    // vendors and benign "usage limit" / "429" mentions don't false-pause.
    code == "usage_limit_exceeded"
        || code == "rate_limit_exceeded"
        || adapter_core::is_quota_exhaustion_line(msg)
}

impl Adapter for CodexAdapter {
    fn ingest_line(&mut self, line: &str, stream: LogStream) -> Vec<Event> {
        let trimmed = line.trim_end_matches(['\r', '\n']);
        if trimmed.is_empty() {
            return Vec::new();
        }

        if matches!(stream, LogStream::Stderr) {
            let mut out = Vec::new();
            self.core.ensure_idle(&mut out);
            // S5: stderr 429 / rate-limit text is a quota signal.
            let lower = trimmed.to_ascii_lowercase();
            if is_codex_quota_signal("", &lower) {
                self.core.pending_quota_signal = Some(adapter_core::QuotaSignal {
                    estimated_reset_at_ms: None,
                });
            }
            out.push(self.core.make(EventKind::Log(Log {
                level: LogLevel::Warn,
                stream: LogStream::Stderr,
                line: trimmed.to_string(),
                tag: None,
            })));
            return out;
        }

        let mut out = Vec::new();
        self.core.ensure_idle(&mut out);

        let value: Value = match serde_json::from_str(trimmed) {
            Ok(v) => v,
            Err(_) => {
                out.push(self.core.make(EventKind::Log(Log {
                    level: LogLevel::Info,
                    stream: LogStream::Stdout,
                    line: trimmed.to_string(),
                    tag: None,
                })));
                return out;
            }
        };

        let line_type = value
            .get("type")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_string();
        match line_type.as_str() {
            "thread.started" => self.handle_thread_started(&value, &mut out),
            "turn.started" => self.handle_turn_started(&mut out),
            "item.started" => {
                if let Some(item) = value.get("item") {
                    self.handle_item_started(item, &mut out);
                }
            }
            "item.completed" => {
                if let Some(item) = value.get("item") {
                    self.handle_item_completed(item, &mut out);
                }
            }
            "turn.completed" => self.handle_turn_completed(&value, &mut out),
            "" => {
                out.push(self.core.make(EventKind::Log(Log {
                    level: LogLevel::Trace,
                    stream: LogStream::Stdout,
                    line: trimmed.to_string(),
                    tag: None,
                })));
            }
            other => {
                out.push(self.core.make(EventKind::Log(Log {
                    level: LogLevel::Trace,
                    stream: LogStream::Stdout,
                    line: format!("type={other}"),
                    tag: Some(format!("codex:{other}")),
                })));
            }
        }
        out
    }

    fn finalize(&mut self, exit: AdapterExit) -> Vec<Event> {
        if self.core.terminal_emitted {
            return Vec::new();
        }
        self.core.terminal_emitted = true;
        if !self.core.started {
            return Vec::new();
        }
        let mut out = Vec::new();
        // Codex doesn't emit an explicit "task done" event, so we
        // synthesise the terminal state ourselves on EOF — but we
        // synthesise based on the child's exit status, not always
        // a clean Completion. Killing or crashing the worker used
        // to leave the tile reading "done" even though the run
        // never finished.
        match exit {
            AdapterExit::Clean => {
                let summary = self
                    .last_agent_message
                    .clone()
                    .unwrap_or_else(|| "codex run completed".into());
                out.push(self.core.make(EventKind::Completion(Completion {
                    summary,
                    artifacts: None,
                    duration_ms: None,
                })));
                out.push(self.core.make(EventKind::StateChange(StateChange {
                    state: WorkerState::Done,
                    from: Some(WorkerState::Executing),
                    note: None,
                })));
            }
            AdapterExit::Failed { code } => {
                out.push(self.core.make(EventKind::Failure(Failure {
                    error: match code {
                        Some(c) => format!("codex exited with code {c}"),
                        None => "codex exited with non-zero status".into(),
                    },
                    retryable: false,
                    suggestion: None,
                    exit_code: code,
                    category: Some(FailureCategory::Internal),
                })));
                out.push(self.core.make(EventKind::StateChange(StateChange {
                    state: WorkerState::Failed,
                    from: Some(WorkerState::Executing),
                    note: None,
                })));
            }
            AdapterExit::Killed => {
                out.push(self.core.make(EventKind::Failure(Failure {
                    error: "codex killed before completion".into(),
                    retryable: false,
                    suggestion: None,
                    exit_code: None,
                    category: Some(FailureCategory::Internal),
                })));
                out.push(self.core.make(EventKind::StateChange(StateChange {
                    state: WorkerState::Failed,
                    from: Some(WorkerState::Executing),
                    note: Some("worker stopped".into()),
                })));
            }
        }
        out
    }

    fn take_session_id(&mut self) -> Option<String> {
        self.core.pending_session_id.take()
    }

    fn take_memory_intents(&mut self) -> Vec<adapter_core::MemoryIntent> {
        std::mem::take(&mut self.core.pending_memory_intents)
    }

    fn take_quota_signal(&mut self) -> Option<adapter_core::QuotaSignal> {
        self.core.pending_quota_signal.take()
    }
}

#[cfg(test)]
mod quota_tests {
    use super::*;
    use adapter_core::Adapter;
    use event_schema::LogStream;

    #[test]
    fn turn_completed_with_usage_limit_error_emits_quota_signal() {
        let mut a = CodexAdapter::new("w1", None);
        let line = r#"{"type":"turn.completed","status":"error","error":{"code":"usage_limit_exceeded","message":"rate limit exceeded"},"usage":{"input_tokens":0,"output_tokens":0}}"#;
        let _ = a.ingest_line(line, LogStream::Stdout);
        let sig = a.take_quota_signal();
        assert!(
            sig.is_some(),
            "expected quota signal on usage_limit_exceeded"
        );
    }

    #[test]
    fn stderr_429_emits_quota_signal() {
        let mut a = CodexAdapter::new("w1", None);
        let line = r#"ERROR: API request failed with status 429: Too Many Requests"#;
        let _ = a.ingest_line(line, LogStream::Stderr);
        let sig = a.take_quota_signal();
        assert!(sig.is_some(), "expected quota signal on stderr 429");
    }

    #[test]
    fn normal_completion_does_not_emit() {
        let mut a = CodexAdapter::new("w1", None);
        let line = r#"{"type":"turn.completed","status":"ok","usage":{"input_tokens":10,"output_tokens":5}}"#;
        let _ = a.ingest_line(line, LogStream::Stdout);
        assert!(a.take_quota_signal().is_none());
    }

    #[test]
    fn take_quota_signal_is_drained() {
        let mut a = CodexAdapter::new("w1", None);
        let line =
            r#"{"type":"turn.completed","status":"error","error":{"code":"usage_limit_exceeded"}}"#;
        a.ingest_line(line, LogStream::Stdout);
        assert!(a.take_quota_signal().is_some());
        assert!(a.take_quota_signal().is_none());
    }

    #[test]
    fn informational_usage_limit_line_does_not_emit_signal() {
        // Regression (C3): a benign line that merely states the limit
        // value must not be mistaken for exhaustion and pause the worker.
        let mut a = CodexAdapter::new("w1", None);
        let _ = a.ingest_line("Note: current usage limit is 1M tokens", LogStream::Stderr);
        assert!(a.take_quota_signal().is_none());
    }
}
