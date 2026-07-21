//! Vigla adapter contract.
//!
//! The **only** abstraction Vigla permits is the event boundary
//! (see `ARCHITECTURE.md`, "Adapter Boundary"). This crate hosts the
//! `Adapter` trait that every
//! vendor-specific adapter implements: a stateful, line-at-a-time
//! function from raw bytes (whatever the vendor CLI emits) to
//! canonical [`event_schema::Event`]s.
//!
//! Adapters are pure (modulo their internal mutable state for
//! cross-line context). They never spawn processes, do I/O, or know
//! about persistence — Step 5's supervision pipeline owns those
//! concerns. An adapter receives raw lines and returns
//! `Vec<Event>` — that's the entire contract.

#![deny(missing_debug_implementations)]

pub mod core;
pub mod memory_intent;
pub mod quota_signal;
pub mod raw_log_adapter;

pub use core::AdapterCore;
pub use memory_intent::{extract_intents, parse_line, MemoryIntent, ProposeIntent, ScopeIntent};
pub use quota_signal::is_quota_exhaustion_line;
pub use raw_log_adapter::RawLogAdapter;

use event_schema::{Event, LogStream};

/// Append text while retaining only the newest `max_bytes` on UTF-8
/// boundaries. Returns `true` when older content was discarded.
///
/// Streaming CLI adapters use this for summary/context buffers: individual
/// input lines are bounded by the orchestrator, but a long-running process can
/// still emit an unbounded number of valid lines.
pub fn append_bounded_tail(target: &mut String, fragment: &str, max_bytes: usize) -> bool {
    if max_bytes == 0 {
        let truncated = !target.is_empty() || !fragment.is_empty();
        target.clear();
        return truncated;
    }
    if fragment.len() >= max_bytes {
        let mut start = fragment.len() - max_bytes;
        while start < fragment.len() && !fragment.is_char_boundary(start) {
            start += 1;
        }
        let truncated = !target.is_empty() || start > 0;
        target.clear();
        target.push_str(&fragment[start..]);
        return truncated;
    }

    target.push_str(fragment);
    if target.len() <= max_bytes {
        return false;
    }
    let mut remove = target.len() - max_bytes;
    while remove < target.len() && !target.is_char_boundary(remove) {
        remove += 1;
    }
    target.drain(..remove);
    true
}

/// How the supervised child process ended. Passed to
/// [`Adapter::finalize`] so adapters can emit the right terminal event
/// when the natural CLI flow didn't get a chance to (process killed
/// by user-stop, crashed, exited non-zero).
///
/// Without this signal, ClaudeAdapter would leave the worker stuck in
/// `executing` after a kill, and CodexAdapter would emit a clean
/// `completion` + `done` even when the process was killed or failed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AdapterExit {
    /// Child exited with status 0.
    Clean,
    /// Child exited with a non-zero status code.
    Failed { code: Option<i32> },
    /// Child was killed by signal (SIGKILL from supervisor stop, or
    /// any other signal).
    Killed,
}

/// Adapter trait. One method, called once per line of the vendor
/// CLI's stdout or stderr.
///
/// Returning an empty vec is valid (the line was structural noise or
/// requires accumulating context). Returning multiple events is valid
/// (one wire-line might map to several canonical events: e.g. a
/// stream-json `result` chunk produces both `state_change → done`
/// and `completion`).
///
/// The adapter MUST stamp envelope fields (`schema_version`,
/// `worker_id`, `task_id`, `seq`, `ts`) on every emitted event. Seq
/// is the adapter's own monotonic counter — it does NOT have to
/// match anything from the upstream CLI.
pub trait Adapter: Send + Sync + std::fmt::Debug {
    fn ingest_line(&mut self, line: &str, stream: LogStream) -> Vec<Event>;

    /// Optional hook called when the child process closes its stdout
    /// (EOF) or is terminated. Adapters use `exit` to decide whether
    /// to synthesize a terminal event when the CLI's natural flow
    /// didn't emit one (kill, non-zero exit, etc.). Default returns
    /// nothing — appropriate for adapters that always emit a terminal
    /// state during ingest_line.
    fn finalize(&mut self, exit: AdapterExit) -> Vec<Event> {
        let _ = exit;
        Vec::new()
    }

    /// Optional hook returning the vendor CLI's session identifier as
    /// soon as the adapter has seen it. The supervisor calls this
    /// after each `ingest_line` and persists the first `Some` it
    /// receives, then never asks again. Adapters that have not yet
    /// observed an init / session-started line return `None`. The
    /// supervisor uses the captured id later to spawn `--resume`
    /// follow-ups (Step 25). Default returns `None` — appropriate for
    /// vendors that do not expose a resume primitive.
    fn take_session_id(&mut self) -> Option<String> {
        None
    }

    /// Drain any [`MemoryIntent`]s the adapter has accumulated since
    /// the last call. Called by the orchestrator after each
    /// `ingest_line` so memory proposals route to the kernel in the
    /// same turn the worker emitted them.
    ///
    /// Default returns empty: adapters that don't speak the
    /// `vigla_memory` protocol simply opt out. Vendor adapters
    /// override to scan extracted assistant text against the shared
    /// [`memory_intent::extract_intents`] parser and accumulate the
    /// results between calls.
    ///
    /// Implementations must clear their internal buffer on every
    /// call — the orchestrator depends on each intent being delivered
    /// exactly once.
    fn take_memory_intents(&mut self) -> Vec<MemoryIntent> {
        Vec::new()
    }

    /// Drain any [`QuotaSignal`] the adapter has accumulated since
    /// the last call. The supervisor's recovery engine consumes
    /// these to pause affected workers (see roadmap §2 / S5).
    ///
    /// Default returns `None`: adapters that don't parse vendor
    /// quota errors simply opt out. Real-CLI adapters
    /// (claude/codex/gemini) override.
    ///
    /// Implementations must clear the internal buffer on every
    /// call.
    fn take_quota_signal(&mut self) -> Option<QuotaSignal> {
        None
    }

    /// Drain any [`ContextRequestSignal`]s the worker has emitted
    /// since the last call. Informational only — workers do not
    /// block on these. See S5 §4.6.
    fn take_context_requests(&mut self) -> Vec<ContextRequestSignal> {
        Vec::new()
    }
}

/// Worker-level signal that the vendor's quota window has closed.
/// Adapters parse the vendor-specific error format and emit this
/// canonical shape; the supervisor consumes it via the recovery
/// engine. `estimated_reset_at_ms` is `None` when the adapter
/// detected exhaustion but cannot extract a reset timestamp —
/// the supervisor's `VendorQuotaTracker` will fall back to the
/// vendor's configured window.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct QuotaSignal {
    pub estimated_reset_at_ms: Option<u64>,
}

/// Worker-side informational signal: the worker is missing context.
/// Not blocking — the worker continues with what it has. The
/// supervisor catches up async via the recovery engine's
/// `RequestSupervisor` action.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ContextRequestSignal {
    pub kind: ContextRequestSignalKind,
    pub detail: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ContextRequestSignalKind {
    FileContent,
    Documentation,
    PriorDecision,
}

#[cfg(test)]
mod adapter_default_tests {
    use super::*;
    use event_schema::{Event, LogStream};

    #[derive(Debug)]
    struct NoopAdapter;
    impl Adapter for NoopAdapter {
        fn ingest_line(&mut self, _line: &str, _stream: LogStream) -> Vec<Event> {
            Vec::new()
        }
    }

    #[test]
    fn default_take_quota_signal_is_none() {
        let mut a = NoopAdapter;
        assert!(a.take_quota_signal().is_none());
    }

    #[test]
    fn default_take_context_requests_is_empty() {
        let mut a = NoopAdapter;
        assert!(a.take_context_requests().is_empty());
    }

    #[test]
    fn bounded_tail_preserves_utf8_and_latest_content() {
        let mut text = "prefix-🙂".to_string();
        assert!(append_bounded_tail(&mut text, "-latest", 10));
        assert!(text.len() <= 10);
        assert!(text.ends_with("latest"));
        assert!(std::str::from_utf8(text.as_bytes()).is_ok());
    }
}
