//! Claude Code CLI adapter.
//!
//! Translates Anthropic Claude Code's `--output-format stream-json`
//! output into Vigla canonical events (the `event-schema` crate).
//! Stateful: events arrive line-by-line and require small amounts of
//! cross-line context (current task, seq counter, accumulated
//! assistant text).
//!
//! **This crate does no I/O.** It is invoked by the Step 5
//! supervision pipeline with each stdout line; Step 11 wires it up
//! end-to-end against a real `claude` subprocess. Step 10 (this
//! file) only provides the byte→event function and tests against
//! captured fixtures.

#![deny(missing_debug_implementations)]

use adapter_core::{Adapter, AdapterExit};
use event_schema::{
    ArtifactKind, Completion, Cost, Event, EventKind, Failure as FailureEv, FailureCategory,
    FileActivity, FileOp, Log, LogLevel, LogStream, Progress, StateChange, WorkerState,
};
use serde::Deserialize;
use serde_json::Value;

const MAX_ACCUMULATED_TEXT_BYTES: usize = 256 * 1024;

/// Claude Code stream-json adapter.
///
/// Lifecycle:
/// 1. First emitted event = `state_change → idle` (the adapter
///    contract: every adapter announces the worker before anything
///    else).
/// 2. On the first `system/init` line, emit `state_change →
///    executing` to mark the worker as actively running.
/// 3. `assistant` content (text) → `log` events. Tool-use blocks
///    map to `file_activity` (Read/Edit/Write/MultiEdit) or `log`
///    (Bash/other tools — Step 11+ may add finer mapping).
/// 4. `result/success` → `cost` (with usage), `state_change → done`,
///    `completion`.
/// 5. `result/error` → `failure`, `state_change → failed`.
#[derive(Debug)]
pub struct ClaudeAdapter {
    core: adapter_core::AdapterCore,
    /// Vendor-specific: accumulated assistant text, used to derive the
    /// completion summary when the result line omits one.
    accumulated_text: String,
}

impl ClaudeAdapter {
    pub fn new(worker_id: impl Into<String>, task_id: Option<String>) -> Self {
        Self::with_starting_seq(worker_id, task_id, 0)
    }

    /// Build an adapter that begins numbering events at `starting_seq`.
    /// Used by `spawn_claude_resume`: the resumed run reuses the prior
    /// worker_id but the events table's `(worker_id, seq)` PRIMARY KEY
    /// means a fresh `seq=0..N` would collide with the original run's
    /// persisted rows. `insert_event_raw` silently drops those duplicates
    /// (`InsertOutcome::DuplicateSkipped`) and `persist_and_emit` then
    /// skips the UI emit — so without seeding the seq, every resumed
    /// event vanishes before the user sees it.
    pub fn with_starting_seq(
        worker_id: impl Into<String>,
        task_id: Option<String>,
        starting_seq: u64,
    ) -> Self {
        Self {
            core: adapter_core::AdapterCore::with_starting_seq(worker_id, task_id, starting_seq),
            accumulated_text: String::new(),
        }
    }

    fn handle_system(&mut self, line_value: &Value, out: &mut Vec<Event>) {
        let subtype = line_value
            .get("subtype")
            .and_then(|v| v.as_str())
            .unwrap_or("");
        match subtype {
            "init" => {
                // Promote the worker into the running state on first
                // session-init line.
                self.core.ensure_started(out);
                // Capture the CLI's session id so the supervisor can
                // spawn `claude --resume <id>` follow-ups.
                if self.core.pending_session_id.is_none() {
                    if let Some(sid) = line_value.get("session_id").and_then(|v| v.as_str()) {
                        self.core.pending_session_id = Some(sid.to_owned());
                    }
                }
            }
            // hook_started / hook_response / etc — Claude Code's
            // session lifecycle. We surface them as debug logs so
            // they're visible in the drawer feed but don't drive
            // visible state.
            "hook_started" | "hook_response" | "hook_stopped" | "hook_failed" => {
                let hook_name = line_value
                    .get("hook_name")
                    .and_then(|v| v.as_str())
                    .unwrap_or("hook");
                let detail = line_value
                    .get("detail")
                    .and_then(|v| v.as_str())
                    .unwrap_or("");
                // S5: detect the 5-hour message limit signal that
                // Claude Code emits as a hook response.
                let combined = format!("{hook_name} {detail}").to_ascii_lowercase();
                if combined.contains("5-hour message limit")
                    || combined.contains("message limit reached")
                    || combined.contains("usage limit reached")
                {
                    self.core.pending_quota_signal = Some(adapter_core::QuotaSignal {
                        estimated_reset_at_ms: None,
                    });
                }
                out.push(self.core.make(EventKind::Log(Log {
                    level: LogLevel::Debug,
                    stream: LogStream::Stdout,
                    line: format!("{subtype}: {hook_name}"),
                    tag: Some("hook".into()),
                })));
            }
            _ => {
                // Unknown system subtype — surface as trace log for
                // forward-compat without dropping data.
                out.push(self.core.make(EventKind::Log(Log {
                    level: LogLevel::Trace,
                    stream: LogStream::Stdout,
                    line: format!("system {subtype}"),
                    tag: Some("system".into()),
                })));
            }
        }
    }

    fn handle_assistant(&mut self, line_value: &Value, out: &mut Vec<Event>) {
        self.core.ensure_started(out);

        let Some(message) = line_value.get("message") else {
            return;
        };
        let Some(content) = message.get("content").and_then(|v| v.as_array()) else {
            return;
        };

        for block in content {
            let Some(block_type) = block.get("type").and_then(|v| v.as_str()) else {
                continue;
            };
            match block_type {
                "text" => {
                    if let Some(text) = block.get("text").and_then(|v| v.as_str()) {
                        if text.trim().is_empty() {
                            continue;
                        }
                        // Tier-2D: scan this text block for
                        // `vigla_memory` JSON lines. The parser
                        // is fast on non-matching input (cheap prefix
                        // check) so we run it on every block.
                        self.core
                            .pending_memory_intents
                            .extend(adapter_core::extract_intents(text));
                        adapter_core::append_bounded_tail(
                            &mut self.accumulated_text,
                            text,
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
                            line: text.to_string(),
                            tag: Some("assistant".into()),
                        })));
                    }
                }
                "tool_use" => {
                    self.handle_tool_use(block, out);
                }
                _ => {
                    out.push(self.core.make(EventKind::Log(Log {
                        level: LogLevel::Trace,
                        stream: LogStream::Stdout,
                        line: format!("content block: {block_type}"),
                        tag: Some("content".into()),
                    })));
                }
            }
        }
    }

    fn handle_tool_use(&mut self, block: &Value, out: &mut Vec<Event>) {
        let tool_name = block.get("name").and_then(|v| v.as_str()).unwrap_or("");
        let input = block.get("input").cloned().unwrap_or(Value::Null);

        match tool_name {
            "Read" | "Edit" | "Write" | "MultiEdit" => {
                let path = input
                    .get("file_path")
                    .and_then(|v| v.as_str())
                    .or_else(|| input.get("path").and_then(|v| v.as_str()))
                    .map(str::to_string);
                let op = match tool_name {
                    "Read" => FileOp::Read,
                    "Write" => FileOp::Created,
                    "Edit" | "MultiEdit" => FileOp::Modified,
                    _ => FileOp::Read,
                };
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
            "Bash" => {
                let command = input
                    .get("command")
                    .and_then(|v| v.as_str())
                    .map(str::to_string)
                    .unwrap_or_else(|| "(no command)".into());
                out.push(self.core.make(EventKind::Log(Log {
                    level: LogLevel::Info,
                    stream: LogStream::Stdout,
                    line: format!("bash: {command}"),
                    tag: Some("tool:Bash".into()),
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

    fn handle_result(&mut self, line_value: &Value, out: &mut Vec<Event>) {
        // A terminal state was already emitted — a second `result` line
        // (malformed but observed in the wild) must not re-emit a
        // Cost + Completion/Failure + StateChange with a desynchronised
        // seq, which would leave the worker tile in an inconsistent UI
        // state and claim an invalid `from: Executing` transition out of
        // an already-terminal worker (F-18). Mirrors GeminiAdapter's guard.
        if self.core.terminal_emitted {
            return;
        }
        // Guarantee the worker actually entered `Executing` before we emit
        // a terminal state_change with `from: Executing`. A stream where
        // `result` arrives before any `system/init` would otherwise claim
        // a transition out of a state that was never emitted (F-18).
        self.core.ensure_started(out);
        // Push any usage/cost first so the per-worker cost count is
        // accurate by the time the UI sees the final state_change.
        if let Some(cost) = self.parse_cost(line_value) {
            out.push(self.core.make(EventKind::Cost(cost)));
        }

        let is_error = line_value
            .get("is_error")
            .and_then(|v| v.as_bool())
            .unwrap_or(false);
        let subtype = line_value
            .get("subtype")
            .and_then(|v| v.as_str())
            .unwrap_or("success");

        if is_error || subtype == "error_during_execution" || subtype == "error_max_turns" {
            let api_error = line_value
                .get("api_error_status")
                .and_then(|v| v.as_str())
                .map(str::to_string);
            // S5: 429 is a vendor quota signal, not just a network
            // error. Surface it so the supervisor pauses rather than
            // retries blindly.
            if api_error.as_deref() == Some("429") {
                self.core.pending_quota_signal = Some(adapter_core::QuotaSignal {
                    estimated_reset_at_ms: None,
                });
            }
            let detail = line_value
                .get("result")
                .and_then(|v| v.as_str())
                .map(str::to_string)
                .unwrap_or_else(|| "claude run failed".into());
            // `error_max_turns` is a budget-exceeded condition (the
            // user set --max-turns and Claude ran out before finishing)
            // — semantically identical to a timeout. `Internal` was
            // wrong: that category implies a bug or crash and points
            // operators at the wrong remediation (file a report)
            // instead of the right one (raise the turn limit).
            // 429 is a vendor quota signal — surface as RateLimit so
            // operators (and any UI tier that routes on category) see
            // the right remediation. The quota signal channel above
            // already drives the supervisor's pause path; this is the
            // semantic fix on the visible Failure event.
            let category = if subtype == "error_max_turns" {
                Some(FailureCategory::Timeout)
            } else if api_error.as_deref() == Some("429") {
                Some(FailureCategory::RateLimit)
            } else if api_error.is_some() {
                Some(FailureCategory::Network)
            } else {
                Some(FailureCategory::Unknown)
            };
            out.push(self.core.make(EventKind::Failure(FailureEv {
                error: detail,
                retryable: matches!(subtype, "error_max_turns" | "error_during_execution"),
                suggestion: None,
                exit_code: None,
                category,
            })));
            out.push(self.core.make(EventKind::StateChange(StateChange {
                state: WorkerState::Failed,
                from: Some(WorkerState::Executing),
                note: None,
            })));
            self.core.terminal_emitted = true;
        } else {
            let summary = line_value
                .get("result")
                .and_then(|v| v.as_str())
                .map(str::to_string)
                .unwrap_or_else(|| {
                    let trimmed = self.accumulated_text.trim();
                    if trimmed.is_empty() {
                        "claude run completed".to_string()
                    } else {
                        trimmed.to_string()
                    }
                });
            let duration_ms = line_value.get("duration_ms").and_then(|v| v.as_u64());
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

    fn parse_cost(&self, line_value: &Value) -> Option<Cost> {
        let usage = line_value.get("usage")?;
        let input_tokens = usage.get("input_tokens")?.as_u64()?;
        let output_tokens = usage.get("output_tokens")?.as_u64()?;
        let usd = line_value
            .get("total_cost_usd")
            .and_then(|v| v.as_f64())
            .unwrap_or(0.0);
        let cache_read_tokens = usage
            .get("cache_read_input_tokens")
            .and_then(|v| v.as_u64());
        let cache_write_tokens = usage
            .get("cache_creation_input_tokens")
            .and_then(|v| v.as_u64());
        let model = line_value
            .get("modelUsage")
            .and_then(|v| v.as_object())
            .and_then(|m| m.keys().next().cloned());
        Some(Cost {
            input_tokens,
            output_tokens,
            usd,
            cache_read_tokens,
            cache_write_tokens,
            model,
        })
    }

    fn handle_user(&mut self, _line_value: &Value, out: &mut Vec<Event>) {
        // Tool results return as `user` messages in stream-json. We
        // surface them as info logs so they're visible in the drawer
        // feed.
        self.core.ensure_started(out);
        out.push(self.core.make(EventKind::Log(Log {
            level: LogLevel::Info,
            stream: LogStream::Stdout,
            line: "tool result".to_string(),
            tag: Some("tool_result".into()),
        })));
    }

    fn handle_rate_limit(&mut self, line_value: &Value, out: &mut Vec<Event>) {
        let info = line_value.get("rate_limit_info");
        let status = info
            .and_then(|v| v.get("status"))
            .and_then(|v| v.as_str())
            .unwrap_or("unknown");
        let level = match status {
            "exceeded" | "blocked" => LogLevel::Warn,
            _ => LogLevel::Debug,
        };
        out.push(self.core.make(EventKind::Log(Log {
            level,
            stream: LogStream::Stdout,
            line: format!("rate_limit: {status}"),
            tag: Some("rate_limit".into()),
        })));

        // S5: structural quota signal. exceeded/blocked → emit. The
        // optional `resets_at` is an RFC 3339 string; convert to
        // Unix ms.
        if matches!(status, "exceeded" | "blocked") {
            let reset_ms = info
                .and_then(|v| v.get("resets_at"))
                .and_then(|v| v.as_str())
                .and_then(parse_rfc3339_to_unix_ms);
            self.core.pending_quota_signal = Some(adapter_core::QuotaSignal {
                estimated_reset_at_ms: reset_ms,
            });
        }
    }

    // `maybe_emit_progress` reserved for a later polish pass.
    // Claude stream-json doesn't carry progress %; we'd derive it
    // from `iterations` count vs `max_turns` in a future polish step.
}

#[derive(Deserialize)]
struct LineEnvelope<'a> {
    #[serde(rename = "type")]
    line_type: &'a str,
}

impl Adapter for ClaudeAdapter {
    fn ingest_line(&mut self, line: &str, stream: LogStream) -> Vec<Event> {
        let trimmed = line.trim_end_matches(['\r', '\n']);
        if trimmed.is_empty() {
            return Vec::new();
        }

        // First, surface raw stderr lines as warn-level logs so the
        // drawer terminal tab still shows them. Real claude doesn't
        // typically write to stderr, but adapters should be defensive.
        if matches!(stream, LogStream::Stderr) {
            let mut out = Vec::new();
            self.core.ensure_idle(&mut out);
            out.push(self.core.make(EventKind::Log(Log {
                level: LogLevel::Warn,
                stream: LogStream::Stderr,
                line: trimmed.to_string(),
                tag: None,
            })));
            return out;
        }

        let mut out = Vec::new();
        // Every emitted event sequence must START with state_change
        // → idle (event-schema §5). We emit it eagerly here on the
        // first line of *any* shape — JSON or not, recognized or not.
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
            "system" => self.handle_system(&value, &mut out),
            "assistant" => self.handle_assistant(&value, &mut out),
            "user" => self.handle_user(&value, &mut out),
            "result" => self.handle_result(&value, &mut out),
            "rate_limit_event" => self.handle_rate_limit(&value, &mut out),
            "" => {
                // Unrecognised JSON shape — preserve as a trace log.
                out.push(self.core.make(EventKind::Log(Log {
                    level: LogLevel::Trace,
                    stream: LogStream::Stdout,
                    line: trimmed.to_string(),
                    tag: None,
                })));
            }
            other => {
                // Forward-compat: future Claude Code stream-json
                // chunk types we don't recognise yet.
                out.push(self.core.make(EventKind::Log(Log {
                    level: LogLevel::Trace,
                    stream: LogStream::Stdout,
                    line: format!("type={other}"),
                    tag: Some(format!("claude:{other}")),
                })));
            }
        }
        out
    }

    fn finalize(&mut self, exit: AdapterExit) -> Vec<Event> {
        // If the run never reached `executing` there's no tile state
        // to close out — the supervisor's mark_worker_ended takes
        // care of the row in the database.
        if !self.core.started || self.core.terminal_emitted {
            return Vec::new();
        }
        // The CLI didn't emit a result line before stdout closed.
        // Synthesize a terminal event derived from the child's exit
        // status so the tile doesn't get stuck in `executing`.
        let mut out = Vec::new();
        match exit {
            AdapterExit::Clean => {
                let summary = {
                    let trimmed = self.accumulated_text.trim();
                    if trimmed.is_empty() {
                        "claude exited without summary".to_string()
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
                    from: Some(WorkerState::Executing),
                    note: None,
                })));
            }
            AdapterExit::Failed { code } => {
                out.push(self.core.make(EventKind::Failure(FailureEv {
                    error: match code {
                        Some(c) => format!("claude exited with code {c}"),
                        None => "claude exited with non-zero status".into(),
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
                out.push(self.core.make(EventKind::Failure(FailureEv {
                    error: "claude killed before completion".into(),
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

/// Parse an RFC 3339 timestamp to Unix milliseconds. Best-effort:
/// accepts the subset of forms Claude emits (always UTC `Z`-suffix,
/// optional millisecond fraction). Returns `None` on parse failure
/// so the caller falls back to its default window.
fn parse_rfc3339_to_unix_ms(s: &str) -> Option<u64> {
    // Shape: 2026-05-19T20:00:00Z or 2026-05-19T20:00:00.000Z
    let s = s.trim_end_matches('Z');
    let (date, time) = s.split_once('T')?;
    let mut date_parts = date.split('-');
    let year: i64 = date_parts.next()?.parse().ok()?;
    let month: u32 = date_parts.next()?.parse().ok()?;
    let day: u32 = date_parts.next()?.parse().ok()?;
    let (time, ms_part) = match time.split_once('.') {
        // Vendor may emit sub-millisecond precision (e.g. `.123456`).
        // The schema is milliseconds, so take the first 3 fractional
        // digits and right-pad shorter values with zeros. Parsing the
        // raw fraction as u32 turned `.123456` into 123 456 ms — over
        // two minutes of bogus offset on the quota-reset wake-up.
        Some((t, ms)) => {
            let mut digits = String::with_capacity(3);
            for ch in ms.chars().take(3) {
                if ch.is_ascii_digit() {
                    digits.push(ch);
                } else {
                    break;
                }
            }
            while digits.len() < 3 {
                digits.push('0');
            }
            (t, digits.parse::<u32>().unwrap_or(0))
        }
        None => (time, 0u32),
    };
    let mut time_parts = time.split(':');
    let hour: u32 = time_parts.next()?.parse().ok()?;
    let minute: u32 = time_parts.next()?.parse().ok()?;
    let second: u32 = time_parts.next()?.parse().ok()?;

    // Days since 1970-01-01 (Howard Hinnant's civil-from-days,
    // inverse of the existing `rfc3339_from_unix_ms` formula).
    let m = month as i64;
    let y = if m <= 2 { year - 1 } else { year };
    let era = if y >= 0 { y } else { y - 399 } / 400;
    let yoe = (y - era * 400) as u64;
    let m_shift = if m > 2 { m - 3 } else { m + 9 } as u64;
    let doy = (153 * m_shift + 2) / 5 + day as u64 - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    let days = era * 146_097 + doe as i64 - 719_468;
    if days < 0 {
        return None;
    }
    // Checked arithmetic: `year` is parsed with no upper bound, and a
    // malformed vendor `resets_at` with an absurd year (≥ ~585 million)
    // would overflow `days * 86_400 * 1000` — a debug panic / release wrap
    // into a garbage reset timestamp. On overflow, return None so the
    // caller falls back to its default quota window.
    let secs_of_day = hour as u64 * 3600 + minute as u64 * 60 + second as u64;
    let total_secs = (days as u64)
        .checked_mul(86_400)?
        .checked_add(secs_of_day)?;
    total_secs.checked_mul(1000)?.checked_add(ms_part as u64)
}

// Suppress unused-warning for the artifact kind import — reserved
// for future stream-json result.artifacts handling.
const _: ArtifactKind = ArtifactKind::File;
const _: Progress = Progress {
    percent: 0.0,
    eta_ms: None,
    note: None,
};

#[cfg(test)]
mod quota_tests {
    use super::*;
    use adapter_core::Adapter;
    use event_schema::LogStream;

    #[test]
    fn rate_limit_event_with_status_exceeded_emits_quota_signal() {
        let mut a = ClaudeAdapter::new("w1", None);
        let line = r#"{"type":"rate_limit_event","rate_limit_info":{"status":"exceeded","resets_at":"2026-05-19T20:00:00Z"}}"#;
        let _events = a.ingest_line(line, LogStream::Stdout);
        let sig = a.take_quota_signal();
        assert!(
            sig.is_some(),
            "expected quota signal on rate_limit exceeded"
        );
        // Reset is supplied as RFC 3339 → ms; the adapter converts.
        // We don't assert the exact ms value here (timezone fragile)
        // but it must be non-zero.
        assert!(sig.unwrap().estimated_reset_at_ms.unwrap_or(0) > 0);
    }

    #[test]
    fn rate_limit_event_with_status_ok_does_not_emit() {
        let mut a = ClaudeAdapter::new("w1", None);
        let line = r#"{"type":"rate_limit_event","rate_limit_info":{"status":"ok"}}"#;
        let _events = a.ingest_line(line, LogStream::Stdout);
        assert!(a.take_quota_signal().is_none());
    }

    #[test]
    fn result_line_with_429_emits_quota_signal_without_reset() {
        let mut a = ClaudeAdapter::new("w1", None);
        let line = r#"{"type":"result","is_error":true,"api_error_status":"429","result":"rate limit","usage":{"input_tokens":0,"output_tokens":0}}"#;
        let events = a.ingest_line(line, LogStream::Stdout);
        let sig = a.take_quota_signal().unwrap();
        // 429 with no resets_at → adapter cannot extract a time;
        // emits None and the supervisor tracker falls back.
        assert!(sig.estimated_reset_at_ms.is_none());
        // The Failure event must carry RateLimit (not Network) so any
        // UI/recovery code that routes on category sees the right kind.
        let failure_cat = events.iter().find_map(|e| match &e.kind {
            EventKind::Failure(f) => f.category,
            _ => None,
        });
        assert_eq!(failure_cat, Some(FailureCategory::RateLimit));
    }

    #[test]
    fn result_before_init_emits_executing_before_terminal() {
        // F-18: a stream where `result` arrives with no prior system/init
        // must still emit Executing before the terminal Done state_change,
        // so the `from: Executing` transition reflects a state that was
        // actually emitted.
        let mut a = ClaudeAdapter::new("w1", None);
        let line = r#"{"type":"result","subtype":"success","result":"ok","duration_ms":5}"#;
        let events = a.ingest_line(line, LogStream::Stdout);

        let states: Vec<WorkerState> = events
            .iter()
            .filter_map(|e| match &e.kind {
                EventKind::StateChange(sc) => Some(sc.state),
                _ => None,
            })
            .collect();
        assert_eq!(
            states,
            vec![WorkerState::Idle, WorkerState::Executing, WorkerState::Done],
            "result-first stream must open idle→executing before the terminal Done"
        );

        let done_from = events
            .iter()
            .find_map(|e| match &e.kind {
                EventKind::StateChange(sc) if sc.state == WorkerState::Done => Some(sc.from),
                _ => None,
            })
            .flatten();
        assert_eq!(done_from, Some(WorkerState::Executing));
    }

    #[test]
    fn second_result_line_does_not_emit_duplicate_terminal() {
        // A malformed stream with two `result` lines (observed in the
        // wild) must emit exactly one terminal sequence; the second line
        // is fully ignored so the worker tile doesn't desync. Regression
        // for the missing `terminal_emitted` guard in handle_result.
        let mut a = ClaudeAdapter::new("w1", None);
        let first = a.ingest_line(
            r#"{"type":"result","subtype":"success","result":"ok","duration_ms":5}"#,
            LogStream::Stdout,
        );
        let second = a.ingest_line(
            r#"{"type":"result","subtype":"success","result":"ok again","duration_ms":6}"#,
            LogStream::Stdout,
        );

        let done_count = |evs: &[Event]| {
            evs.iter()
                .filter(|e| {
                    matches!(&e.kind, EventKind::StateChange(sc) if sc.state == WorkerState::Done)
                })
                .count()
        };
        assert_eq!(
            done_count(&first),
            1,
            "first result emits one terminal Done"
        );
        assert!(
            second.is_empty(),
            "a second result line must emit nothing, got {second:?}"
        );
    }

    #[test]
    fn take_quota_signal_is_drained() {
        let mut a = ClaudeAdapter::new("w1", None);
        let line = r#"{"type":"rate_limit_event","rate_limit_info":{"status":"exceeded","resets_at":"2026-05-19T20:00:00Z"}}"#;
        a.ingest_line(line, LogStream::Stdout);
        assert!(a.take_quota_signal().is_some());
        assert!(
            a.take_quota_signal().is_none(),
            "second call should be empty"
        );
    }

    #[test]
    fn parse_rfc3339_no_fraction_returns_zero_ms() {
        let ms = parse_rfc3339_to_unix_ms("2026-05-19T20:00:00Z").unwrap();
        assert_eq!(ms % 1000, 0);
    }

    #[test]
    fn parse_rfc3339_three_digit_fraction_is_taken_verbatim() {
        let ms = parse_rfc3339_to_unix_ms("2026-05-19T20:00:00.123Z").unwrap();
        assert_eq!(ms % 1000, 123);
    }

    #[test]
    fn parse_rfc3339_six_digit_fraction_truncates_to_three() {
        // Regression: vendors that emit microsecond precision (.123456)
        // used to inject the raw 123_456 into ms, putting the reset
        // wake-up over two minutes past the real time.
        let ms = parse_rfc3339_to_unix_ms("2026-05-19T20:00:00.123456Z").unwrap();
        assert_eq!(ms % 1000, 123);
    }

    #[test]
    fn parse_rfc3339_short_fraction_right_pads_with_zeros() {
        // `.5` is 500 ms, not 5 ms.
        let ms = parse_rfc3339_to_unix_ms("2026-05-19T20:00:00.5Z").unwrap();
        assert_eq!(ms % 1000, 500);
        let ms = parse_rfc3339_to_unix_ms("2026-05-19T20:00:00.05Z").unwrap();
        assert_eq!(ms % 1000, 50);
    }

    #[test]
    fn parse_rfc3339_absurd_year_returns_none_not_panic() {
        // A malformed vendor `resets_at` with an astronomically large year
        // must not overflow the ms computation (a debug panic / release
        // wrap) — it returns None so the caller uses its default window.
        assert_eq!(parse_rfc3339_to_unix_ms("999999999-01-01T00:00:00Z"), None);
        // A normal year still parses.
        assert!(parse_rfc3339_to_unix_ms("2026-05-19T20:00:00Z").is_some());
    }

    #[test]
    fn five_hour_limit_message_in_log_emits_quota_signal() {
        let mut a = ClaudeAdapter::new("w1", None);
        // Real-world example: Claude Code emits this as a log line
        // when the 5-hour message limit hits even outside the
        // structural rate_limit_event channel.
        let line = r#"{"type":"system","subtype":"hook_response","hook_name":"approaching_limit","detail":"5-hour message limit reached"}"#;
        let _ = a.ingest_line(line, LogStream::Stdout);
        let sig = a.take_quota_signal();
        assert!(
            sig.is_some(),
            "expected quota signal on 5-hour message limit"
        );
    }

    #[test]
    fn accumulated_completion_text_is_bounded() {
        let mut adapter = ClaudeAdapter::new("w1", None);
        let line = serde_json::json!({
            "type": "assistant",
            "message": { "content": [{
                "type": "text",
                "text": "x".repeat(MAX_ACCUMULATED_TEXT_BYTES * 2)
            }]}
        });
        let _ = adapter.ingest_line(&line.to_string(), LogStream::Stdout);
        assert!(adapter.accumulated_text.len() <= MAX_ACCUMULATED_TEXT_BYTES);
    }
}
