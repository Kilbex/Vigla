//! Vigla canonical worker event schema.
//!
//! This crate is the canonical event contract — the types and their
//! doc comments ARE the spec. It is deliberately runtime-free — only
//! `serde` (for wire-format) and `specta` (for TypeScript binding
//! generation). No tokio, no I/O, no orchestrator surface. Adapters,
//! the mock-harness, and the orchestrator all consume types from here.
//!
//! Wire format example:
//!
//! ```jsonc
//! {
//!   "schema_version": "1.0",
//!   "worker_id": "...",
//!   "task_id": "..." | null,
//!   "seq": 123,
//!   "ts": "2026-05-08T19:42:13.481Z",
//!   "type": "state_change",
//!   "payload": { ... }
//! }
//! ```
//!
//! Implementation note: the envelope's `type` and `payload` fields are
//! produced by the adjacently-tagged [`EventKind`] enum, flattened into
//! [`Event`]. This matches the wire format exactly while keeping the
//! Rust API a discriminated union.

#![deny(missing_debug_implementations)]
#![deny(missing_docs)]

/// Memory Kernel canonical event types — the orchestrator-emitted log
/// that runs parallel to the worker event stream.
pub mod memory;
/// RFC 3339 timestamp helpers shared by every producer of [`Event`].
pub mod time;

use serde::{Deserialize, Serialize};
use specta::Type;

/// Schema version. Producers stamp this on every event.
/// Major bump = breaking wire change (removal, rename, retyping);
/// minor bump = additive change consumers can ignore.
///
/// 2.0 (2026-05-10) — `Vendor::Aider` removed (major). Old saved
/// events with `vendor: "aider"` no longer deserialize.
/// 1.0 — initial.
pub const SCHEMA_VERSION: &str = "2.0";

// ---------------------------------------------------------------------
// Identity models (§2)
// ---------------------------------------------------------------------

/// Closed set of supported worker vendors. Expanding requires a minor
/// schema-version bump; removing requires a major bump (see
/// [`SCHEMA_VERSION`]). `Aider` was removed in 2.0.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, Type)]
#[serde(rename_all = "snake_case")]
pub enum Vendor {
    /// Anthropic Claude Code CLI. Wire name `claude`.
    Claude,
    /// OpenAI Codex CLI. Wire name `codex`.
    Codex,
    /// Legacy Gemini CLI, kept as an enterprise compatibility path
    /// after consumer Login with Google ended. Wire name `gemini`.
    Gemini,
    /// Google Antigravity CLI. Wire name `antigravity`.
    Antigravity,
    /// AWS Kiro, driven through a stored vendor profile. Wire name
    /// `kiro`.
    Kiro,
    /// GitHub Copilot CLI, driven through a stored vendor profile.
    /// Wire name `copilot`.
    Copilot,
    /// OpenCode. Reserved by the schema; no adapter crate ships yet
    /// (see `docs/GOOD_FIRST_ISSUES.md`). Wire name `opencode`.
    Opencode,
    /// The credential-free mock harness used by demos and tests. Wire
    /// name `mock`.
    Mock,
}

/// Worker identity record (§2). The orchestrator builds this at spawn
/// time and persists it; events reference the worker by `id`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Type)]
pub struct WorkerInfo {
    /// Stable worker id. Every [`Event`] this worker emits carries it
    /// as `worker_id`.
    pub id: String,
    /// Operator-facing display name shown on the station canvas.
    pub name: String,
    /// Which vendor CLI backs this worker.
    pub vendor: Vendor,
    /// Executable actually spawned, as resolved on `PATH`.
    pub cli_binary: String,
    /// Version string the CLI reported at spawn, when it reports one.
    pub cli_version: Option<String>,
    /// Working directory the CLI was spawned in — the worker's own
    /// git worktree, not the operator's repository root.
    pub cwd: String,
    /// Model the worker was asked to run, when the vendor exposes a
    /// model selector.
    pub model: Option<String>,
    /// RFC 3339 timestamp of spawn.
    pub spawned_at: String,
    /// RFC 3339 timestamp of process exit; `None` while running.
    pub ended_at: Option<String>,
}

/// Task identity record. `depends_on` drives dependency-aware dispatch.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Type)]
pub struct TaskInfo {
    /// Stable task id. Events produced while working this task carry
    /// it as `task_id`.
    pub id: String,
    /// Enclosing task, for plans decomposed more than one level deep.
    pub parent_id: Option<String>,
    /// Operator-facing task title, as shown on the execution map.
    pub title: String,
    /// Task ids that must reach a terminal state before this one is
    /// dispatchable.
    pub depends_on: Vec<String>,
    /// RFC 3339 timestamp of task creation.
    pub created_at: String,
}

// ---------------------------------------------------------------------
// Envelope (§1) and discriminated event union (§4)
// ---------------------------------------------------------------------

/// Canonical event envelope. The `kind` field is flattened into the
/// outer object so the wire format is exactly the envelope shown in
/// the crate-level docs.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Type)]
pub struct Event {
    /// Value of [`SCHEMA_VERSION`] at the time the producer emitted
    /// this event.
    pub schema_version: String,
    /// Id of the emitting worker ([`WorkerInfo::id`]).
    pub worker_id: String,
    /// Task the worker was executing, when the event is attributable
    /// to one ([`TaskInfo::id`]).
    pub task_id: Option<String>,
    /// Adapter-owned counter, monotonic per worker. It is the
    /// adapter's own numbering and need not match anything the vendor
    /// CLI emits; `(worker_id, seq)` is the event store's primary key.
    pub seq: u64,
    /// RFC 3339 timestamp, millisecond precision, UTC.
    pub ts: String,
    /// The payload, flattened into `type` + `payload` on the wire.
    #[serde(flatten)]
    pub kind: EventKind,
}

/// Discriminated payload union. Adjacently tagged: `{"type": "...",
/// "payload": {...}}`. `rename_all = "snake_case"` defines the
/// canonical wire names.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Type)]
#[serde(tag = "type", content = "payload", rename_all = "snake_case")]
pub enum EventKind {
    /// Worker lifecycle transition. Wire name `state_change`.
    StateChange(StateChange),
    /// One line of adapter-normalized CLI output. Wire name `log`.
    Log(Log),
    /// Self-reported progress toward the current task. Wire name
    /// `progress`.
    Progress(Progress),
    /// A file the worker created, changed, moved, or read. Wire name
    /// `file_activity`.
    FileActivity(FileActivity),
    /// Outcome of a test run the worker executed. Wire name
    /// `test_result`.
    TestResult(TestResult),
    /// Token and spend accounting for the turn. Wire name `cost`.
    Cost(Cost),
    /// The worker is blocked on other tasks. Wire name `dependency`.
    Dependency(Dependency),
    /// Terminal success payload. Wire name `completion`.
    Completion(Completion),
    /// Terminal failure payload. Wire name `failure`.
    Failure(Failure),
}

// ---------------------------------------------------------------------
// §4.1 state_change
// ---------------------------------------------------------------------

/// Lifecycle states a worker transitions through. Every adapter
/// announces `Idle` first, then advances the worker until it lands
/// on `Done`, `Failed`, or `Blocked`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, Type)]
#[serde(rename_all = "snake_case")]
pub enum WorkerState {
    /// Spawned, nothing started yet. Every adapter announces this
    /// first.
    Idle,
    /// Deciding what to do before touching the worktree.
    Planning,
    /// Actively working the task.
    Executing,
    /// Waiting on a dependency; see [`Dependency`].
    Blocked,
    /// Work is submitted and awaiting the arbiter's audit.
    Reviewing,
    /// Terminal: the task finished successfully.
    Done,
    /// Terminal: the task did not finish; see [`Failure`].
    Failed,
}

/// Payload of an [`EventKind::StateChange`].
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Type)]
pub struct StateChange {
    /// State the worker just entered.
    pub state: WorkerState,
    /// State it left. Absent on the first announcement.
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub from: Option<WorkerState>,
    /// Optional human-readable reason for the transition.
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub note: Option<String>,
}

// ---------------------------------------------------------------------
// §4.2 log
// ---------------------------------------------------------------------

/// Severity an adapter assigns to a normalized output line. Vendor
/// CLIs rarely label their own output, so this is the adapter's
/// classification, not the CLI's.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, Type)]
#[serde(rename_all = "snake_case")]
pub enum LogLevel {
    /// Fine-grained protocol detail.
    Trace,
    /// Diagnostic detail useful when something went wrong.
    Debug,
    /// Normal progress narration.
    Info,
    /// Something recoverable that an operator should notice.
    Warn,
    /// An error the worker reported but did not necessarily die from.
    Error,
}

/// Which of the child process's two output streams a line arrived on.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, Type)]
#[serde(rename_all = "snake_case")]
pub enum LogStream {
    /// The child's standard output.
    Stdout,
    /// The child's standard error.
    Stderr,
}

/// Payload of an [`EventKind::Log`] — one adapter-normalized output
/// line.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Type)]
pub struct Log {
    /// Adapter-assigned severity.
    pub level: LogLevel,
    /// Stream the line arrived on.
    pub stream: LogStream,
    /// The line itself, with the vendor's framing removed and the
    /// trailing newline stripped.
    pub line: String,
    /// Optional adapter-specific classifier (for example the vendor's
    /// own subsystem or tool name).
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub tag: Option<String>,
}

// ---------------------------------------------------------------------
// §4.3 progress
// ---------------------------------------------------------------------

/// Payload of an [`EventKind::Progress`]. Self-reported by the worker
/// and therefore advisory — the arbiter never treats it as evidence.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Type)]
pub struct Progress {
    /// Completion estimate in the range 0.0–100.0.
    pub percent: f64,
    /// Estimated milliseconds remaining, when the worker offers one.
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub eta_ms: Option<u64>,
    /// Short description of what is happening now.
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub note: Option<String>,
}

// ---------------------------------------------------------------------
// §4.4 file_activity
// ---------------------------------------------------------------------

/// What a worker did to a file.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, Type)]
#[serde(rename_all = "snake_case")]
pub enum FileOp {
    /// The file did not exist before this operation.
    Created,
    /// An existing file's contents changed.
    Modified,
    /// The file was removed.
    Deleted,
    /// The file moved; the old path is in
    /// [`FileActivity::from_path`].
    Renamed,
    /// The file was only read. Recorded because scope auditing counts
    /// reads outside the declared envelope.
    Read,
}

/// Payload of an [`EventKind::FileActivity`]. Paths are relative to
/// the worker's worktree ([`WorkerInfo::cwd`]).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Type)]
pub struct FileActivity {
    /// Path the operation applied to; the destination for a rename.
    pub path: String,
    /// What happened to the file.
    pub op: FileOp,
    /// Original path, set only for [`FileOp::Renamed`].
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub from_path: Option<String>,
    /// Lines added, when the adapter can extract a diffstat.
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub lines_added: Option<u64>,
    /// Lines removed, when the adapter can extract a diffstat.
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub lines_removed: Option<u64>,
    /// Resulting file size in bytes, when the vendor reports it.
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub bytes: Option<u64>,
}

// ---------------------------------------------------------------------
// §4.5 test_result
// ---------------------------------------------------------------------

/// One failing test inside a [`TestResult`].
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Type)]
pub struct TestFailure {
    /// Test name as the runner printed it.
    pub name: String,
    /// Assertion or panic message.
    pub message: String,
    /// Source file, when the runner reports one.
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub file: Option<String>,
    /// 1-based line in `file`, when the runner reports one.
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub line: Option<u64>,
}

/// Payload of an [`EventKind::TestResult`] — the outcome of one test
/// run. The arbiter's regression scorer compares these across the
/// baseline and the submission.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Type)]
pub struct TestResult {
    /// Suite or runner name (for example `cargo test`, `vitest`).
    pub suite: String,
    /// Count of passing tests.
    pub passed: u64,
    /// Count of failing tests.
    pub failed: u64,
    /// Count of skipped or ignored tests.
    pub skipped: u64,
    /// Wall-clock duration of the run, in milliseconds.
    pub duration_ms: u64,
    /// Per-test detail. `None` means the adapter could not extract
    /// detail, which is not the same as an empty list (no failures).
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub failures: Option<Vec<TestFailure>>,
}

// ---------------------------------------------------------------------
// §4.6 cost
// ---------------------------------------------------------------------

/// Payload of an [`EventKind::Cost`]. Values are whatever the vendor
/// CLI reported for the turn; Vigla never calls a billing API to
/// reconcile them.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Type)]
pub struct Cost {
    /// Prompt tokens consumed this turn.
    pub input_tokens: u64,
    /// Completion tokens produced this turn.
    pub output_tokens: u64,
    /// Vendor-reported spend for the turn, in US dollars.
    pub usd: f64,
    /// Tokens served from the vendor's prompt cache, when reported.
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub cache_read_tokens: Option<u64>,
    /// Tokens written into the vendor's prompt cache, when reported.
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub cache_write_tokens: Option<u64>,
    /// Model the vendor billed, when it names one. May differ from
    /// [`WorkerInfo::model`] if the CLI downgraded mid-run.
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub model: Option<String>,
}

// ---------------------------------------------------------------------
// §4.7 dependency
// ---------------------------------------------------------------------

/// Payload of an [`EventKind::Dependency`] — why a worker is in
/// [`WorkerState::Blocked`].
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Type)]
pub struct Dependency {
    /// Task ids ([`TaskInfo::id`]) that must land first.
    pub waiting_on: Vec<String>,
    /// Human-readable explanation shown in the worker drawer.
    pub reason: String,
    /// RFC 3339 timestamp the wait began, when the adapter knows it.
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub since: Option<String>,
}

// ---------------------------------------------------------------------
// §4.8 completion
// ---------------------------------------------------------------------

/// How to interpret an [`Artifact`]'s `ref`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, Type)]
#[serde(rename_all = "snake_case")]
pub enum ArtifactKind {
    /// A path in the worker's worktree.
    File,
    /// A unified diff.
    Diff,
    /// A generated report or summary document.
    Report,
    /// An external URL.
    Url,
}

/// One thing a worker produced, referenced from a [`Completion`].
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Type)]
pub struct Artifact {
    /// How to interpret [`Artifact::artifact_ref`].
    pub kind: ArtifactKind,
    /// Renamed because `ref` is a Rust keyword; the wire name stays
    /// `ref`.
    #[serde(rename = "ref")]
    pub artifact_ref: String,
    /// Operator-facing label for the artifact.
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub label: Option<String>,
}

/// Payload of an [`EventKind::Completion`] — the terminal success
/// record for a task.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Type)]
pub struct Completion {
    /// The worker's own account of what it did.
    pub summary: String,
    /// Artifacts the worker points the reviewer at.
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub artifacts: Option<Vec<Artifact>>,
    /// Wall-clock duration of the task, in milliseconds.
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub duration_ms: Option<u64>,
}

// ---------------------------------------------------------------------
// §4.9 failure
// ---------------------------------------------------------------------

/// Coarse cause of a [`Failure`], used by the recovery engine to
/// decide whether to pause, retry, or escalate.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, Type)]
#[serde(rename_all = "snake_case")]
pub enum FailureCategory {
    /// The worker or a tool it invoked ran out of time.
    Timeout,
    /// The vendor CLI is not signed in, or its credential expired.
    Auth,
    /// A network call failed for a reason other than rate limiting.
    Network,
    /// The vendor throttled or exhausted the account's quota window.
    RateLimit,
    /// A tool the worker invoked returned an error.
    ToolError,
    /// The task itself was impossible or contradictory as specified.
    TaskLogic,
    /// A fault inside Vigla rather than the vendor.
    Internal,
    /// The adapter could not classify the failure.
    Unknown,
}

/// Payload of an [`EventKind::Failure`] — the terminal failure record
/// for a task.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Type)]
pub struct Failure {
    /// Failure message, adapter-normalized.
    pub error: String,
    /// Whether re-running the same task could plausibly succeed.
    pub retryable: bool,
    /// Suggested operator action, when the adapter can offer one.
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub suggestion: Option<String>,
    /// Child process exit status, when the process exited rather than
    /// being killed.
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub exit_code: Option<i32>,
    /// Classification used by the recovery engine.
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub category: Option<FailureCategory>,
}
