//! Memory hierarchy types and invariants (V3 §2 / §3).
//!
//! Re-exports the canonical vocabulary from `event-schema::memory`
//! and adds orchestrator-side domain types (full notes, summaries,
//! filters) plus tier-level constants. The wire format lives in
//! `event-schema`; this module is where the kernel speaks to the rest
//! of the orchestrator.

pub use event_schema::memory::{
    AuthorSource, BarrierKind, BundleSlot, MemoryBarrier, MemoryBundleComposed,
    MemoryBundleRendered, MemoryEvent, MemoryEventKind, MemoryNoteAuthored, MemoryPromoted,
    MemoryProposed, MemoryRatified, NoteKind, NoteState, RatifyDecision, Scope, ScopeKind,
    StandardNoteKind, MEMORY_SCHEMA_VERSION,
};

// ---------------------------------------------------------------------
// Tier-level constants (V3 §3 — bounded resources, fail-soft)
// ---------------------------------------------------------------------

/// Default per-worker T1 token budget. Rendering adapters own their effective
/// budgets; this value is the fail-soft fallback used by generic callers.
pub const T1_MAX_TOKENS_DEFAULT: usize = 1200;

/// Per-mission fault budget — how many `memory.fetch` requests a worker
/// may issue before the kernel starts denying them.
pub const FAULT_BUDGET_PER_MISSION: u8 = 8;

/// Per-note body cap. Atomic notes only; encourages splitting.
pub const NOTE_BODY_CAP_BYTES: usize = 4 * 1024;

// ---------------------------------------------------------------------
// Domain types — orchestrator-internal views of the memory store
// ---------------------------------------------------------------------

/// Fully-loaded note. Body is read from disk on demand by
/// [`crate::memory::MemoryStore::note_show`].
#[derive(Debug, Clone)]
pub struct Note {
    pub id: String,
    pub kind: NoteKind,
    pub scope: Scope,
    pub body: String,
    pub body_hash: String,
    pub state: NoteState,
    pub created_event_id: String,
    pub created_at: String,
    pub last_verified_at: Option<String>,
    /// Curator-facing title, extracted from the body's first H1 at
    /// write time. `None` for notes that have no H1 (legacy prose) or
    /// whose body file was unreadable during backfill. V1.1 BM25
    /// scoring uses titles as a high-weight signal; consumers that
    /// only need identity + metadata should keep using `NoteSummary`.
    pub title: Option<String>,
}

/// Lightweight projection for `note_list`. Avoids reading bodies off
/// disk when the caller only needs identity + metadata.
#[derive(Debug, Clone)]
pub struct NoteSummary {
    pub id: String,
    pub kind: NoteKind,
    pub scope: Scope,
    pub state: NoteState,
    pub created_at: String,
}

/// Filter passed to `note_list`. Each field is independent; `None`
/// means "any". Implementations compose into SQL `AND` predicates.
#[derive(Debug, Clone, Default)]
pub struct ListFilter {
    pub kind: Option<NoteKind>,
    pub state: Option<NoteState>,
    pub scope_kind: Option<ScopeKind>,
    pub scope_value: Option<String>,
}

/// Carrier for `note_add` callers (user-authored notes in P0; the
/// supervisor ratification path lands in P2).
#[derive(Debug, Clone, Copy)]
pub enum NoteAuthor {
    User { source: AuthorSource },
}

/// MOESI-Lite states (V3 §3). Kept for cross-module typing; the in-row
/// representation is `memory_notes.state`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum MoesiState {
    Modified,
    Owned,
    Exclusive,
    Shared,
    Invalid,
}

impl MoesiState {
    pub fn from_note_state(s: NoteState) -> Self {
        match s {
            NoteState::Owned => MoesiState::Owned,
            NoteState::Promoted => MoesiState::Exclusive,
            NoteState::Disputed => MoesiState::Modified,
            NoteState::Invalid => MoesiState::Invalid,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn moesi_mapping_is_total() {
        for s in [
            NoteState::Owned,
            NoteState::Promoted,
            NoteState::Disputed,
            NoteState::Invalid,
        ] {
            let _ = MoesiState::from_note_state(s);
        }
    }
}
