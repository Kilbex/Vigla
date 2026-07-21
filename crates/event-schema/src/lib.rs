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

pub mod memory;
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
    Claude,
    Codex,
    Gemini,
    Antigravity,
    Kiro,
    Copilot,
    Opencode,
    Mock,
}

/// Worker identity record (§2). The orchestrator builds this at spawn
/// time and persists it; events reference the worker by `id`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Type)]
pub struct WorkerInfo {
    pub id: String,
    pub name: String,
    pub vendor: Vendor,
    pub cli_binary: String,
    pub cli_version: Option<String>,
    pub cwd: String,
    pub model: Option<String>,
    pub spawned_at: String,
    pub ended_at: Option<String>,
}

/// Task identity record. `depends_on` drives dependency-aware dispatch.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Type)]
pub struct TaskInfo {
    pub id: String,
    pub parent_id: Option<String>,
    pub title: String,
    pub depends_on: Vec<String>,
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
    pub schema_version: String,
    pub worker_id: String,
    pub task_id: Option<String>,
    pub seq: u64,
    pub ts: String,
    #[serde(flatten)]
    pub kind: EventKind,
}

/// Discriminated payload union. Adjacently tagged: `{"type": "...",
/// "payload": {...}}`. `rename_all = "snake_case"` defines the
/// canonical wire names.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Type)]
#[serde(tag = "type", content = "payload", rename_all = "snake_case")]
pub enum EventKind {
    StateChange(StateChange),
    Log(Log),
    Progress(Progress),
    FileActivity(FileActivity),
    TestResult(TestResult),
    Cost(Cost),
    Dependency(Dependency),
    Completion(Completion),
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
    Idle,
    Planning,
    Executing,
    Blocked,
    Reviewing,
    Done,
    Failed,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Type)]
pub struct StateChange {
    pub state: WorkerState,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub from: Option<WorkerState>,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub note: Option<String>,
}

// ---------------------------------------------------------------------
// §4.2 log
// ---------------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, Type)]
#[serde(rename_all = "snake_case")]
pub enum LogLevel {
    Trace,
    Debug,
    Info,
    Warn,
    Error,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, Type)]
#[serde(rename_all = "snake_case")]
pub enum LogStream {
    Stdout,
    Stderr,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Type)]
pub struct Log {
    pub level: LogLevel,
    pub stream: LogStream,
    pub line: String,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub tag: Option<String>,
}

// ---------------------------------------------------------------------
// §4.3 progress
// ---------------------------------------------------------------------

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Type)]
pub struct Progress {
    pub percent: f64,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub eta_ms: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub note: Option<String>,
}

// ---------------------------------------------------------------------
// §4.4 file_activity
// ---------------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, Type)]
#[serde(rename_all = "snake_case")]
pub enum FileOp {
    Created,
    Modified,
    Deleted,
    Renamed,
    Read,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Type)]
pub struct FileActivity {
    pub path: String,
    pub op: FileOp,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub from_path: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub lines_added: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub lines_removed: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub bytes: Option<u64>,
}

// ---------------------------------------------------------------------
// §4.5 test_result
// ---------------------------------------------------------------------

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Type)]
pub struct TestFailure {
    pub name: String,
    pub message: String,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub file: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub line: Option<u64>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Type)]
pub struct TestResult {
    pub suite: String,
    pub passed: u64,
    pub failed: u64,
    pub skipped: u64,
    pub duration_ms: u64,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub failures: Option<Vec<TestFailure>>,
}

// ---------------------------------------------------------------------
// §4.6 cost
// ---------------------------------------------------------------------

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Type)]
pub struct Cost {
    pub input_tokens: u64,
    pub output_tokens: u64,
    pub usd: f64,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub cache_read_tokens: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub cache_write_tokens: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub model: Option<String>,
}

// ---------------------------------------------------------------------
// §4.7 dependency
// ---------------------------------------------------------------------

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Type)]
pub struct Dependency {
    pub waiting_on: Vec<String>,
    pub reason: String,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub since: Option<String>,
}

// ---------------------------------------------------------------------
// §4.8 completion
// ---------------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, Type)]
#[serde(rename_all = "snake_case")]
pub enum ArtifactKind {
    File,
    Diff,
    Report,
    Url,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Type)]
pub struct Artifact {
    pub kind: ArtifactKind,
    /// Renamed because `ref` is a Rust keyword; the wire name stays
    /// `ref`.
    #[serde(rename = "ref")]
    pub artifact_ref: String,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub label: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Type)]
pub struct Completion {
    pub summary: String,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub artifacts: Option<Vec<Artifact>>,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub duration_ms: Option<u64>,
}

// ---------------------------------------------------------------------
// §4.9 failure
// ---------------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, Type)]
#[serde(rename_all = "snake_case")]
pub enum FailureCategory {
    Timeout,
    Auth,
    Network,
    RateLimit,
    ToolError,
    TaskLogic,
    Internal,
    Unknown,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Type)]
pub struct Failure {
    pub error: String,
    pub retryable: bool,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub suggestion: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub exit_code: Option<i32>,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub category: Option<FailureCategory>,
}
