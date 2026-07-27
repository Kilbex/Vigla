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
    /// One of the four seeded kinds.
    Standard(StandardNoteKind),
    /// Forward-compatible escape hatch — preserves unknown kinds in
    /// replay without losing data.
    Other(String),
}

/// The four kinds seeded into `memory_taxonomy`. Promotion thresholds
/// are asymmetric across them (`policy.rs::fallback_threshold`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, Type)]
#[serde(rename_all = "snake_case")]
pub enum StandardNoteKind {
    /// A verifiable statement about the repository.
    Fact,
    /// A choice that was made, and why.
    Decision,
    /// A repeatable sequence of steps.
    Procedure,
    /// A trap: something that looks safe and is not.
    Hazard,
}

impl NoteKind {
    /// Taxonomy string for this kind — the exact value that appears on
    /// the wire and in `memory_taxonomy`.
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

/// What a note's [`Scope`] is anchored to. Closed set — unlike
/// [`NoteKind`], scopes are not taxonomy-extensible.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, Type)]
#[serde(rename_all = "snake_case")]
pub enum ScopeKind {
    /// The repository as a whole. The only kind with no `value`.
    Repo,
    /// A path prefix inside the repository.
    Path,
    /// One vendor's workers ([`Vendor`]).
    Vendor,
    /// The supervisor's own context.
    Supervisor,
    /// A single worker.
    Worker,
}

impl ScopeKind {
    /// Wire and database string for this scope kind.
    pub fn as_str(self) -> &'static str {
        match self {
            ScopeKind::Repo => "repo",
            ScopeKind::Path => "path",
            ScopeKind::Vendor => "vendor",
            ScopeKind::Supervisor => "supervisor",
            ScopeKind::Worker => "worker",
        }
    }

    /// Parse a scope kind from its wire string. `None` for anything
    /// outside the closed set — scopes are not extensible the way
    /// [`NoteKind`] is.
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
    /// What the scope is anchored to.
    pub kind: ScopeKind,
    /// The anchor itself — a path prefix, vendor name, or worker id.
    /// Absent only for [`ScopeKind::Repo`].
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub value: Option<String>,
}

/// Lifecycle state of a ratified note.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, Type)]
#[serde(rename_all = "snake_case")]
pub enum NoteState {
    /// Ratified but not yet trusted enough to enter a bundle
    /// unprompted.
    Owned,
    /// Cleared its kind's confidence threshold; eligible for
    /// composition into worker bundles.
    Promoted,
    /// Contradicted by a higher-confidence note.
    Disputed,
    /// Retired — never composed, kept for replay.
    Invalid,
}

impl NoteState {
    /// Wire and database string for this state.
    pub fn as_str(self) -> &'static str {
        match self {
            NoteState::Owned => "owned",
            NoteState::Promoted => "promoted",
            NoteState::Disputed => "disputed",
            NoteState::Invalid => "invalid",
        }
    }

    /// Parse a note state from its wire string. `None` for anything
    /// outside the closed set.
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

/// Canonical Memory Kernel event envelope. Mirrors [`crate::Event`]
/// but is orchestrator-sequenced, so it carries an `event_id` instead
/// of a per-worker `seq`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Type)]
pub struct MemoryEvent {
    /// Value of [`MEMORY_SCHEMA_VERSION`] at emit time.
    pub schema_version: String,
    /// ULID identifying this event. Also the idempotence key used by
    /// witnesses that reference it.
    pub event_id: String,
    /// Mission the event belongs to. Absent for repository-wide
    /// events such as a user-authored note.
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub mission_id: Option<String>,
    /// Worker the event is attributable to, when there is one.
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub worker_id: Option<String>,
    /// RFC 3339 timestamp, millisecond precision, UTC.
    pub ts: String,
    /// The payload, flattened into `type` + `payload` on the wire.
    #[serde(flatten)]
    pub kind: MemoryEventKind,
}

/// Discriminated payload union for [`MemoryEvent`]. Adjacently
/// tagged, so `rename_all = "snake_case"` fixes the canonical wire
/// names.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Type)]
#[serde(tag = "type", content = "payload", rename_all = "snake_case")]
pub enum MemoryEventKind {
    /// A worker proposed a note. Wire name `proposed`.
    Proposed(MemoryProposed),
    /// The scanner refused a proposal before it was persisted. Wire
    /// name `proposal_rejected`.
    ProposalRejected(MemoryProposalRejected),
    /// The supervisor rewrote a proposal into atomic form. Wire name
    /// `normalized`.
    Normalized(MemoryNormalized),
    /// The supervisor ruled on a proposal. Wire name `ratified`.
    Ratified(MemoryRatified),
    /// The supervisor rejected a proposal at ratification. Wire name
    /// `rejected`.
    Rejected(MemoryRejected),
    /// A note crossed its promotion threshold. Wire name `promoted`.
    Promoted(MemoryPromoted),
    /// A promoted note fell back below threshold. Wire name
    /// `demoted`.
    Demoted(MemoryDemoted),
    /// The composer selected notes for a worker turn. Wire name
    /// `bundle_composed`.
    BundleComposed(MemoryBundleComposed),
    /// A composed bundle was written into a vendor-native file. Wire
    /// name `bundle_rendered`.
    BundleRendered(MemoryBundleRendered),
    /// A mission-level barrier ran. Wire name `barrier`.
    Barrier(MemoryBarrier),
    /// The operator authored a note directly. Wire name
    /// `note_authored`.
    NoteAuthored(MemoryNoteAuthored),
    /// A rendered native file no longer matches its bundle. Wire name
    /// `drift_detected`.
    DriftDetected(MemoryDriftDetected),
    /// The composer served a stale bundle to stay inside budget. Wire
    /// name `composer_stale`.
    ComposerStale(MemoryComposerStale),
    /// A pipeline stage fell through to a reduced-fidelity path. Wire
    /// name `retrieval_degraded`.
    RetrievalDegraded(MemoryRetrievalDegraded),
    /// A witness signal was observed for a note. Wire name
    /// `witness_recorded`.
    WitnessRecorded(MemoryWitnessRecorded),
    /// A note's confidence was recomputed. Wire name
    /// `confidence_computed`.
    ConfidenceComputed(MemoryConfidenceComputed),
}

// ---------------------------------------------------------------------
// Payloads
// ---------------------------------------------------------------------

/// A worker's candidate note, as parsed from its `vigla_memory`
/// output and cleared by the scanner.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Type)]
pub struct MemoryProposed {
    /// Id assigned by the adapter at parse time; stable across the
    /// proposal's whole pipeline.
    pub proposal_id: String,
    /// Taxonomy kind the worker claimed.
    pub kind: NoteKind,
    /// What the note applies to.
    pub scope: Scope,
    /// The proposed note text, post-scanner.
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

/// The four rulings a supervisor can return for a proposal.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, Type)]
#[serde(rename_all = "snake_case")]
pub enum RatifyDecision {
    /// Store the proposal as a new note.
    Accept,
    /// Fold it into an existing note, named by
    /// [`MemoryRatified::merged_into_note_id`].
    MergeInto,
    /// Not decided this turn; the proposal stays pending.
    Defer,
    /// Refused. No note is created.
    Reject,
}

/// The supervisor's ruling on one proposal.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Type)]
pub struct MemoryRatified {
    /// Proposal being ruled on.
    pub proposal_id: String,
    /// Set on `Accept` / `MergeInto`; absent on `Defer` / `Reject`.
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub note_id: Option<String>,
    /// What the supervisor decided.
    pub decision: RatifyDecision,
    /// Human-readable justification, surfaced in the memory UI.
    pub reason: String,
    /// Set on `MergeInto`: the existing note that absorbed this proposal.
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub merged_into_note_id: Option<String>,
}

/// Emitted when a note's confidence clears its kind's promotion
/// threshold and it becomes eligible for composition.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Type)]
pub struct MemoryPromoted {
    /// Note that moved.
    pub note_id: String,
    /// State before the transition.
    pub from_state: NoteState,
    /// State after the transition.
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
    /// Note that moved.
    pub note_id: String,
    /// State before the transition.
    pub from_state: NoteState,
    /// State after the transition.
    pub to_state: NoteState,
    /// Confidence at demotion time — the value that fell below threshold.
    pub confidence: f64,
}

/// One slot in a rendered bundle: a note plus its token cost.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Type)]
pub struct BundleSlot {
    /// Zero-based position in the bundle's page table.
    pub slot: u32,
    /// Note occupying the slot.
    pub note_id: String,
    /// Token cost charged against the bundle's budget.
    pub tokens: u32,
}

/// The composer's selection for one worker turn.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Type)]
pub struct MemoryBundleComposed {
    /// Id of the composed bundle.
    pub bundle_id: String,
    /// Mission the bundle was composed for.
    pub mission_id: String,
    /// Worker the bundle is destined for.
    pub worker_id: String,
    /// Turn index within that worker's run.
    pub turn: u32,
    /// Vendor whose native file format the bundle will render into.
    pub vendor: Vendor,
    /// Content hash of the composed bundle, used to detect drift.
    pub hash: String,
    /// The selected notes, in composition order.
    pub page_table: Vec<BundleSlot>,
    /// Composer audit trace (V3 §7.3). Carries dropped candidates and
    /// scoring decisions as an opaque JSON string. P0 keeps the schema
    /// loose so the composer in P4 can introduce a typed
    /// `ComposerTrace` struct without breaking on-wire compatibility
    /// with archived bundle events.
    #[serde(default)]
    pub trace_json: String,
    /// Note ids speculatively warmed for the next turn.
    #[serde(default)]
    pub prefetched: Vec<String>,
}

/// Emitted once a composed bundle has been written into the vendor's
/// own memory file (`CLAUDE.md` and friends) between Vigla's anchors.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Type)]
pub struct MemoryBundleRendered {
    /// Bundle that was rendered.
    pub bundle_id: String,
    /// Path of the vendor-native file that received it.
    pub native_file_path: String,
    /// Byte offset of the opening anchor in that file.
    pub anchor_open_offset: u64,
    /// Byte offset of the closing anchor in that file.
    pub anchor_close_offset: u64,
    /// Hash of the whole file after rendering, for drift detection.
    pub file_hash: String,
}

/// Why a mission-level memory barrier ran.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, Type)]
#[serde(rename_all = "snake_case")]
pub enum BarrierKind {
    /// The mission was accepted; its proposals settle.
    Accept,
    /// The mission was scrubbed; its proposals are withdrawn.
    Scrub,
    /// The operator forced a barrier outside the mission verdict.
    Explicit,
}

/// A mission-level settlement point for pending proposals.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Type)]
pub struct MemoryBarrier {
    /// Mission whose proposals are settling.
    pub mission_id: String,
    /// What triggered the barrier.
    pub kind: BarrierKind,
}

/// Where an operator-authored note came from.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, Type)]
#[serde(rename_all = "snake_case")]
pub enum AuthorSource {
    /// Written through the command line.
    Cli,
    /// Pinned from the desktop UI.
    UiPin,
}

/// A note the operator wrote directly, bypassing the proposal
/// pipeline. Carries a [`WitnessKind::UserAuthored`] witness, which is
/// what lets it clear promotion on the user-oracle fast path.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Type)]
pub struct MemoryNoteAuthored {
    /// Id of the created note.
    pub note_id: String,
    /// Surface the operator used.
    pub source: AuthorSource,
    /// The note text.
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
    /// Bundle that was rendered into the file.
    pub bundle_id: String,
    /// File whose contents no longer match.
    pub native_file_path: String,
    /// Hash recorded at render time.
    pub expected_hash: String,
    /// Hash observed now.
    pub observed_hash: String,
    /// Byte offset of the anchor block, to localise the diff.
    pub file_offset: u64,
}

/// Emitted when the composer falls back to a previously-rendered
/// bundle because the current pass exceeded its latency budget
/// (V3 §8 — fail-soft is the rule).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Type)]
pub struct MemoryComposerStale {
    /// Bundle the composer was asked to produce.
    pub bundle_id: String,
    /// Previously-rendered bundle served instead.
    pub served_bundle_id: String,
    /// Why the fallback happened.
    pub reason: String,
}

/// Which pipeline stage ran out of budget.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, Type)]
#[serde(rename_all = "snake_case")]
pub enum DegradedStage {
    /// Candidate selection (hybrid ranking down to BM25-only).
    Retrieval,
    /// Bundle assembly (optimal packing down to greedy).
    Composition,
    /// Writing the bundle into the vendor-native file.
    Render,
}

/// Emitted when a stage exceeded its hard cap and fell through to a
/// reduced-fidelity path (e.g. BM25-only retrieval, greedy-only
/// composition). Replay tooling surfaces these as "running on fumes"
/// markers (V3 §8).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Type)]
pub struct MemoryRetrievalDegraded {
    /// Stage that exceeded its cap.
    pub stage: DegradedStage,
    /// How long the stage ran before giving up, in milliseconds.
    pub elapsed_ms: u64,
    /// The reduced-fidelity path taken instead.
    pub fallback: String,
}

// ---------------------------------------------------------------------
// P2 — proposal pipeline + witness signals (V3 §2, §4 threats)
// ---------------------------------------------------------------------

/// Why the pre-event scanner refused a proposal outright.
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
    /// Why the scanner refused it.
    pub reason: ProposalRejectReason,
    /// Body with suspect spans replaced by `[REDACTED:<len>:<sha8>]`.
    /// Safe to surface in UI / logs.
    pub redacted_preview: String,
}

/// Supervisor rewrote a proposal's body into atomic form. Replay can
/// diff before/after to surface what was changed (V3 §1.1).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Type)]
pub struct MemoryNormalized {
    /// Proposal that was rewritten.
    pub proposal_id: String,
    /// Kind after normalization; the supervisor may reclassify.
    pub kind: NoteKind,
    /// Scope after normalization; the supervisor may narrow it.
    pub scope: Scope,
    /// The rewritten body.
    pub normalized_body: String,
}

/// Supervisor rejected the proposal at the ratification turn (as
/// opposed to the scanner). Reason is human-readable.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Type)]
pub struct MemoryRejected {
    /// Proposal that was refused.
    pub proposal_id: String,
    /// Human-readable justification.
    pub reason: String,
}

/// Typed witness signals that feed the confidence score in the
/// orchestrator's `memory/scoring.rs` (V3 §2). Adding a new variant
/// here is the only place the kernel's confidence formula can grow —
/// keeping the surface narrow keeps the moat defensible.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, Type)]
#[serde(rename_all = "snake_case")]
pub enum WitnessKind {
    /// The operator wrote the note themselves.
    UserAuthored,
    /// The operator accepted a mission that used the note.
    UserAccepted,
    /// Tests passed on a task that consumed the note.
    TestPassedAfterUse,
    /// A review approved work that relied on the note.
    ReviewApproved,
    /// A worker proposed it — the weakest positive signal.
    WorkerProposed,
    /// Its provenance points outside the worktree (V3 §4 threat #2).
    DerivedFromUntrustedFile,
    /// The operator scrubbed a mission that used the note.
    UserScrubbed,
    /// Tests failed on a task that consumed the note.
    TestFailedAfterUse,
    /// A higher-confidence note contradicts it.
    ConflictWithHigherConfidence,
    /// The branch the note came from was never merged.
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

    /// Wire and database string for this witness kind.
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

    /// Parse a witness kind from its wire string. `None` for anything
    /// outside the closed set.
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

/// One observed witness signal, recorded against a note.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Type)]
pub struct MemoryWitnessRecorded {
    /// Id of the `memory_witnesses` row.
    pub witness_id: String,
    /// Note the signal is about.
    pub note_id: String,
    /// Which signal was observed.
    pub kind: WitnessKind,
    /// Weight applied — [`WitnessKind::default_weight`] unless a
    /// runtime profile overrode it when the witness was observed.
    pub weight: f64,
    /// The event whose effect this witness records — e.g. the
    /// `barrier` event id for a mission accept/scrub, the `ratified`
    /// event id for a worker proposal, the `note_authored` event id
    /// for a user-pinned note. Together with `(note_id, kind)` this
    /// is the witness's identity for idempotence purposes.
    pub source_event_id: String,
    /// RFC 3339 timestamp of observation; feeds the recency bonus.
    pub observed_at: String,
}

/// Audit trail for a confidence recomputation. The score is derived
/// (V3 §2 — "Storage. confidence is *derived*, not stored"), but the
/// event lets replay show the score that drove a promotion decision.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Type)]
pub struct MemoryConfidenceComputed {
    /// Note that was scored.
    pub note_id: String,
    /// The resulting confidence, in the open interval (0, 1).
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
