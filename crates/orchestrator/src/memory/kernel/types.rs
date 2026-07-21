//! Shared input/output types for the memory kernel public API.
//!
//! Centralised here so that sibling sub-modules (`ratify.rs`, etc.) can
//! import from a single place without pulling in all of `mod.rs`.

use std::path::PathBuf;

use event_schema::memory::{NoteKind, ProposalRejectReason, Scope};

/// Caller-supplied proposal payload. The adapter parses worker
/// stdout into this shape; the kernel runs scanner + persistence.
#[derive(Debug, Clone)]
pub struct ProposalInput {
    pub mission_id: String,
    pub worker_id: String,
    pub kind: NoteKind,
    pub scope: Scope,
    pub body: String,
    pub derived_from: Vec<String>,
    pub evidence_event_ids: Vec<String>,
}

#[derive(Debug, Clone)]
pub enum ProposalOutcome {
    Accepted {
        proposal_id: String,
    },
    Rejected {
        proposal_id: String,
        reason: ProposalRejectReason,
    },
}

/// One element of a batched ratification turn.
#[derive(Debug, Clone)]
pub struct RatifyInput {
    pub proposal_id: String,
    pub decision: RatificationDecision,
    pub reason: String,
}

#[derive(Debug, Clone)]
pub enum RatificationDecision {
    /// Accept. If `normalized_body` is set, emits a `MemoryNormalized`
    /// event with the rewritten body.
    Accept { normalized_body: Option<String> },
    /// Hard reject. Pending row transitions to `state='rejected'`.
    Reject { reason: String },
}

#[derive(Debug, Clone)]
pub enum RatifyOutcome {
    Accepted {
        proposal_id: String,
        note_id: String,
    },
    Rejected {
        proposal_id: String,
        reason: String,
    },
}

/// User-facing pin operation shared by host surfaces. The Tauri command lowers
/// its validated request into this type before entering the kernel.
#[derive(Debug, Clone)]
pub struct PinInput {
    pub kind: NoteKind,
    pub scope: Scope,
    pub body: String,
    pub source: event_schema::memory::AuthorSource,
}

#[derive(Debug, Clone)]
pub enum PinOutcome {
    /// Note created. `promoted` is `true` when the policy shortcut
    /// for `UserAuthored` witnesses cleared the bar.
    Pinned { note_id: String, promoted: bool },
    /// Scanner or oversize check rejected the body. No note created;
    /// only the redacted preview is stored.
    Rejected {
        reason: ProposalRejectReason,
        redacted_preview: String,
    },
}

/// Untyped projection of one `memory_events` row. Used by Tier-2E read
/// surfaces — the host crate translates `event_type` + `payload_json`
/// into a UI DTO. Kept as plain strings here so the kernel has no
/// presentation coupling.
#[derive(Debug, Clone)]
pub struct MemoryEventRow {
    pub event_id: String,
    pub mission_id: Option<String>,
    pub worker_id: Option<String>,
    pub ts: String,
    pub event_type: String,
    pub payload_json: String,
}

/// Untyped projection of one `memory_bundles` row.
#[derive(Debug, Clone)]
pub struct MemoryBundleRow {
    pub bundle_id: String,
    pub mission_id: String,
    pub worker_id: String,
    pub turn: u32,
    pub vendor: String,
    pub page_table_json: String,
}

/// Output of [`super::MemoryKernel::render_for_worker`]. Contains everything
/// the caller needs to (a) launch the worker against a worktree with
/// the right native file in place and (b) verify drift on the next
/// turn.
#[derive(Debug, Clone)]
pub struct RenderedBundle {
    pub bundle_id: String,
    pub block_hash: String,
    pub file_hash: String,
    pub native_file_path: PathBuf,
    pub archived_path: PathBuf,
    pub anchor_open_offset: u64,
    pub anchor_close_offset: u64,
    /// The page table that ended up in the bundle, in slot order.
    pub page_table: Vec<event_schema::memory::BundleSlot>,
}
