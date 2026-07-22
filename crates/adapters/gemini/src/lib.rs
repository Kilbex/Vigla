//! Google Gemini CLI adapter.
//!
//! Translates `gemini --output-format stream-json` output into Vigla
//! canonical events (the `event-schema` crate). Stateful: events
//! arrive line-by-line and require small amounts of cross-line
//! context (current task, seq counter, accumulated assistant text).
//!
//! **This crate does no I/O.** It is invoked by the supervision pipeline
//! with each stdout line; the supervisor wires it up end-to-end against
//! a real `gemini` subprocess.
//!
//! The adapter is built against a deterministic synthetic fixture
//! (`tests/fixtures/happy_path.jsonl`) that models these line shapes:
//!
//! | type        | notable fields                                       |
//! |-------------|------------------------------------------------------|
//! | init        | session_id, model                                    |
//! | message     | role (user/assistant), content, optional delta=true  |
//! | tool_use    | tool_name, tool_id, parameters                       |
//! | tool_result | tool_id, status (success/error), output              |
//! | result      | status, stats.{total,input,output}_tokens            |

#![deny(missing_debug_implementations)]

use adapter_core::{Adapter, AdapterExit};
use event_schema::{
    Completion, Cost, Event, EventKind, Failure as FailureEv, FailureCategory, FileActivity,
    FileOp, Log, LogLevel, LogStream, StateChange, WorkerState,
};
use serde::Deserialize;
use serde_json::Value;

const MAX_ACCUMULATED_TEXT_BYTES: usize = 256 * 1024;

/// Gemini stream-json adapter.
#[derive(Debug)]
pub struct GeminiAdapter {
    core: adapter_core::AdapterCore,
    /// Vendor-specific: accumulated assistant text for the completion
    /// summary fallback.
    accumulated_text: String,
}

impl GeminiAdapter {
    pub fn new(worker_id: impl Into<String>, task_id: Option<String>) -> Self {
        Self {
            core: adapter_core::AdapterCore::new(worker_id, task_id),
            accumulated_text: String::new(),
        }
    }

    fn handle_init(&mut self, line_value: &Value, out: &mut Vec<Event>) {
        // First line in the gemini stream — promote to executing.
        self.core.ensure_started(out);
        if self.core.pending_session_id.is_none() {
            if let Some(sid) = line_value.get("session_id").and_then(|v| v.as_str()) {
                self.core.pending_session_id = Some(sid.to_owned());
            }
        }
    }

    fn handle_message(&mut self, line_value: &Value, out: &mut Vec<Event>) {
        self.core.ensure_started(out);

        let role = line_value
            .get("role")
            .and_then(|v| v.as_str())
            .unwrap_or("");
        let content = line_value
            .get("content")
            .and_then(|v| v.as_str())
            .unwrap_or("");
        if content.is_empty() {
            return;
        }

        match role {
            "assistant" => {
                // Tier-2D: scan the extracted text for
                // vigla_memory JSON lines (see ClaudeAdapter
                // for the design rationale).
                self.core
                    .pending_memory_intents
                    .extend(adapter_core::extract_intents(content));
                adapter_core::append_bounded_tail(
                    &mut self.accumulated_text,
                    content,
                    MAX_ACCUMULATED_TEXT_BYTES,
                );
                if !self.accumulated_text.ends_with('\n') {
                    adapter_core::append_bounded_tail(
                        &mut self.accumulated_text,
                        "\n",
                        MAX_ACCUMULATED_TEXT_BYTES,
                    );
                }
                out.push(self.core.make(EventKind::Log(Log {
                    level: LogLevel::Info,
                    stream: LogStream::Stdout,
                    line: content.to_string(),
                    tag: Some("assistant".into()),
                })));
            }
            "user" => {
                // Surface the user prompt as a debug log so it's
                // visible in the drawer feed but doesn't dominate.
                out.push(self.core.make(EventKind::Log(Log {
                    level: LogLevel::Debug,
                    stream: LogStream::Stdout,
                    line: content.to_string(),
                    tag: Some("user".into()),
                })));
            }
            other => {
                out.push(self.core.make(EventKind::Log(Log {
                    level: LogLevel::Trace,
                    stream: LogStream::Stdout,
                    line: format!("[{other}] {content}"),
                    tag: Some(format!("role:{other}")),
                })));
            }
        }
    }

    fn handle_tool_use(&mut self, line_value: &Value, out: &mut Vec<Event>) {
        self.core.ensure_started(out);

        let tool_name = line_value
            .get("tool_name")
            .and_then(|v| v.as_str())
            .unwrap_or("");
        let parameters = line_value.get("parameters").cloned().unwrap_or(Value::Null);

        // File-touching tools surface as FileActivity events.
        let file_path = parameters
            .get("file_path")
            .or_else(|| parameters.get("path"))
            .and_then(|v| v.as_str())
            .map(str::to_string);

        match tool_name {
            "read_file" | "Read" => {
                if let Some(path) = file_path {
                    out.push(self.core.make(EventKind::FileActivity(FileActivity {
                        path,
                        op: FileOp::Read,
                        from_path: None,
                        lines_added: None,
                        lines_removed: None,
                        bytes: None,
                    })));
                }
            }
            "write_file" | "Write" => {
                if let Some(path) = file_path {
                    out.push(self.core.make(EventKind::FileActivity(FileActivity {
                        path,
                        op: FileOp::Created,
                        from_path: None,
                        lines_added: None,
                        lines_removed: None,
                        bytes: None,
                    })));
                }
            }
            "edit_file" | "replace" | "Edit" | "MultiEdit" => {
                if let Some(path) = file_path {
                    out.push(self.core.make(EventKind::FileActivity(FileActivity {
                        path,
                        op: FileOp::Modified,
                        from_path: None,
                        lines_added: None,
                        lines_removed: None,
                        bytes: None,
                    })));
                }
            }
            "run_shell_command" | "Bash" => {
                let command = parameters
                    .get("command")
                    .and_then(|v| v.as_str())
                    .map(str::to_string)
                    .unwrap_or_else(|| "(no command)".into());
                out.push(self.core.make(EventKind::Log(Log {
                    level: LogLevel::Info,
                    stream: LogStream::Stdout,
                    line: format!("shell: {command}"),
                    tag: Some("tool:shell".into()),
                })));
            }
            "update_topic" => {
                // Gemini's planning meta-tool. Surface the strategic
                // intent if present so users see the plan in the feed.
                let intent = parameters
                    .get("strategic_intent")
                    .and_then(|v| v.as_str())
                    .unwrap_or("");
                out.push(self.core.make(EventKind::Log(Log {
                    level: LogLevel::Info,
                    stream: LogStream::Stdout,
                    line: if intent.is_empty() {
                        "update_topic".into()
                    } else {
                        format!("plan: {intent}")
                    },
                    tag: Some("tool:plan".into()),
                })));
            }
            other if !other.is_empty() => {
                out.push(self.core.make(EventKind::Log(Log {
                    level: LogLevel::Info,
                    stream: LogStream::Stdout,
                    line: format!("tool: {other}"),
                    tag: Some(format!("tool:{other}")),
                })));
            }
            _ => {}
        }
    }

    fn handle_tool_result(&mut self, line_value: &Value, out: &mut Vec<Event>) {
        self.core.ensure_started(out);

        let status = line_value
            .get("status")
            .and_then(|v| v.as_str())
            .unwrap_or("");
        let level = if status == "error" {
            LogLevel::Warn
        } else {
            LogLevel::Debug
        };
        let tool_id = line_value
            .get("tool_id")
            .and_then(|v| v.as_str())
            .unwrap_or("?");

        out.push(self.core.make(EventKind::Log(Log {
            level,
            stream: LogStream::Stdout,
            line: format!("tool_result {tool_id} ({status})"),
            tag: Some("tool_result".into()),
        })));
    }

    fn handle_result(&mut self, line_value: &Value, out: &mut Vec<Event>) {
        // Guard against duplicate terminal events when the vendor
        // emits a second `result` line after the first (malformed but
        // observed in the wild). Without this, we'd emit a second
        // Cost + Completion/Failure + StateChange pair with a
        // desynchronised seq, leaving the worker tile in an
        // inconsistent UI state.
        if self.core.terminal_emitted {
            return;
        }
        self.core.ensure_started(out);

        // Push usage/cost first so the per-worker cost count is
        // accurate before the terminal state event fires.
        if let Some(cost) = self.parse_cost(line_value) {
            out.push(self.core.make(EventKind::Cost(cost)));
        }

        let status = line_value
            .get("status")
            .and_then(|v| v.as_str())
            .unwrap_or("success");

        if status == "error" {
            let detail = line_value
                .get("error")
                .and_then(|v| v.as_str())
                .map(str::to_string)
                .unwrap_or_else(|| "gemini run failed".into());

            // S5: quota detection. RESOURCE_EXHAUSTED is Gemini's
            // canonical quota code; HTTP 429 is the equivalent.
            let is_quota = is_gemini_quota_signal(&detail);
            if is_quota {
                self.core.pending_quota_signal = Some(adapter_core::QuotaSignal {
                    estimated_reset_at_ms: None,
                });
            }

            // Quota exhaustion is NOT a retryable task-logic failure:
            // pretending so makes the UI offer "retry" and any future
            // policy code that gates on `retryable` re-fires the wall.
            // The pause path is driven by the quota signal above, so the
            // Failure event carries the matching RateLimit category and
            // an explicit retryable=false.
            let (retryable, category) = if is_quota {
                (false, FailureCategory::RateLimit)
            } else {
                (true, FailureCategory::TaskLogic)
            };
            out.push(self.core.make(EventKind::Failure(FailureEv {
                error: detail,
                retryable,
                suggestion: None,
                exit_code: None,
                category: Some(category),
            })));
            out.push(self.core.make(EventKind::StateChange(StateChange {
                state: WorkerState::Failed,
                from: Some(WorkerState::Executing),
                note: None,
            })));
            self.core.terminal_emitted = true;
        } else {
            let summary = {
                let trimmed = self.accumulated_text.trim();
                if trimmed.is_empty() {
                    "gemini completed".into()
                } else {
                    trimmed.to_string()
                }
            };
            let duration_ms = line_value
                .get("stats")
                .and_then(|v| v.get("duration_ms"))
                .and_then(|v| v.as_u64());
            out.push(self.core.make(EventKind::Completion(Completion {
                summary,
                artifacts: None,
                duration_ms,
            })));
            out.push(self.core.make(EventKind::StateChange(StateChange {
                state: WorkerState::Done,
                from: Some(WorkerState::Executing),
                note: None,
            })));
            self.core.terminal_emitted = true;
        }
    }

    /// Parse the `stats` block from a result line into a Cost event.
    /// Gemini's stream-json doesn't include a USD price, so usd is 0.0
    /// (the Settings panel can show "—" for unknown). Token counts and
    /// model name come directly from the parsed stream.
    fn parse_cost(&self, line_value: &Value) -> Option<Cost> {
        let stats = line_value.get("stats")?;
        let input_tokens = stats
            .get("input_tokens")
            .and_then(|v| v.as_u64())
            .unwrap_or(0);
        let output_tokens = stats
            .get("output_tokens")
            .and_then(|v| v.as_u64())
            .unwrap_or(0);
        let cache_read_tokens = stats.get("cached").and_then(|v| v.as_u64());
        // Pick the model with the most tokens used as the headline name.
        let model = stats
            .get("models")
            .and_then(|v| v.as_object())
            .and_then(|m| {
                m.iter()
                    .max_by_key(|(_, v)| {
                        v.get("total_tokens").and_then(|t| t.as_u64()).unwrap_or(0)
                    })
                    .map(|(name, _)| name.clone())
            });
        Some(Cost {
            input_tokens,
            output_tokens,
            usd: 0.0,
            cache_read_tokens,
            cache_write_tokens: None,
            model,
        })
    }
}

#[derive(Deserialize)]
struct LineEnvelope<'a> {
    #[serde(rename = "type")]
    line_type: &'a str,
}

impl Adapter for GeminiAdapter {
    fn ingest_line(&mut self, line: &str, stream: LogStream) -> Vec<Event> {
        let trimmed = line.trim_end_matches(['\r', '\n']);
        if trimmed.is_empty() {
            return Vec::new();
        }

        // Surface raw stderr lines as warn-level logs.
        if matches!(stream, LogStream::Stderr) {
            let mut out = Vec::new();
            self.core.ensure_idle(&mut out);
            // S5: stderr quota messages.
            if is_gemini_quota_signal(trimmed) {
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

        let line_type = match serde_json::from_str::<LineEnvelope>(trimmed) {
            Ok(env) => env.line_type.to_string(),
            Err(_) => value
                .get("type")
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .to_string(),
        };

        match line_type.as_str() {
            "init" => self.handle_init(&value, &mut out),
            "message" => self.handle_message(&value, &mut out),
            "tool_use" => self.handle_tool_use(&value, &mut out),
            "tool_result" => self.handle_tool_result(&value, &mut out),
            "result" => self.handle_result(&value, &mut out),
            "" => {
                // Unrecognized JSON shape — preserve as a trace log.
                out.push(self.core.make(EventKind::Log(Log {
                    level: LogLevel::Trace,
                    stream: LogStream::Stdout,
                    line: trimmed.to_string(),
                    tag: None,
                })));
            }
            other => {
                // Forward-compat: future Gemini stream-json types.
                out.push(self.core.make(EventKind::Log(Log {
                    level: LogLevel::Trace,
                    stream: LogStream::Stdout,
                    line: format!("type={other}"),
                    tag: Some(format!("gemini:{other}")),
                })));
            }
        }
        out
    }

    fn finalize(&mut self, exit: AdapterExit) -> Vec<Event> {
        // Already emitted a terminal state from a `result` line.
        if self.core.terminal_emitted {
            return Vec::new();
        }
        let mut out = Vec::new();
        // Stamp the §5-mandated initial idle if no line ever arrived.
        self.core.ensure_idle(&mut out);
        // The state we transition FROM in the synthesized terminal:
        // executing if we saw any content, idle otherwise.
        let from_state = if self.core.started {
            WorkerState::Executing
        } else {
            WorkerState::Idle
        };
        match exit {
            AdapterExit::Clean => {
                let summary = {
                    let trimmed = self.accumulated_text.trim();
                    if trimmed.is_empty() {
                        if self.core.started {
                            "gemini exited without summary".to_string()
                        } else {
                            "gemini exited before producing output".to_string()
                        }
                    } else {
                        trimmed.to_string()
                    }
                };
                out.push(self.core.make(EventKind::Completion(Completion {
                    summary,
                    artifacts: None,
                    duration_ms: None,
                })));
                out.push(self.core.make(EventKind::StateChange(StateChange {
                    state: WorkerState::Done,
                    from: Some(from_state),
                    note: None,
                })));
            }
            AdapterExit::Failed { code } => {
                out.push(self.core.make(EventKind::Failure(FailureEv {
                    error: match code {
                        Some(c) => format!("gemini exited with code {c}"),
                        None => "gemini exited with non-zero status".into(),
                    },
                    retryable: false,
                    suggestion: None,
                    exit_code: code,
                    category: Some(FailureCategory::Internal),
                })));
                out.push(self.core.make(EventKind::StateChange(StateChange {
                    state: WorkerState::Failed,
                    from: Some(from_state),
                    note: None,
                })));
            }
            AdapterExit::Killed => {
                out.push(self.core.make(EventKind::Failure(FailureEv {
                    error: "gemini killed before completion".into(),
                    retryable: false,
                    suggestion: None,
                    exit_code: None,
                    category: Some(FailureCategory::Internal),
                })));
                out.push(self.core.make(EventKind::StateChange(StateChange {
                    state: WorkerState::Failed,
                    from: Some(from_state),
                    note: Some("worker stopped".into()),
                })));
            }
        }
        self.core.terminal_emitted = true;
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

fn is_gemini_quota_signal(detail: &str) -> bool {
    // Delegates to the shared, token-anchored matcher so RESOURCE_EXHAUSTED
    // / HTTP 429 detection is uniform across vendors and a stray "429" in a
    // filename or "resource_exhausted_path" identifier doesn't false-pause.
    adapter_core::is_quota_exhaustion_line(detail)
}

#[cfg(test)]
mod quota_tests {
    use super::*;
    use adapter_core::Adapter;
    use event_schema::LogStream;

    #[test]
    fn result_with_resource_exhausted_emits_quota_signal() {
        let mut a = GeminiAdapter::new("w1", None);
        let line = r#"{"type":"result","status":"error","error":"RESOURCE_EXHAUSTED: quota exceeded for the day","stats":{"input_tokens":0,"output_tokens":0}}"#;
        let events = a.ingest_line(line, LogStream::Stdout);
        assert!(a.take_quota_signal().is_some());
        // A quota-exhaustion Failure must NOT be marked retryable, and
        // its category must be RateLimit so any caller routing on
        // category sees the right kind. The earlier shape (retryable=true,
        // TaskLogic) re-fired the wall on every UI-driven retry.
        let failure = events
            .iter()
            .find_map(|e| match &e.kind {
                EventKind::Failure(f) => Some(f),
                _ => None,
            })
            .expect("expected a Failure event for the quota error");
        assert!(!failure.retryable, "quota failures must not be retryable");
        assert_eq!(failure.category, Some(FailureCategory::RateLimit));
    }

    #[test]
    fn ordinary_error_stays_retryable_with_task_logic_category() {
        let mut a = GeminiAdapter::new("w1", None);
        let line = r#"{"type":"result","status":"error","error":"tool execution failed"}"#;
        let events = a.ingest_line(line, LogStream::Stdout);
        let failure = events
            .iter()
            .find_map(|e| match &e.kind {
                EventKind::Failure(f) => Some(f),
                _ => None,
            })
            .expect("expected a Failure event for the task-logic error");
        assert!(failure.retryable, "non-quota errors stay retryable");
        assert_eq!(failure.category, Some(FailureCategory::TaskLogic));
    }

    #[test]
    fn result_with_429_in_error_emits_quota_signal() {
        let mut a = GeminiAdapter::new("w1", None);
        let line = r#"{"type":"result","status":"error","error":"HTTP 429: rate limit exceeded"}"#;
        let _ = a.ingest_line(line, LogStream::Stdout);
        assert!(a.take_quota_signal().is_some());
    }

    #[test]
    fn stderr_quota_message_emits_signal() {
        let mut a = GeminiAdapter::new("w1", None);
        let line = "Error: Quota exceeded. Try again later.";
        let _ = a.ingest_line(line, LogStream::Stderr);
        assert!(a.take_quota_signal().is_some());
    }

    #[test]
    fn ordinary_error_does_not_emit_quota_signal() {
        let mut a = GeminiAdapter::new("w1", None);
        let line = r#"{"type":"result","status":"error","error":"tool execution failed"}"#;
        let _ = a.ingest_line(line, LogStream::Stdout);
        assert!(a.take_quota_signal().is_none());
    }

    #[test]
    fn take_quota_signal_is_drained() {
        let mut a = GeminiAdapter::new("w1", None);
        let line = r#"{"type":"result","status":"error","error":"RESOURCE_EXHAUSTED"}"#;
        a.ingest_line(line, LogStream::Stdout);
        assert!(a.take_quota_signal().is_some());
        assert!(a.take_quota_signal().is_none());
    }

    #[test]
    fn benign_429_in_stderr_does_not_emit_signal() {
        // Regression (C3): a stray "429" in a filename / log line must not
        // trip a quota pause for the vendor's full reset window.
        let mut a = GeminiAdapter::new("w1", None);
        let _ = a.ingest_line("wrote build/artifact-429.bin", LogStream::Stderr);
        assert!(a.take_quota_signal().is_none());
    }

    #[test]
    fn accumulated_completion_text_is_bounded() {
        let mut adapter = GeminiAdapter::new("w1", None);
        let line = serde_json::json!({
            "type": "message",
            "role": "assistant",
            "content": "x".repeat(MAX_ACCUMULATED_TEXT_BYTES * 2)
        });
        let _ = adapter.ingest_line(&line.to_string(), LogStream::Stdout);
        assert!(adapter.accumulated_text.len() <= MAX_ACCUMULATED_TEXT_BYTES);
    }
}
