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
    /// Every built-in script, in presentation order. `from_name`, the
    /// [`ScriptError::UnknownScript`] listing, and `mock-harness --help`
    /// all read this one array, so they cannot disagree about what the
    /// harness accepts. A variant missing from here is unreachable from
    /// the CLI rather than silently absent from one of three lists.
    pub const ALL: &'static [Self] = &[
        Self::ClaudeHappy,
        Self::CodexBlocked,
        Self::GeminiHappy,
        Self::GeminiBlocked,
        Self::GeminiFailed,
        Self::GeminiTerminal,
        Self::ClaudeQuotaExhausted,
    ];

    /// Parse the `--script` argv value.
    pub fn from_name(name: &str) -> Result<Self, ScriptError> {
        Self::ALL
            .iter()
            .copied()
            .find(|script| script.name() == name)
            .ok_or_else(|| ScriptError::UnknownScript(name.to_owned()))
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

    /// One-line trajectory summary, rendered by `mock-harness --help`.
    pub fn description(&self) -> &'static str {
        match self {
            Self::ClaudeHappy => "planning → executing → reviewing → done",
            Self::CodexBlocked => "executing → blocked → executing → done",
            Self::GeminiHappy => "planning → executing → reviewing → done (gemini-flavored)",
            Self::GeminiBlocked => "executing → blocked → executing → done (gemini-flavored)",
            Self::GeminiFailed => "executing → failed (retryable: true)",
            Self::GeminiTerminal => "executing → failed (retryable: false)",
            Self::ClaudeQuotaExhausted => "executing → quota-exhausted (Claude rate_limit_event)",
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
            Self::UnknownScript(name) => {
                write!(f, "unknown script {name:?} (expected: ")?;
                for (i, script) in Script::ALL.iter().enumerate() {
                    if i > 0 {
                        write!(f, ", ")?;
                    }
                    write!(f, "{}", script.name())?;
                }
                write!(f, ")")
            }
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_listed_script_parses_back_from_its_own_name() {
        for script in Script::ALL {
            assert_eq!(Script::from_name(script.name()).unwrap(), *script);
        }
    }

    #[test]
    fn the_unknown_script_error_names_every_listed_script() {
        let message = ScriptError::UnknownScript("nope".into()).to_string();
        for script in Script::ALL {
            assert!(
                message.contains(script.name()),
                "{message} omits {}",
                script.name()
            );
        }
    }
}
