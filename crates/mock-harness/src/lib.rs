//! Mock Vigla worker harness.
//!
//! Emits scripted JSONL event trajectories matching the canonical
//! `event-schema` contract before any real vendor CLI is integrated.
//! Pure: scripts are deterministic functions of (`Script`, [`EmitOpts`]).
//!
//! Used by:
//!   * the `mock-harness` binary, which emits to stdout with pacing,
//!   * the orchestrator's supervision pipeline tests (Step 5+),
//!   * direct in-process consumers (mock-from-mock harness, debug
//!     scripting).

use event_schema::{Event, Vendor};
use std::fmt;

mod scripts;
mod time;

pub use crate::time::rfc3339_from_unix_ms;

/// Built-in script trajectories. Each maps to a public function in the
/// `scripts` module that produces a deterministic event sequence.
///
/// Aider variants were removed in schema 2.0 alongside `Vendor::Aider`.
/// The Gemini variants below replace them: `GeminiHappy` mirrors
/// `ClaudeHappy`, `GeminiBlocked` mirrors `CodexBlocked`, and
/// `GeminiFailed` / `GeminiTerminal` exercise the supervisor's
/// retry-policy gates that the audit-r5 polish round depends on.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Script {
    /// `planning → executing → reviewing → done` — the happy path.
    ClaudeHappy,
    /// `executing → blocked → executing → done` — exercises the
    /// dependency / blocked visual state and unblock recovery.
    CodexBlocked,
    /// Gemini happy-path mirror of `ClaudeHappy`.
    GeminiHappy,
    /// Gemini blocked mirror of `CodexBlocked`.
    GeminiBlocked,
    /// `executing → failed` with `retryable: true` — exercises the
    /// supervisor's retry policy.
    GeminiFailed,
    /// `executing → failed` with `retryable: false` — exercises the
    /// audit-r5 retry gate (worker's retryable=false flag must
    /// override `RetryPolicy::OnFailure`).
    GeminiTerminal,
    /// S5: `executing → quota-exhausted`. Emits a Claude-shaped
    /// `rate_limit_event` with status=exceeded so the
    /// ClaudeAdapter produces a QuotaSignal. The supervisor pauses
    /// the mission and the wake-up task resumes it.
    ClaudeQuotaExhausted,
}

impl Script {
    /// Parse the `--script` argv value.
    pub fn from_name(name: &str) -> Result<Self, ScriptError> {
        match name {
            "claude_happy" => Ok(Self::ClaudeHappy),
            "codex_blocked" => Ok(Self::CodexBlocked),
            "gemini_happy" => Ok(Self::GeminiHappy),
            "gemini_blocked" => Ok(Self::GeminiBlocked),
            "gemini_failed" => Ok(Self::GeminiFailed),
            "gemini_terminal" => Ok(Self::GeminiTerminal),
            "claude_quota_exhausted" => Ok(Self::ClaudeQuotaExhausted),
            other => Err(ScriptError::UnknownScript(other.to_owned())),
        }
    }

    /// The vendor stamped on the worker info this script represents.
    /// Used by Step 5 supervision to record the right vendor name.
    pub fn vendor(&self) -> Vendor {
        match self {
            Self::ClaudeHappy | Self::ClaudeQuotaExhausted => Vendor::Claude,
            Self::CodexBlocked => Vendor::Codex,
            Self::GeminiHappy | Self::GeminiBlocked | Self::GeminiFailed | Self::GeminiTerminal => {
                Vendor::Gemini
            }
        }
    }

    /// Stable string name (matches `from_name`).
    pub fn name(&self) -> &'static str {
        match self {
            Self::ClaudeHappy => "claude_happy",
            Self::CodexBlocked => "codex_blocked",
            Self::GeminiHappy => "gemini_happy",
            Self::GeminiBlocked => "gemini_blocked",
            Self::GeminiFailed => "gemini_failed",
            Self::GeminiTerminal => "gemini_terminal",
            Self::ClaudeQuotaExhausted => "claude_quota_exhausted",
        }
    }
}

/// Errors returned by the public surface.
#[derive(Debug)]
pub enum ScriptError {
    UnknownScript(String),
}

impl fmt::Display for ScriptError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::UnknownScript(name) => write!(
                f,
                "unknown script {name:?} (expected: claude_happy, codex_blocked, gemini_happy, gemini_blocked, gemini_failed, gemini_terminal, claude_quota_exhausted)"
            ),
        }
    }
}

impl std::error::Error for ScriptError {}

/// Inputs for a script run: the synthetic worker_id and task_id stamped
/// on every event, plus the wall-clock anchor used to render `ts`.
#[derive(Debug, Clone)]
pub struct EmitOpts {
    pub worker_id: String,
    pub task_id: String,
    pub start_unix_ms: u64,
}

/// One pre-rendered event plus the delay (ms) the binary should wait
/// **before** emitting it. The first event in a script always has
/// `delay_ms_before == 0`.
#[derive(Debug, Clone)]
pub struct TimedEvent {
    pub event: Event,
    pub delay_ms_before: u64,
}

/// Build the full event sequence for a script. Pure: same inputs always
/// yield byte-identical output (modulo whatever the caller passes for
/// `start_unix_ms`).
pub fn build_script(script: Script, opts: &EmitOpts) -> Vec<TimedEvent> {
    match script {
        Script::ClaudeHappy => scripts::claude_happy(opts),
        Script::CodexBlocked => scripts::codex_blocked(opts),
        Script::GeminiHappy => scripts::gemini_happy(opts),
        Script::GeminiBlocked => scripts::gemini_blocked(opts),
        Script::GeminiFailed => scripts::gemini_failed(opts),
        Script::GeminiTerminal => scripts::gemini_terminal(opts),
        Script::ClaudeQuotaExhausted => scripts::claude_quota_exhausted(opts),
    }
}
