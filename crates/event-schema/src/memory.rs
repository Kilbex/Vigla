//! Memory Kernel canonical event types (V3 §1).
//!
//! These events are orchestrator-emitted (not vendor-emitted) and form
//! a parallel log to the worker event stream in [`crate::Event`]. They
//! sit in their own envelope because:
//!
//! 1. Some carry no `worker_id` (e.g. `MemoryBarrier`, user-authored
//!    notes via CLI), which the worker `Event` envelope forbids.
//! 2. Memory events are not sequenced per-worker — they reference
//!    workers but originate at the orchestrator or supervisor.
//!
//! Wire format mirrors [`crate::Event`]:
//!
//! ```jsonc
//! {
//!   "schema_version": "1.0",
//!   "event_id": "01JM4XQK...",
//!   "mission_id": "..." | null,
//!   "worker_id":  "..." | null,
//!   "ts": "2026-05-15T14:22:01.481Z",
//!   "type": "note_authored",
//!   "payload": { ... }
//! }
//! ```

use serde::{Deserialize, Serialize};
use specta::Type;

use crate::Vendor;

/// Memory schema version. Independent of the worker event schema so
/// the two can evolve separately. Bump rules match `docs/event-
/// schema.md` §3.
pub const MEMORY_SCHEMA_VERSION: &str = "1.0";

// ---------------------------------------------------------------------
// Shared vocabulary (V3 §13 — open enums, extensible via taxonomy)
// ---------------------------------------------------------------------

/// Note kind. The seeded values are the four canonical kinds; the
/// `memory_taxonomy` table permits adding more without a code change,
/// in which case unknown variants deserialize via [`NoteKind::Other`].
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize, Type)]
#[serde(rename_all = "snake_case", untagged)]
pub enum NoteKind {
    Standard(StandardNoteKind),
    /// Forward-compatible escape hatch — preserves unknown kinds in
    /// replay without losing data.
    Other(String),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, Type)]
#[serde(rename_all = "snake_case")]
pub enum StandardNoteKind {
    Fact,
    Decision,
    Procedure,
    Hazard,
}

impl NoteKind {
    pub fn as_str(&self) -> &str {
        match self {
            NoteKind::Standard(StandardNoteKind::Fact) => "fact",
            NoteKind::Standard(StandardNoteKind::Decision) => "decision",
            NoteKind::Standard(StandardNoteKind::Procedure) => "procedure",
            NoteKind::Standard(StandardNoteKind::Hazard) => "hazard",
            NoteKind::Other(s) => s,
        }
    }

    /// Build from a taxonomy string. Returns `Other` for any unknown
    /// value — the kernel relies on the `memory_taxonomy` table to
    /// reject truly unknown kinds before this is called.
    #[allow(clippy::should_implement_trait)]
    pub fn from_str(s: &str) -> Self {
        match s {
            "fact" => NoteKind::Standard(StandardNoteKind::Fact),
            "decision" => NoteKind::Standard(StandardNoteKind::Decision),
            "procedure" => NoteKind::Standard(StandardNoteKind::Procedure),
            "hazard" => NoteKind::Standard(StandardNoteKind::Hazard),
            other => NoteKind::Other(other.to_owned()),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, Type)]
#[serde(rename_all = "snake_case")]
pub enum ScopeKind {
    Repo,
    Path,
    Vendor,
    Supervisor,
    Worker,
}

impl ScopeKind {
    pub fn as_str(self) -> &'static str {
        match self {
            ScopeKind::Repo => "repo",
            ScopeKind::Path => "path",
            ScopeKind::Vendor => "vendor",
            ScopeKind::Supervisor => "supervisor",
            ScopeKind::Worker => "worker",
        }
    }

    #[allow(clippy::should_implement_trait)]
    pub fn from_str(s: &str) -> Option<Self> {
        Some(match s {
            "repo" => ScopeKind::Repo,
            "path" => ScopeKind::Path,
            "vendor" => ScopeKind::Vendor,
            "supervisor" => ScopeKind::Supervisor,
            "worker" => ScopeKind::Worker,
            _ => return None,
        })
    }
}

/// Scope of a note. `value` is required for every kind except
/// `repo`, where it is implicit (the repo as a whole).
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize, Type)]
pub struct Scope {
    pub kind: ScopeKind,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub value: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, Type)]
#[serde(rename_all = "snake_case")]
pub enum NoteState {
    Owned,
    Promoted,
    Disputed,
    Invalid,
}

impl NoteState {
    pub fn as_str(self) -> &'static str {
        match self {
            NoteState::Owned => "owned",
            NoteState::Promoted => "promoted",
            NoteState::Disputed => "disputed",
            NoteState::Invalid => "invalid",
        }
    }

    #[allow(clippy::should_implement_trait)]
    pub fn from_str(s: &str) -> Option<Self> {
        Some(match s {
            "owned" => NoteState::Owned,
            "promoted" => NoteState::Promoted,
            "disputed" => NoteState::Disputed,
            "invalid" => NoteState::Invalid,
            _ => return None,
        })
    }
}

// ---------------------------------------------------------------------
// Envelope + discriminated union (V3 §1)
// ---------------------------------------------------------------------

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Type)]
pub struct MemoryEvent {
    pub schema_version: String,
    pub event_id: String,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub mission_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub worker_id: Option<String>,
    pub ts: String,
    #[serde(flatten)]
    pub kind: MemoryEventKind,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Type)]
#[serde(tag = "type", content = "payload", rename_all = "snake_case")]
pub enum MemoryEventKind {
    Proposed(MemoryProposed),
    ProposalRejected(MemoryProposalRejected),
    Normalized(MemoryNormalized),
    Ratified(MemoryRatified),
    Rejected(MemoryRejected),
    Promoted(MemoryPromoted),
    Demoted(MemoryDemoted),
    BundleComposed(MemoryBundleComposed),
    BundleRendered(MemoryBundleRendered),
    Barrier(MemoryBarrier),
    NoteAuthored(MemoryNoteAuthored),
    DriftDetected(MemoryDriftDetected),
    ComposerStale(MemoryComposerStale),
    RetrievalDegraded(MemoryRetrievalDegraded),
    WitnessRecorded(MemoryWitnessRecorded),
    ConfidenceComputed(MemoryConfidenceComputed),
}

// ---------------------------------------------------------------------
// Payloads
// ---------------------------------------------------------------------

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Type)]
pub struct MemoryProposed {
    pub proposal_id: String,
    pub kind: NoteKind,
    pub scope: Scope,
    pub body: String,
    /// Provenance trail for adversarial scoring (V3 §4 threat #2).
    /// Each entry is an opaque source identifier — e.g.
    /// `"worktree:src/x.rs:42"`, `"url:https://..."`, or
    /// `"vendor_file:CLAUDE.md"`. Entries outside the worktree
    /// downgrade the proposal's witness weight in P2.
    #[serde(default)]
    pub derived_from: Vec<String>,
    /// Worker event ids that justify this proposal (e.g. test results,
    /// tool calls, file edits). Used for replay.
    #[serde(default)]
    pub evidence_event_ids: Vec<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, Type)]
#[serde(rename_all = "snake_case")]
pub enum RatifyDecision {
    Accept,
    MergeInto,
    Defer,
    Reject,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Type)]
pub struct MemoryRatified {
    pub proposal_id: String,
    /// Set on `Accept` / `MergeInto`; absent on `Defer` / `Reject`.
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub note_id: Option<String>,
    pub decision: RatifyDecision,
    pub reason: String,
    /// Set on `MergeInto`: the existing note that absorbed this proposal.
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub merged_into_note_id: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Type)]
pub struct MemoryPromoted {
    pub note_id: String,
    pub from_state: NoteState,
    pub to_state: NoteState,
    /// Confidence at promotion time (V3 §2 scoring). Recorded so
    /// replay shows the exact value that cleared the threshold.
    pub confidence: f64,
}

/// Emitted by `try_demote` when a promoted note's confidence drops
/// below its kind's promotion threshold (F-014 fix). Mirrors
/// [`MemoryPromoted`] so replay can reconstruct the full
/// promoted → owned lifecycle. Wraps the confidence event +
/// state UPDATE in a single transaction (F-015 fix).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Type)]
pub struct MemoryDemoted {
    pub note_id: String,
    pub from_state: NoteState,
    pub to_state: NoteState,
    /// Confidence at demotion time — the value that fell below threshold.
    pub confidence: f64,
}

/// One slot in a rendered bundle: a note plus its token cost.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Type)]
pub struct BundleSlot {
    pub slot: u32,
    pub note_id: String,
    pub tokens: u32,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Type)]
pub struct MemoryBundleComposed {
    pub bundle_id: String,
    pub mission_id: String,
    pub worker_id: String,
    pub turn: u32,
    pub vendor: Vendor,
    pub hash: String,
    pub page_table: Vec<BundleSlot>,
    /// Composer audit trace (V3 §7.3). Carries dropped candidates and
    /// scoring decisions as an opaque JSON string. P0 keeps the schema
    /// loose so the composer in P4 can introduce a typed
    /// `ComposerTrace` struct without breaking on-wire compatibility
    /// with archived bundle events.
    #[serde(default)]
    pub trace_json: String,
    #[serde(default)]
    pub prefetched: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Type)]
pub struct MemoryBundleRendered {
    pub bundle_id: String,
    pub native_file_path: String,
    pub anchor_open_offset: u64,
    pub anchor_close_offset: u64,
    pub file_hash: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, Type)]
#[serde(rename_all = "snake_case")]
pub enum BarrierKind {
    Accept,
    Scrub,
    Explicit,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Type)]
pub struct MemoryBarrier {
    pub mission_id: String,
    pub kind: BarrierKind,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, Type)]
#[serde(rename_all = "snake_case")]
pub enum AuthorSource {
    Cli,
    UiPin,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Type)]
pub struct MemoryNoteAuthored {
    pub note_id: String,
    pub source: AuthorSource,
    pub body: String,
}

// ---------------------------------------------------------------------
// P1 — coherence & degradation events (V3 §1.3 / §4)
// ---------------------------------------------------------------------

/// Emitted by the coherence module when a worker's native file no
/// longer matches the bundle that was rendered to it (V3 §4 threat #2
/// reflexive defence). The `file_offset` is the start of the anchor
/// block — diff tooling can use it to localise the drift.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Type)]
pub struct MemoryDriftDetected {
    pub bundle_id: String,
    pub native_file_path: String,
    pub expected_hash: String,
    pub observed_hash: String,
    pub file_offset: u64,
}

/// Emitted when the composer falls back to a previously-rendered
/// bundle because the current pass exceeded its latency budget
/// (V3 §8 — fail-soft is the rule).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Type)]
pub struct MemoryComposerStale {
    pub bundle_id: String,
    pub served_bundle_id: String,
    pub reason: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, Type)]
#[serde(rename_all = "snake_case")]
pub enum DegradedStage {
    Retrieval,
    Composition,
    Render,
}

/// Emitted when a stage exceeded its hard cap and fell through to a
/// reduced-fidelity path (e.g. BM25-only retrieval, greedy-only
/// composition). Replay tooling surfaces these as "running on fumes"
/// markers (V3 §8).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Type)]
pub struct MemoryRetrievalDegraded {
    pub stage: DegradedStage,
    pub elapsed_ms: u64,
    pub fallback: String,
}

// ---------------------------------------------------------------------
// P2 — proposal pipeline + witness signals (V3 §2, §4 threats)
// ---------------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, Type)]
#[serde(rename_all = "snake_case")]
pub enum ProposalRejectReason {
    /// Secret-pattern or high-entropy match. Body never enters the
    /// event store — only the redacted preview does (V3 §4 threat #5).
    Secret,
    /// Body exceeded `NOTE_BODY_CAP_BYTES`.
    Oversize,
    /// Adapter parsed something that failed schema validation.
    Malformed,
}

/// Emitted by the scanner *before* `MemoryProposed` would land. If
/// this event exists for a given `(mission_id, worker_id)` candidate
/// proposal, no corresponding `MemoryProposed` was ever persisted.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Type)]
pub struct MemoryProposalRejected {
    /// Stable id assigned by the adapter at parse time so workers can
    /// reference their own rejected proposals in subsequent turns.
    pub proposal_id: String,
    pub reason: ProposalRejectReason,
    /// Body with suspect spans replaced by `[REDACTED:<len>:<sha8>]`.
    /// Safe to surface in UI / logs.
    pub redacted_preview: String,
}

/// Supervisor rewrote a proposal's body into atomic form. Replay can
/// diff before/after to surface what was changed (V3 §1.1).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Type)]
pub struct MemoryNormalized {
    pub proposal_id: String,
    pub kind: NoteKind,
    pub scope: Scope,
    pub normalized_body: String,
}

/// Supervisor rejected the proposal at the ratification turn (as
/// opposed to the scanner). Reason is human-readable.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Type)]
pub struct MemoryRejected {
    pub proposal_id: String,
    pub reason: String,
}

/// Typed witness signals that feed [`scoring.rs`] (V3 §2). Adding a
/// new variant here is the only place the kernel's confidence formula
/// can grow — keeping the surface narrow keeps the moat defensible.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, Type)]
#[serde(rename_all = "snake_case")]
pub enum WitnessKind {
    UserAuthored,
    UserAccepted,
    TestPassedAfterUse,
    ReviewApproved,
    WorkerProposed,
    DerivedFromUntrustedFile,
    UserScrubbed,
    TestFailedAfterUse,
    ConflictWithHigherConfidence,
    BranchUnmerged,
}

impl WitnessKind {
    /// Default weight (V3 §4.12). Profiles may override at runtime;
    /// the recorded `weight` on `memory_witnesses` row is whichever
    /// value was current when the witness was observed.
    pub fn default_weight(self) -> f64 {
        match self {
            WitnessKind::UserAuthored => 1.0,
            WitnessKind::UserAccepted => 0.4,
            WitnessKind::TestPassedAfterUse => 0.3,
            WitnessKind::ReviewApproved => 0.5,
            WitnessKind::WorkerProposed => 0.05,
            WitnessKind::DerivedFromUntrustedFile => -0.4,
            WitnessKind::UserScrubbed => -0.6,
            WitnessKind::TestFailedAfterUse => -0.3,
            WitnessKind::ConflictWithHigherConfidence => -0.5,
            WitnessKind::BranchUnmerged => -0.2,
        }
    }

    /// True if this kind satisfies the promotion predicate's
    /// "outcome OR human witness" conjunct (V3 §7.5).
    pub fn is_qualifying(self) -> bool {
        matches!(
            self,
            WitnessKind::UserAuthored
                | WitnessKind::UserAccepted
                | WitnessKind::ReviewApproved
                | WitnessKind::TestPassedAfterUse
        )
    }

    pub fn as_str(self) -> &'static str {
        match self {
            WitnessKind::UserAuthored => "user_authored",
            WitnessKind::UserAccepted => "user_accepted",
            WitnessKind::TestPassedAfterUse => "test_passed_after_use",
            WitnessKind::ReviewApproved => "review_approved",
            WitnessKind::WorkerProposed => "worker_proposed",
            WitnessKind::DerivedFromUntrustedFile => "derived_from_untrusted_file",
            WitnessKind::UserScrubbed => "user_scrubbed",
            WitnessKind::TestFailedAfterUse => "test_failed_after_use",
            WitnessKind::ConflictWithHigherConfidence => "conflict_with_higher_confidence",
            WitnessKind::BranchUnmerged => "branch_unmerged",
        }
    }

    #[allow(clippy::should_implement_trait)]
    pub fn from_str(s: &str) -> Option<Self> {
        Some(match s {
            "user_authored" => WitnessKind::UserAuthored,
            "user_accepted" => WitnessKind::UserAccepted,
            "test_passed_after_use" => WitnessKind::TestPassedAfterUse,
            "review_approved" => WitnessKind::ReviewApproved,
            "worker_proposed" => WitnessKind::WorkerProposed,
            "derived_from_untrusted_file" => WitnessKind::DerivedFromUntrustedFile,
            "user_scrubbed" => WitnessKind::UserScrubbed,
            "test_failed_after_use" => WitnessKind::TestFailedAfterUse,
            "conflict_with_higher_confidence" => WitnessKind::ConflictWithHigherConfidence,
            "branch_unmerged" => WitnessKind::BranchUnmerged,
            _ => return None,
        })
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Type)]
pub struct MemoryWitnessRecorded {
    pub witness_id: String,
    pub note_id: String,
    pub kind: WitnessKind,
    pub weight: f64,
    /// The event whose effect this witness records — e.g. the
    /// `barrier` event id for a mission accept/scrub, the `ratified`
    /// event id for a worker proposal, the `note_authored` event id
    /// for a user-pinned note. Together with `(note_id, kind)` this
    /// is the witness's identity for idempotence purposes.
    pub source_event_id: String,
    pub observed_at: String,
}

/// Audit trail for a confidence recomputation. The score is derived
/// (V3 §2 — "Storage. confidence is *derived*, not stored"), but the
/// event lets replay show the score that drove a promotion decision.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Type)]
pub struct MemoryConfidenceComputed {
    pub note_id: String,
    pub confidence: f64,
    /// Witness ids that contributed to this computation, in observation
    /// order. Caller does not need to ship every detail — the event
    /// store has the full witnesses already.
    pub contributing_witnesses: Vec<String>,
}

// ---------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn note_authored_event_roundtrips() {
        let ev = MemoryEvent {
            schema_version: MEMORY_SCHEMA_VERSION.into(),
            event_id: "01JM4XQK0000000000000A".into(),
            mission_id: None,
            worker_id: None,
            ts: "2026-05-15T14:22:01.481Z".into(),
            kind: MemoryEventKind::NoteAuthored(MemoryNoteAuthored {
                note_id: "01JM4XQK0000000000000B".into(),
                source: AuthorSource::Cli,
                body: "Build with `cargo build --workspace`.".into(),
            }),
        };

        let wire = serde_json::to_value(&ev).expect("serialize");
        assert_eq!(wire["type"], "note_authored");
        assert_eq!(wire["payload"]["source"], "cli");
        assert!(wire.get("mission_id").is_none() || wire["mission_id"].is_null());

        let back: MemoryEvent = serde_json::from_value(wire).expect("roundtrip");
        assert_eq!(back, ev);
    }

    #[test]
    fn proposed_event_carries_scope_and_provenance() {
        let ev = MemoryEvent {
            schema_version: MEMORY_SCHEMA_VERSION.into(),
            event_id: "01JM4XQK0000000000000C".into(),
            mission_id: Some("add-logout-7a3f".into()),
            worker_id: Some("01JM4XQK0000000000000D".into()),
            ts: "2026-05-15T14:22:01.481Z".into(),
            kind: MemoryEventKind::Proposed(MemoryProposed {
                proposal_id: "01JM4XQK0000000000000E".into(),
                kind: NoteKind::Standard(StandardNoteKind::Hazard),
                scope: Scope {
                    kind: ScopeKind::Path,
                    value: Some("adapters/claude".into()),
                },
                body: "Resume tokens are host-bound.".into(),
                derived_from: vec!["worktree:src/x.rs:42".into()],
                evidence_event_ids: vec!["01J...".into()],
            }),
        };
        let wire = serde_json::to_value(&ev).unwrap();
        assert_eq!(wire["type"], "proposed");
        assert_eq!(wire["payload"]["scope"]["kind"], "path");
        assert_eq!(wire["payload"]["scope"]["value"], "adapters/claude");
        let back: MemoryEvent = serde_json::from_value(wire).unwrap();
        assert_eq!(back, ev);
    }

    #[test]
    fn unknown_note_kind_preserved_as_other() {
        let raw = json!({
            "schema_version": "1.0",
            "event_id": "01JM",
            "ts": "2026-05-15T14:22:01.481Z",
            "type": "proposed",
            "payload": {
                "proposal_id": "01JM",
                "kind": "lesson",
                "scope": {"kind": "repo"},
                "body": "x"
            }
        });
        let parsed: MemoryEvent = serde_json::from_value(raw).expect("forward-compat parse");
        if let MemoryEventKind::Proposed(p) = parsed.kind {
            assert_eq!(p.kind.as_str(), "lesson");
        } else {
            panic!("wrong variant");
        }
    }

    #[test]
    fn note_kind_serde_uses_taxonomy_strings() {
        for (k, s) in [
            (NoteKind::Standard(StandardNoteKind::Fact), "fact"),
            (NoteKind::Standard(StandardNoteKind::Decision), "decision"),
            (NoteKind::Standard(StandardNoteKind::Procedure), "procedure"),
            (NoteKind::Standard(StandardNoteKind::Hazard), "hazard"),
        ] {
            assert_eq!(k.as_str(), s);
            assert_eq!(NoteKind::from_str(s), k);
            // serde wire form matches the taxonomy string exactly.
            assert_eq!(serde_json::to_value(&k).unwrap(), serde_json::json!(s));
        }
    }
}
