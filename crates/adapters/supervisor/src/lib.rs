//! Mission supervisor adapter.
//!
//! Translates `claude -p --output-format stream-json` output into
//! `SupervisorIntent` values — the contract by which a real Claude CLI
//! drives `MissionRuntime`. Pure: no I/O, no process spawning. The
//! orchestrator spawns `claude`, line-streams its stdout, and feeds
//! each line through [`SupervisorAdapter::ingest_line`].
//!
//! Why not the canonical [`event_schema`] surface used by
//! [`claude_adapter::ClaudeAdapter`]? Worker adapters surface a CLI's
//! output for the user — runs that produce code edits, file activity,
//! test results. The supervisor's role is different: it doesn't edit
//! files; it makes mission-level decisions (decompose, spawn, review,
//! complete). Those map to mission-level events that the orchestrator
//! emits on its existing broadcast channel — not into the canonical
//! worker-event schema. Keeping the supervisor adapter on its own
//! intent vocabulary preserves the "adapters are the only abstraction"
//! discipline (`ARCHITECTURE.md`, "Adapter Boundary") while staying
//! off the protected worker-event surface.
//!
//! ## Intent contract
//!
//! Each supervisor turn the playbook makes the model end its
//! assistant response with a single fenced ```json``` block containing
//! one supported intent envelope:
//!
//! ```text
//! { "action": "decompose", "tasks": [ {"title": "...", "description": "..."} ] }
//! { "action": "spawn_worker", "task_index": 0 }
//! { "action": "review", "worker_id": "mock-1", "decision": "accept",  "summary": "..." }
//! { "action": "review", "worker_id": "mock-1", "decision": "revise",  "directive": "..." }
//! { "action": "review", "worker_id": "mock-1", "decision": "reject",  "reason": "..." }
//! { "action": "declare_complete", "summary": "..." }
//! ```
//!
//! The adapter accumulates `assistant`-channel text from the stream
//! and, on terminal (`result/success` or `finalize`), extracts the last
//! fenced JSON block and parses it as a [`SupervisorIntent`]. Logs
//! (assistant prose outside the fence, plus result text) surface as
//! [`SupervisorOutput::Log`] for the orchestrator to record as
//! mission-level supervisor logs.

#![deny(missing_debug_implementations)]

pub mod evals;

use serde::Deserialize;
use serde_json::Value;
use thiserror::Error;

const MAX_ACCUMULATED_TEXT_BYTES: usize = 256 * 1024;
const MAX_DIAGNOSTIC_TEXT_BYTES: usize = 16 * 1024;

/// Bundled supervisor playbook — the system prompt that turns a raw
/// `claude` CLI process into the Vigla mission supervisor.
///
/// Use with `claude --append-system-prompt "$PLAYBOOK"` (or wire it
/// through whatever system-prompt mechanism the CLI version exposes).
/// Compiled into the adapter binary via `include_str!` so distribution
/// is one file, not a runtime asset path.
pub const PLAYBOOK: &str = include_str!("playbook.md");

/// Phase 2 evidence-path pin for the first real Claude supervisor
/// playbook. This is a product/eval version, not an event-schema
/// version.
pub const PLAYBOOK_VERSION: &str = "real-claude-supervisor-v1";

/// Bundled worker playbook (MSV U4.1) — the system prompt that scopes
/// a Claude/Codex/Gemini CLI to acting as a mission worker: edit files
/// in cwd only, no git commands (Vigla commits after exit), produce
/// a 2–4 sentence plain-prose summary as the final message.
///
/// Loaded by [`crate::PLAYBOOK`]'s sibling consumer
/// `orchestrator::mission_worker_dispatch` and injected via
/// `--append-system-prompt` (Claude) or prepended to the user prompt
/// (Codex / Gemini, which don't expose a system-prompt CLI flag).
pub const WORKER_PLAYBOOK: &str = include_str!("worker_playbook.md");

/// One thing the adapter produced from the supervisor stream.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SupervisorOutput {
    /// The supervisor expressed a decision via the JSON envelope.
    Intent(SupervisorIntent),
    /// Plain text from the supervisor's response (assistant prose) or
    /// from the CLI result line — useful for the structured event
    /// drawer but not used for control flow.
    Log(String),
    /// The supervisor turn finished with no parseable intent. The
    /// orchestrator decides how to handle this (retry or surface a
    /// boundary event).
    NoIntent,
    /// The CLI emitted a `result/error` line — terminal failure of
    /// the turn.
    Error(String),
}

/// QC-3: one row of the supervisor's `tech_stack` summary on a
/// `Decompose` intent. The orchestrator maps this to
/// `mission_event::TechChoice` field-for-field; we duplicate the
/// shape here to keep the adapter crate independent of the
/// orchestrator types.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct TechChoice {
    pub layer: String,
    pub choice: String,
    pub rationale: String,
    #[serde(default)]
    pub is_new: bool,
}

/// QC-3: per-bound classification of the proposed plan against the
/// user's authority envelope. Mirrors `mission_event::BoundFitKind`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum BoundFitKind {
    Within,
    NearLimit,
    Exceeds,
}

/// QC-3: a bound classification plus the supervisor's free-form
/// note. Mirrors `mission_event::BoundFit`.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct BoundFit {
    pub fit: BoundFitKind,
    #[serde(default)]
    pub note: String,
}

/// QC-3: the supervisor's self-assessment of the proposed plan
/// against each of the four authority bounds. Mirrors
/// `mission_event::EnvelopeFit`.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct EnvelopeFit {
    pub scope: BoundFit,
    pub reversibility: BoundFit,
    pub risk: BoundFit,
    pub quality: BoundFit,
}

/// What the supervisor wants Vigla to do next.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(tag = "action", rename_all = "snake_case")]
pub enum SupervisorIntent {
    /// "Here are the tasks." Emitted once on the first turn.
    Decompose {
        tasks: Vec<SupervisorTaskDescriptor>,
        /// QC-3: short prose summary the FE renders above the
        /// task list. `None` for legacy adapters.
        #[serde(default)]
        overview: Option<String>,
        /// QC-3: typed tech-stack rows. `None` for legacy adapters.
        #[serde(default)]
        tech_stack: Option<Vec<TechChoice>>,
        /// QC-3: four-bound self-assessment. `None` for legacy
        /// adapters; mission_loop treats that as "no gate".
        #[serde(default)]
        envelope_fit: Option<EnvelopeFit>,
    },
    /// "Spin up the next worker for this task." Optional in this step
    /// — the orchestrator drives the spawn loop directly off
    /// `Decompose`, but the variant exists so playbooks that want
    /// per-task spawn calls don't fail to parse.
    SpawnWorker { task_index: u32 },
    /// "Here is my decision on this worker's submission."
    Review(ReviewIntent),
    /// "All tasks done; here is the mission summary."
    DeclareComplete { summary: String },
}

/// What the supervisor wants its decomposed tasks to look like.
///
/// `depends_on` and `scope_paths` were added in S7 to let scripted
/// supervisors express DAG topology + per-task file ACLs. Both are
/// `#[serde(default)]`, so legacy playbooks that emit only
/// `{title, description}` continue to deserialize cleanly (root task
/// with no dependencies, no scope override).
///
/// Role and acceptance-criteria expansion is intentionally deferred:
/// those types (`TaskRole`, `AcceptanceCriteria`) live in
/// `orchestrator::task_graph` and the adapter crate doesn't (and
/// shouldn't) depend on `orchestrator`. Surfacing them across the
/// adapter boundary requires hoisting them into `event-schema` first.
#[derive(Debug, Clone, Default, PartialEq, Eq, Deserialize)]
pub struct SupervisorTaskDescriptor {
    pub title: String,
    #[serde(default)]
    pub description: Option<String>,
    /// Indices of upstream tasks that must complete before this one
    /// is dispatched. Empty = root task.
    #[serde(default)]
    pub depends_on: Vec<u32>,
    /// Per-task file ACL override, intersected with the mission's
    /// `scope_paths` at worker-spawn time. Empty = inherit the
    /// mission scope verbatim.
    #[serde(default)]
    pub scope_paths: Vec<std::path::PathBuf>,
}

/// Body of the `review` action. Carries the worker id and the
/// discriminant; the per-discriminant fields are all optional so a
/// single struct decodes every variant. The orchestrator validates
/// per-discriminant which fields must be present.
///
/// ## Field semantics by decision
///
/// | decision           | fields used                              |
/// |--------------------|------------------------------------------|
/// | accept             | summary                                  |
/// | revise             | directive                                |
/// | reject             | reason                                   |
/// | reassign           | from_worker (omitted → use worker_id),   |
/// |                    | to_vendor (omitted → keep current)       |
/// | split              | sub_tasks                                |
/// | narrow             | reduced_scope                            |
/// | rebrief            | new_brief                                |
/// | mark_unachievable  | rationale (or `reason` for back-compat)  |
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct ReviewIntent {
    pub worker_id: String,
    pub decision: ReviewDecisionTag,

    // ── existing fields (S2-shipped) ─────────────────────────────
    /// Used by `accept`.
    #[serde(default)]
    pub summary: Option<String>,
    /// Used by `revise`.
    #[serde(default)]
    pub directive: Option<String>,
    /// Used by `reject` (legacy) and `mark_unachievable` (new
    /// synonym path — see `rationale` below).
    #[serde(default)]
    pub reason: Option<String>,

    // ── S6 additions ────────────────────────────────────────────
    /// `reassign`: the worker_id being torn down. When absent,
    /// defaults to the top-level `worker_id`. Both forms parse so
    /// playbooks can be terse.
    #[serde(default)]
    pub from_worker: Option<String>,
    /// `reassign`: vendor pin for the replacement worker. `None`
    /// (field omitted) keeps the current vendor.
    #[serde(default)]
    pub to_vendor: Option<event_schema::Vendor>,
    /// `split`: replacement sub-tasks. Empty / missing is treated
    /// as a no-op by the orchestrator (falls through to revise).
    #[serde(default)]
    pub sub_tasks: Option<Vec<SupervisorTaskDescriptor>>,
    /// `narrow`: replacement scope_paths.
    #[serde(default)]
    pub reduced_scope: Option<Vec<String>>,
    /// `rebrief`: replacement brief / task title.
    #[serde(default)]
    pub new_brief: Option<String>,
    /// `mark_unachievable`: supervisor's explanation. Falls back
    /// to `reason` when absent (lets playbooks reuse the existing
    /// reject prose without retraining).
    #[serde(default)]
    pub rationale: Option<String>,
}

impl ReviewIntent {
    /// Resolve the `from_worker` field with the top-level
    /// `worker_id` fallback. Used by the orchestrator when
    /// dispatching `Reassign`.
    pub fn resolved_from_worker(&self) -> &str {
        self.from_worker
            .as_deref()
            .unwrap_or(self.worker_id.as_str())
    }

    /// Resolve the unachievable-rationale, treating either field as
    /// authoritative. The supervisor playbook recommends
    /// `rationale`; the back-compat `reason` field is the legacy
    /// reject-synonym path.
    pub fn resolved_rationale(&self) -> Option<&str> {
        self.rationale.as_deref().or(self.reason.as_deref())
    }
}

/// The discriminant on the `review` action envelope. S6 expanded
/// from three (Accept / Revise / Reject) to eight: the original
/// three plus the five new rework kinds from
/// `orchestrator::arbiter::ReworkKind`.
///
/// `Reject` is preserved as an alias for `MarkUnachievable` —
/// playbooks predating S6 use it, and rejection-as-supervisor-
/// declaration is exactly the MarkUnachievable semantic. Both
/// parse; the orchestrator treats them identically. New playbooks
/// should prefer `mark_unachievable`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ReviewDecisionTag {
    /// Integrate the submission as-is.
    Accept,
    /// Re-run the same worker with a directive.
    Revise,
    /// Synonym for MarkUnachievable. Preserved for back-compat;
    /// new playbooks should use `mark_unachievable`.
    Reject,
    /// Tear down the failing worker and spawn a fresh one.
    Reassign,
    /// Replace the current task with N smaller sub-tasks.
    Split,
    /// Re-run with a constrained scope.
    Narrow,
    /// Re-run with a fully replaced brief.
    Rebrief,
    /// Declare the task unachievable; scrub with retained artifacts.
    MarkUnachievable,
}

impl ReviewDecisionTag {
    /// Is this a terminal decision that prevents further automated
    /// rework on the current task?
    pub fn is_terminal(self) -> bool {
        matches!(self, Self::Accept | Self::Reject | Self::MarkUnachievable)
    }

    /// Is this one of the five new rework kinds introduced by S6?
    pub fn is_new_rework_kind(self) -> bool {
        matches!(
            self,
            Self::Reassign | Self::Split | Self::Narrow | Self::Rebrief | Self::MarkUnachievable
        )
    }
}

/// Errors the adapter surfaces. Most parse failures are reported as
/// [`SupervisorOutput::NoIntent`] rather than these — these are for
/// callers (the orchestrator) that need to inspect parse details for
/// boundary events.
#[derive(Debug, Clone, Error, PartialEq, Eq)]
pub enum SupervisorAdapterError {
    #[error("no fenced JSON block found in supervisor response")]
    NoFence,
    #[error("fenced JSON block did not parse as a supervisor intent: {0}")]
    BadIntent(String),
}

/// Adapter state. One per supervisor turn. Reuse across turns is
/// fine: `reset_for_next_turn` clears the per-turn accumulator while
/// preserving the captured `session_id`.
#[derive(Debug, Default)]
pub struct SupervisorAdapter {
    accumulated_text: String,
    session_id: Option<String>,
    pending_error: Option<String>,
    runtime_hint: Option<String>,
    finalized: bool,
}

impl SupervisorAdapter {
    pub fn new() -> Self {
        Self::default()
    }

    /// Session id captured from the first `system/init` line, used to
    /// `--resume <session>` subsequent turns so the supervisor retains
    /// conversation context.
    pub fn session_id(&self) -> Option<&str> {
        self.session_id.as_deref()
    }

    /// Drain one stream-json line into adapter state. Returns the
    /// outputs (if any) the orchestrator should react to right now.
    /// Most lines produce no output — the only line that emits an
    /// intent is the terminal `result` line (or a [`Self::finalize`]
    /// call when the process ends).
    pub fn ingest_line(&mut self, line: &str) -> Vec<SupervisorOutput> {
        if self.finalized {
            return Vec::new();
        }
        let trimmed = line.trim();
        if trimmed.is_empty() {
            return Vec::new();
        }
        let Ok(value): Result<Value, _> = serde_json::from_str(trimmed) else {
            return Vec::new();
        };
        let Some(ty) = value.get("type").and_then(|v| v.as_str()) else {
            return Vec::new();
        };
        match ty {
            "system" => {
                if value.get("subtype").and_then(|v| v.as_str()) == Some("init") {
                    if let Some(sid) = value.get("session_id").and_then(|v| v.as_str()) {
                        self.session_id = Some(sid.to_owned());
                    }
                    return Vec::new();
                }
                self.handle_system(&value)
            }
            "assistant" => {
                self.absorb_assistant(&value);
                Vec::new()
            }
            "result" => self.handle_result(&value),
            _ => Vec::new(),
        }
    }

    /// Drain remaining state at end of process / EOF. Idempotent.
    pub fn finalize(&mut self) -> Vec<SupervisorOutput> {
        if self.finalized {
            return Vec::new();
        }
        self.finalized = true;
        if let Some(err) = self.pending_error.take() {
            return vec![SupervisorOutput::Error(err)];
        }
        // No result line saw the JSON envelope — try one more time
        // against the accumulated assistant text. Common when the CLI
        // ends abruptly (kill, EOF) without emitting `result`.
        self.extract_intent_or_no_intent()
    }

    /// Drop turn-local state so the same adapter instance can drive a
    /// subsequent turn. Keeps `session_id` for `--resume`.
    pub fn reset_for_next_turn(&mut self) {
        self.accumulated_text.clear();
        self.pending_error = None;
        self.runtime_hint = None;
        self.finalized = false;
    }

    fn handle_system(&mut self, line: &Value) -> Vec<SupervisorOutput> {
        let subtype = line.get("subtype").and_then(|v| v.as_str()).unwrap_or("");
        if subtype == "api_retry" {
            let attempt = line
                .get("attempt")
                .and_then(|v| v.as_u64())
                .map(|n| n.to_string())
                .unwrap_or_else(|| "?".into());
            let max_retries = line
                .get("max_retries")
                .and_then(|v| v.as_u64())
                .map(|n| n.to_string())
                .unwrap_or_else(|| "?".into());
            let error = line
                .get("error")
                .and_then(|v| v.as_str())
                .unwrap_or("unknown");
            let status = line
                .get("error_status")
                .and_then(|v| v.as_i64())
                .map(|n| n.to_string())
                .unwrap_or_else(|| "?".into());
            let message =
                format!("supervisor API retry {attempt}/{max_retries}: {error} (status {status})");
            self.runtime_hint = Some(message.clone());
            return vec![SupervisorOutput::Log(message)];
        }
        Vec::new()
    }

    fn absorb_assistant(&mut self, line: &Value) {
        let Some(content) = line
            .get("message")
            .and_then(|m| m.get("content"))
            .and_then(|c| c.as_array())
        else {
            return;
        };
        for block in content {
            if block.get("type").and_then(|v| v.as_str()) != Some("text") {
                continue;
            }
            if let Some(text) = block.get("text").and_then(|v| v.as_str()) {
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
            }
        }
    }

    fn handle_result(&mut self, line: &Value) -> Vec<SupervisorOutput> {
        let subtype = line.get("subtype").and_then(|v| v.as_str()).unwrap_or("");
        if subtype == "error" || subtype.starts_with("error_") {
            let raw_msg = line
                .get("result")
                .and_then(|v| v.as_str())
                .or_else(|| line.get("error").and_then(|v| v.as_str()))
                .unwrap_or("supervisor turn errored");
            let mut msg = String::new();
            adapter_core::append_bounded_tail(&mut msg, raw_msg, MAX_DIAGNOSTIC_TEXT_BYTES);
            self.pending_error = Some(msg);
        }
        self.finalized = true;
        if let Some(err) = self.pending_error.take() {
            return vec![SupervisorOutput::Error(err)];
        }
        // Fold the final `result` text into the log stream too — useful
        // diagnostic when extraction fails.
        let mut out = Vec::new();
        if let Some(text) = line.get("result").and_then(|v| v.as_str()) {
            if !text.trim().is_empty() {
                let mut diagnostic = String::new();
                adapter_core::append_bounded_tail(&mut diagnostic, text, MAX_DIAGNOSTIC_TEXT_BYTES);
                out.push(SupervisorOutput::Log(diagnostic));
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
            }
        }
        out.extend(self.extract_intent_or_no_intent());
        out
    }

    fn extract_intent_or_no_intent(&self) -> Vec<SupervisorOutput> {
        match extract_intent(&self.accumulated_text) {
            Ok(intent) => vec![SupervisorOutput::Intent(intent)],
            Err(SupervisorAdapterError::NoFence) => self.no_intent_output(),
            Err(SupervisorAdapterError::BadIntent(_)) => self.no_intent_output(),
        }
    }

    fn no_intent_output(&self) -> Vec<SupervisorOutput> {
        if let Some(hint) = &self.runtime_hint {
            return vec![SupervisorOutput::Error(format!(
                "supervisor produced no parseable intent after runtime event: {hint}"
            ))];
        }
        vec![SupervisorOutput::NoIntent]
    }
}

/// Pull the last fenced ```json``` block out of `text` and deserialize
/// it as a [`SupervisorIntent`].
///
/// "Last" rather than "first" because if the playbook is misbehaving
/// and emits an example block followed by the real one, we want the
/// real one. Whitespace-tolerant: the block can be preceded or
/// followed by prose without confusing the parser.
pub fn extract_intent(text: &str) -> Result<SupervisorIntent, SupervisorAdapterError> {
    let mut found: Option<&str> = None;
    let mut cursor = 0usize;
    while let Some(start) = text[cursor..].find("```") {
        let abs_start = cursor + start;
        // Skip past the opening fence and optional language tag.
        let after_fence = abs_start + 3;
        let line_end = text[after_fence..]
            .find('\n')
            .map(|i| after_fence + i + 1)
            .unwrap_or(text.len());
        // `line_end` is text.len() or one past a '\n'; both are valid
        // char boundaries, so this slice never panics. `.trim()` drops
        // the trailing newline (when present) along with surrounding
        // whitespace — no need to exclude it by index (the prior
        // `line_end - 1` reversed the range on a bare/EOF fence and could
        // split a multibyte char).
        let lang = text[after_fence..line_end].trim();
        let body_start = line_end;
        let Some(close_rel) = text[body_start..].find("```") else {
            break;
        };
        let body_end = body_start + close_rel;
        if lang.eq_ignore_ascii_case("json") || lang.is_empty() {
            // Accept ``` (no lang) too — playbook should always use
            // ```json but tolerate sloppy formatting.
            found = Some(text[body_start..body_end].trim());
        }
        cursor = body_end + 3;
    }
    let body = found.ok_or(SupervisorAdapterError::NoFence)?;
    serde_json::from_str::<SupervisorIntent>(body)
        .map_err(|e| SupervisorAdapterError::BadIntent(e.to_string()))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fixture(name: &str) -> &'static str {
        match name {
            "init" => include_str!("fixtures/init.jsonl"),
            "decompose" => include_str!("fixtures/decompose.jsonl"),
            "review_accept" => include_str!("fixtures/review_accept.jsonl"),
            "review_revise" => include_str!("fixtures/review_revise.jsonl"),
            "review_reject" => include_str!("fixtures/review_reject.jsonl"),
            "review_reassign" => include_str!("fixtures/review_reassign.jsonl"),
            "review_split" => include_str!("fixtures/review_split.jsonl"),
            "review_narrow" => include_str!("fixtures/review_narrow.jsonl"),
            "review_rebrief" => include_str!("fixtures/review_rebrief.jsonl"),
            "review_mark_unachievable" => include_str!("fixtures/review_mark_unachievable.jsonl"),
            "declare_complete" => include_str!("fixtures/declare_complete.jsonl"),
            "no_fence" => include_str!("fixtures/no_fence.jsonl"),
            "error" => include_str!("fixtures/error.jsonl"),
            _ => panic!("unknown fixture {name}"),
        }
    }

    fn drive(name: &str) -> (SupervisorAdapter, Vec<SupervisorOutput>) {
        let mut adapter = SupervisorAdapter::new();
        let mut outputs = Vec::new();
        for line in fixture(name).lines() {
            outputs.extend(adapter.ingest_line(line));
        }
        outputs.extend(adapter.finalize());
        (adapter, outputs)
    }

    #[test]
    fn captures_session_id_from_system_init() {
        let (adapter, _) = drive("init");
        assert_eq!(adapter.session_id(), Some("sess-abc-123"));
    }

    #[test]
    fn parses_decompose_envelope_from_fenced_block() {
        let (_, outputs) = drive("decompose");
        let intent = outputs
            .iter()
            .find_map(|o| match o {
                SupervisorOutput::Intent(i) => Some(i),
                _ => None,
            })
            .expect("intent");
        match intent {
            SupervisorIntent::Decompose { tasks, .. } => {
                assert_eq!(tasks.len(), 3);
                assert_eq!(tasks[0].title, "Plan integration");
                assert_eq!(tasks[1].title, "Implement changes");
                assert_eq!(tasks[2].title, "Update documentation");
            }
            other => panic!("expected Decompose, got {other:?}"),
        }
    }

    #[test]
    fn parses_decompose_with_envelope_fields() {
        let raw = include_str!("../tests/fixtures/decompose_with_envelope.json");
        let wrapped = format!("```json\n{raw}\n```\n");
        let intent = extract_intent(&wrapped).expect("should parse");
        match intent {
            SupervisorIntent::Decompose {
                tasks,
                overview,
                tech_stack,
                envelope_fit,
            } => {
                assert_eq!(tasks.len(), 1);
                assert_eq!(
                    overview.as_deref(),
                    Some("Add an OAuth callback handler and migrate the existing session table.")
                );
                let ts = tech_stack.expect("tech_stack present");
                assert_eq!(ts.len(), 2);
                assert!(ts.iter().any(|t| t.layer == "migrations" && t.is_new));
                let ef = envelope_fit.expect("envelope_fit present");
                assert_eq!(ef.reversibility.fit, BoundFitKind::NearLimit);
                assert_eq!(ef.reversibility.note, "schema migration; rollback exists");
            }
            other => panic!("expected Decompose, got {other:?}"),
        }
    }

    #[test]
    fn parses_decompose_legacy_payload_with_none_envelope() {
        let raw = include_str!("../tests/fixtures/decompose_legacy_no_envelope.json");
        let wrapped = format!("```json\n{raw}\n```\n");
        let intent = extract_intent(&wrapped).expect("should parse");
        match intent {
            SupervisorIntent::Decompose {
                tasks,
                overview,
                tech_stack,
                envelope_fit,
            } => {
                assert_eq!(tasks.len(), 1);
                assert!(overview.is_none());
                assert!(tech_stack.is_none());
                assert!(envelope_fit.is_none());
            }
            other => panic!("expected Decompose, got {other:?}"),
        }
    }

    #[test]
    fn rejects_decompose_with_malformed_envelope() {
        let raw = include_str!("../tests/fixtures/decompose_envelope_malformed.json");
        let wrapped = format!("```json\n{raw}\n```\n");
        let err = extract_intent(&wrapped).expect_err("should reject");
        let msg = format!("{err}");
        assert!(
            msg.contains("kinda_within") || msg.contains("fit") || msg.contains("variant"),
            "error should name the bad enum value: {msg}"
        );
    }

    #[test]
    fn parses_review_accept() {
        let (_, outputs) = drive("review_accept");
        let intent = outputs
            .iter()
            .find_map(|o| match o {
                SupervisorOutput::Intent(i) => Some(i),
                _ => None,
            })
            .expect("intent");
        match intent {
            SupervisorIntent::Review(r) => {
                assert_eq!(r.worker_id, "mock-1");
                assert_eq!(r.decision, ReviewDecisionTag::Accept);
                assert!(r.summary.as_deref().unwrap().contains("looks good"));
            }
            other => panic!("expected Review, got {other:?}"),
        }
    }

    #[test]
    fn parses_review_revise() {
        let (_, outputs) = drive("review_revise");
        let intent = outputs
            .iter()
            .find_map(|o| match o {
                SupervisorOutput::Intent(i) => Some(i),
                _ => None,
            })
            .expect("intent");
        match intent {
            SupervisorIntent::Review(r) => {
                assert_eq!(r.decision, ReviewDecisionTag::Revise);
                assert!(r.directive.is_some(), "revise must carry a directive");
                assert!(r.directive.as_deref().unwrap().contains("flesh out"));
            }
            other => panic!("expected Review, got {other:?}"),
        }
    }

    #[test]
    fn parses_review_reject() {
        let (_, outputs) = drive("review_reject");
        let intent = outputs
            .iter()
            .find_map(|o| match o {
                SupervisorOutput::Intent(i) => Some(i),
                _ => None,
            })
            .expect("intent");
        match intent {
            SupervisorIntent::Review(r) => {
                assert_eq!(r.decision, ReviewDecisionTag::Reject);
                assert!(r.reason.is_some(), "reject must carry a reason");
            }
            other => panic!("expected Review, got {other:?}"),
        }
    }

    #[test]
    fn parses_declare_complete() {
        let (_, outputs) = drive("declare_complete");
        let intent = outputs
            .iter()
            .find_map(|o| match o {
                SupervisorOutput::Intent(i) => Some(i),
                _ => None,
            })
            .expect("intent");
        match intent {
            SupervisorIntent::DeclareComplete { summary } => {
                assert!(summary.contains("integrated"));
            }
            other => panic!("expected DeclareComplete, got {other:?}"),
        }
    }

    #[test]
    fn no_fence_yields_no_intent_output_not_panic() {
        let (_, outputs) = drive("no_fence");
        assert!(outputs
            .iter()
            .any(|o| matches!(o, SupervisorOutput::NoIntent)));
        assert!(!outputs
            .iter()
            .any(|o| matches!(o, SupervisorOutput::Intent(_))));
    }

    #[test]
    fn result_text_fenced_json_is_parsed_as_fallback() {
        let mut adapter = SupervisorAdapter::new();
        let outputs = adapter.ingest_line(
            r#"{"type":"result","subtype":"success","result":"Done.\n\n```json\n{\"action\":\"declare_complete\",\"summary\":\"all tasks integrated\"}\n```"}"#,
        );

        assert!(outputs.iter().any(|o| matches!(
            o,
            SupervisorOutput::Intent(SupervisorIntent::DeclareComplete { .. })
        )));
    }

    #[test]
    fn api_retry_without_final_intent_surfaces_actionable_error() {
        let mut adapter = SupervisorAdapter::new();
        let logs = adapter.ingest_line(
            r#"{"type":"system","subtype":"api_retry","attempt":1,"max_retries":10,"error_status":529,"error":"rate_limit"}"#,
        );
        assert!(logs.iter().any(|o| matches!(
            o,
            SupervisorOutput::Log(line) if line.contains("rate_limit")
        )));

        let outputs = adapter.finalize();
        assert!(outputs.iter().any(|o| matches!(
            o,
            SupervisorOutput::Error(err) if err.contains("rate_limit")
        )));
    }

    #[test]
    fn result_error_subtype_surfaces_as_error_output() {
        let (_, outputs) = drive("error");
        assert!(outputs
            .iter()
            .any(|o| matches!(o, SupervisorOutput::Error(_))));
    }

    #[test]
    fn extract_intent_picks_last_fenced_block_when_multiple_present() {
        let text = "Example block:\n```json\n{\"action\":\"spawn_worker\",\"task_index\":0}\n```\n\nNow my real decision:\n```json\n{\"action\":\"declare_complete\",\"summary\":\"all done\"}\n```\n";
        let intent = extract_intent(text).expect("parse");
        assert!(matches!(intent, SupervisorIntent::DeclareComplete { .. }));
    }

    #[test]
    fn extract_intent_tolerates_bare_fence_without_lang_tag() {
        let text = "```\n{\"action\":\"spawn_worker\",\"task_index\":0}\n```\n";
        let intent = extract_intent(text).expect("parse");
        assert!(matches!(
            intent,
            SupervisorIntent::SpawnWorker { task_index: 0 }
        ));
    }

    #[test]
    fn extract_intent_bare_fence_at_eof_does_not_panic() {
        // Regression: a message ending in a bare ``` with no trailing
        // newline must return NoFence, not panic on a reversed slice
        // range (line_end.saturating_sub(1) when line_end == text.len()).
        assert!(matches!(
            extract_intent("```"),
            Err(SupervisorAdapterError::NoFence)
        ));
    }

    #[test]
    fn extract_intent_multibyte_after_bare_fence_does_not_panic() {
        // Regression: a fence immediately followed by a multibyte char
        // and no newline must respect UTF-8 char boundaries when slicing
        // the language tag rather than panicking mid-codepoint.
        assert!(matches!(
            extract_intent("```é"),
            Err(SupervisorAdapterError::NoFence)
        ));
    }

    #[test]
    fn supervisor_playbook_pins_codebase_discovery_section() {
        // QC-1: the supervisor must read codebase context before
        // decomposing. The playbook section that documents this
        // procedure is load-bearing — if it's accidentally deleted in
        // a future refactor, decomposition silently regresses to the
        // pre-QC1 "guess from the objective string" behavior. Pin the
        // section header and a few key procedural words.
        assert!(
            PLAYBOOK.contains("## Codebase discovery"),
            "supervisor playbook must keep the Codebase discovery section header"
        );
        assert!(
            PLAYBOOK.contains("README.md"),
            "supervisor playbook discovery section must mention README.md"
        );
        assert!(
            PLAYBOOK.contains("Skim, don't fully read")
                || PLAYBOOK.contains("Skim, not fully read"),
            "supervisor playbook discovery section must keep the soft-budget framing"
        );
    }

    #[test]
    fn supervisor_playbook_declares_v1_identity() {
        assert_eq!(PLAYBOOK_VERSION, "real-claude-supervisor-v1");
        assert!(
            PLAYBOOK.contains(PLAYBOOK_VERSION),
            "playbook must carry the v1 identity used by Phase 2 evals"
        );
        assert!(
            PLAYBOOK.contains("mission supervisor"),
            "playbook must continue to define the CLI process as the mission supervisor"
        );
    }

    #[test]
    fn worker_playbook_pins_no_git_and_summary_contract() {
        // Worker playbook drift matters too: if a future edit silently
        // tells workers to commit themselves (or removes the summary
        // expectation), the orchestrator's post-run git-commit step
        // will fail or the supervisor's review prompt will starve.
        // Pin the load-bearing rules.
        assert!(
            WORKER_PLAYBOOK.contains("Never run `git` commands"),
            "worker playbook must keep the no-git rule"
        );
        assert!(
            WORKER_PLAYBOOK.contains("one-paragraph") || WORKER_PLAYBOOK.contains("2–4 sentences"),
            "worker playbook must keep the short-summary contract"
        );
        assert!(
            WORKER_PLAYBOOK.contains("working directory"),
            "worker playbook must scope the worker to its cwd"
        );
        assert!(
            WORKER_PLAYBOOK.contains("Revision directive"),
            "worker playbook must keep the revision-pass marker"
        );
    }

    #[test]
    fn playbook_pins_intent_action_vocabulary() {
        // If the playbook drifts from the SupervisorIntent vocabulary
        // (e.g. someone renames `declare_complete` in the prose but the
        // adapter still expects the old value), the
        // supervisor will silently produce parse failures at runtime.
        // Pin both sides here so the test must be updated alongside
        // either edit.
        for action in ["decompose", "spawn_worker", "review", "declare_complete"] {
            assert!(
                PLAYBOOK.contains(action),
                "playbook is missing the `{action}` action documentation"
            );
        }
        for decision in [
            "accept",
            "revise",
            "narrow",
            "rebrief",
            "reassign",
            "split",
            "mark_unachievable",
            "reject", // legacy alias still documented
        ] {
            assert!(
                PLAYBOOK.contains(decision),
                "playbook is missing the `{decision}` review decision"
            );
        }
        // Sanity: the fence convention must appear so the adapter's
        // parser actually finds something.
        assert!(PLAYBOOK.contains("```json"));
    }

    #[test]
    fn playbook_pins_six_intervention_kinds_section() {
        // The S6 expansion is load-bearing — if a future edit silently
        // collapses the playbook back to accept/revise/reject only,
        // the supervisor will never emit the new kinds and the rework
        // engine will degrade to S2 behaviour. Pin the section header
        // and a few load-bearing phrases.
        assert!(
            PLAYBOOK.contains("The six intervention kinds"),
            "playbook must keep the six-intervention-kinds section header"
        );
        assert!(
            PLAYBOOK.contains("`narrow`"),
            "playbook must document the narrow decision"
        );
        assert!(
            PLAYBOOK.contains("`rebrief`"),
            "playbook must document the rebrief decision"
        );
        assert!(
            PLAYBOOK.contains("`reassign`"),
            "playbook must document the reassign decision"
        );
        assert!(
            PLAYBOOK.contains("`split`"),
            "playbook must document the split decision"
        );
        assert!(
            PLAYBOOK.contains("`mark_unachievable`"),
            "playbook must document the mark_unachievable decision"
        );
        assert!(
            PLAYBOOK.contains("Cost: low") || PLAYBOOK.contains("**Cost: low.**"),
            "playbook must keep the cost annotations on rework kinds"
        );
        assert!(
            PLAYBOOK.contains("Never `mark_unachievable` on the first pass"),
            "playbook must keep the first-pass-rule against premature unachievable"
        );
    }

    #[test]
    fn reset_for_next_turn_preserves_session_id() {
        let mut adapter = SupervisorAdapter::new();
        for line in fixture("init").lines() {
            adapter.ingest_line(line);
        }
        adapter.finalize();
        let sid_before = adapter.session_id().map(str::to_owned);
        adapter.reset_for_next_turn();
        assert_eq!(adapter.session_id().map(str::to_owned), sid_before);
    }

    #[test]
    fn assistant_text_accumulates_across_multiple_blocks() {
        // Two assistant lines (reasoning + decision) — the fenced
        // envelope on the second still parses after the first has been
        // absorbed. Claude streams complete-thought blocks in `-p`
        // mode, not arbitrary token splits, so the test reflects the
        // realistic boundary.
        let mut adapter = SupervisorAdapter::new();
        let l1 = r#"{"type":"assistant","message":{"role":"assistant","content":[{"type":"text","text":"Looking over the integration. All workers landed clean."}]}}"#;
        let l2 = r#"{"type":"assistant","message":{"role":"assistant","content":[{"type":"text","text":"Ready to complete.\n\n```json\n{\"action\":\"declare_complete\",\"summary\":\"all workers integrated\"}\n```"}]}}"#;
        adapter.ingest_line(l1);
        adapter.ingest_line(l2);
        let out = adapter.finalize();
        assert!(out.iter().any(|o| matches!(
            o,
            SupervisorOutput::Intent(SupervisorIntent::DeclareComplete { .. })
        )));
    }

    #[test]
    fn review_intent_parses_mark_unachievable_via_reason_fallback() {
        let body = r#"{
            "worker_id": "mock-1",
            "decision": "mark_unachievable",
            "reason": "back-compat field"
        }"#;
        let intent: ReviewIntent = serde_json::from_str(body).unwrap();
        assert_eq!(intent.resolved_rationale(), Some("back-compat field"));
    }

    #[test]
    fn review_decision_tag_is_terminal_correctly() {
        assert!(ReviewDecisionTag::Accept.is_terminal());
        assert!(ReviewDecisionTag::Reject.is_terminal());
        assert!(ReviewDecisionTag::MarkUnachievable.is_terminal());
        assert!(!ReviewDecisionTag::Revise.is_terminal());
        assert!(!ReviewDecisionTag::Reassign.is_terminal());
        assert!(!ReviewDecisionTag::Split.is_terminal());
        assert!(!ReviewDecisionTag::Narrow.is_terminal());
        assert!(!ReviewDecisionTag::Rebrief.is_terminal());
    }

    #[test]
    fn review_decision_tag_is_new_rework_kind_correctly() {
        assert!(ReviewDecisionTag::Reassign.is_new_rework_kind());
        assert!(ReviewDecisionTag::Split.is_new_rework_kind());
        assert!(ReviewDecisionTag::Narrow.is_new_rework_kind());
        assert!(ReviewDecisionTag::Rebrief.is_new_rework_kind());
        assert!(ReviewDecisionTag::MarkUnachievable.is_new_rework_kind());
        assert!(!ReviewDecisionTag::Revise.is_new_rework_kind());
        assert!(!ReviewDecisionTag::Accept.is_new_rework_kind());
        assert!(!ReviewDecisionTag::Reject.is_new_rework_kind());
    }

    #[test]
    fn parses_review_reassign() {
        let (_, outputs) = drive("review_reassign");
        let intent = outputs
            .iter()
            .find_map(|o| match o {
                SupervisorOutput::Intent(i) => Some(i),
                _ => None,
            })
            .expect("intent");
        match intent {
            SupervisorIntent::Review(r) => {
                assert_eq!(r.decision, ReviewDecisionTag::Reassign);
                assert_eq!(r.resolved_from_worker(), "mock-1");
                assert_eq!(r.to_vendor, Some(event_schema::Vendor::Codex));
            }
            other => panic!("expected Review, got {other:?}"),
        }
    }

    #[test]
    fn parses_review_split() {
        let (_, outputs) = drive("review_split");
        let intent = outputs
            .iter()
            .find_map(|o| match o {
                SupervisorOutput::Intent(i) => Some(i),
                _ => None,
            })
            .expect("intent");
        match intent {
            SupervisorIntent::Review(r) => {
                assert_eq!(r.decision, ReviewDecisionTag::Split);
                let subs = r.sub_tasks.as_ref().unwrap();
                assert_eq!(subs.len(), 3);
                assert_eq!(subs[0].title, "Add request parser");
            }
            other => panic!("expected Review, got {other:?}"),
        }
    }

    #[test]
    fn parses_review_narrow() {
        let (_, outputs) = drive("review_narrow");
        let intent = outputs
            .iter()
            .find_map(|o| match o {
                SupervisorOutput::Intent(i) => Some(i),
                _ => None,
            })
            .expect("intent");
        match intent {
            SupervisorIntent::Review(r) => {
                assert_eq!(r.decision, ReviewDecisionTag::Narrow);
                let s = r.reduced_scope.as_ref().unwrap();
                assert_eq!(s.len(), 2);
                assert!(s[0].ends_with("parser.rs"));
            }
            other => panic!("expected Review, got {other:?}"),
        }
    }

    #[test]
    fn parses_review_rebrief() {
        let (_, outputs) = drive("review_rebrief");
        let intent = outputs
            .iter()
            .find_map(|o| match o {
                SupervisorOutput::Intent(i) => Some(i),
                _ => None,
            })
            .expect("intent");
        match intent {
            SupervisorIntent::Review(r) => {
                assert_eq!(r.decision, ReviewDecisionTag::Rebrief);
                assert!(r.new_brief.as_deref().unwrap().contains("parser"));
            }
            other => panic!("expected Review, got {other:?}"),
        }
    }

    #[test]
    fn parses_review_mark_unachievable() {
        let (_, outputs) = drive("review_mark_unachievable");
        let intent = outputs
            .iter()
            .find_map(|o| match o {
                SupervisorOutput::Intent(i) => Some(i),
                _ => None,
            })
            .expect("intent");
        match intent {
            SupervisorIntent::Review(r) => {
                assert_eq!(r.decision, ReviewDecisionTag::MarkUnachievable);
                assert!(r.resolved_rationale().unwrap().contains("binary protocol"));
            }
            other => panic!("expected Review, got {other:?}"),
        }
    }

    /// S7 adapter expansion: `SupervisorTaskDescriptor` deserializes
    /// JSON payloads carrying `depends_on` and `scope_paths`. Both
    /// fields default to empty, so legacy `{title, description}`
    /// payloads still parse cleanly.
    #[test]
    fn task_descriptor_accepts_depends_on_and_scope_paths() {
        let json = r#"{
          "title": "implement /api/logout",
          "depends_on": [0, 1],
          "scope_paths": ["src/auth", "src/session"]
        }"#;
        let td: SupervisorTaskDescriptor = serde_json::from_str(json).unwrap();
        assert_eq!(td.title, "implement /api/logout");
        assert_eq!(td.depends_on, vec![0, 1]);
        assert_eq!(
            td.scope_paths,
            vec![
                std::path::PathBuf::from("src/auth"),
                std::path::PathBuf::from("src/session"),
            ]
        );
    }

    #[test]
    fn task_descriptor_legacy_two_field_payload_still_parses() {
        let json = r#"{ "title": "legacy task", "description": "no DAG fields" }"#;
        let td: SupervisorTaskDescriptor = serde_json::from_str(json).unwrap();
        assert_eq!(td.title, "legacy task");
        assert_eq!(td.description.as_deref(), Some("no DAG fields"));
        assert!(td.depends_on.is_empty());
        assert!(td.scope_paths.is_empty());
    }

    #[test]
    fn accumulated_supervisor_text_is_bounded_and_keeps_latest_intent() {
        let mut adapter = SupervisorAdapter::new();
        let noisy = serde_json::json!({
            "type": "assistant",
            "message": { "content": [{
                "type": "text",
                "text": "x".repeat(MAX_ACCUMULATED_TEXT_BYTES * 2)
            }]}
        });
        assert!(adapter.ingest_line(&noisy.to_string()).is_empty());
        assert!(adapter.accumulated_text.len() <= MAX_ACCUMULATED_TEXT_BYTES);

        let result = serde_json::json!({
            "type": "result",
            "subtype": "success",
            "result": "```json\n{\"action\":\"declare_complete\",\"summary\":\"done\"}\n```"
        });
        let outputs = adapter.ingest_line(&result.to_string());
        assert!(outputs.iter().any(|output| matches!(
            output,
            SupervisorOutput::Intent(SupervisorIntent::DeclareComplete { summary })
                if summary == "done"
        )));
        assert!(adapter.accumulated_text.len() <= MAX_ACCUMULATED_TEXT_BYTES);
    }
}
