//! Vigla Memory Kernel.
//!
//! Local, auditable, event-sourced long-term memory. The kernel is the
//! single writer to project memory; vendor CLI native files
//! (`CLAUDE.md`, `AGENTS.md`, `GEMINI.md`) are treated as untrusted
//! delivery channels that we *render into*, never read from for truth.
//!
//! ## Capabilities
//!
//! * **Storage and delivery** — event schema, SQLite store, promotion policy,
//!   vendor renderers, composer, anchor I/O, and drift detection.
//! * **Governance** — proposal scanner + pipeline, witnesses,
//!   stateless confidence scoring, batched supervisor ratification,
//!   barrier-driven reflection. The full closed loop: worker
//!   proposes → supervisor ratifies → mission accept promotes →
//!   next mission's composer picks it up. Audited 2026-05-18;
//!   `kernel/` directory holds the facade and per-concern submodules
//!   (mod, types, ratify, barrier, proposal, pin, compose, sweep,
//!   query); integration + property + action-replay-determinism
//!   tests cover the closed loop; criterion benchmarks have
//!   baselines in `benches/memory/baselines.json`.
//! * **Hybrid retrieval** — alias-expanded BM25 replaced the original
//!   substring scan; the optional embeddings feature layers fastembed-rs
//!   (MiniLM-L6-v2, 384-dim, L2-normalised) on
//!   top under per-candidate-pool min-max BM25 normalisation + a
//!   linear blend (α = 0.6 in [`retrieval::hybrid`]); V1.3 added
//!   Maximal Marginal Relevance diversity ([`retrieval::mmr`],
//!   λ = 0.7) and the retrieval-driven composer
//!   ([`MemoryKernel::compose_retrieval`] +
//!   [`attach_with_retrieval`]). The supervisor's `run_task` path
//!   now builds a [`RetrievalBrief`] from the mission objective,
//!   task title, and upstream handoffs, calls retrieval-driven
//!   attach first, and falls back to the manual + budget path on
//!   any failure (graceful degradation, fail-soft contract
//!   preserved). Promoted notes are embedded on-promote (via
//!   `pin_note`) with a kernel-open backfill catching anything the
//!   live path missed. Off behind the `embeddings` cargo feature so
//!   vanilla `cargo build` stays light; when the feature is off —
//!   or model load fails, or a note has no vector — the hybrid
//!   path collapses to V1.1 BM25 ranking with no change in output.
//!   V0 → V1.1 → V1.2 metric delta on the 30-query golden set:
//!   Recall@3 0.167 → 0.867 → 0.867, MRR 0.167 → 0.844 → 0.844
//!   (lexically tight corpus — BM25 already saturates the
//!   achievable ranking; V1.2 + V1.3 ship the substrate for
//!   paraphrase-heavy production use). V1.3 composer overlap-vs-
//!   reference: 0.960 (gate ≥ 0.80); worst-case
//!   compose_retrieval latency: 13.5 ms (budget ≤ 300 ms).

mod adapter;
mod adapters;
mod archive;
mod attach;
pub(crate) mod coherence;
mod composer;
pub mod context_match;
mod error;
pub mod handoff;
pub mod hierarchy;
pub mod ids;
pub mod intent_router;
pub mod intent_sink;
mod kernel;
pub mod policy;
pub mod reflection;
pub mod registry;
pub mod retrieval;
pub mod scanner;
pub mod scoring;
mod store;
pub mod witnesses;

pub use adapter::{estimate_tokens, MemoryAdapter, RenderedSlot};
pub(crate) use adapters::claude::{
    ANCHOR_CLOSE as MEMORY_ANCHOR_CLOSE, ANCHOR_OPEN as MEMORY_ANCHOR_OPEN,
};
pub use adapters::{ClaudeMemoryAdapter, CodexMemoryAdapter, GeminiMemoryAdapter};
pub use archive::DEFAULT_EVENTS_RETENTION_DAYS;
pub use attach::{
    attach_to_worktree, attach_to_worktree_with_budget, attach_with_retrieval, vendor_for_model,
    DEFAULT_TOKEN_BUDGET,
};
pub use coherence::{
    compose_file_contents, detect_drift, find_anchor_span, write_anchor_block, AnchorSpan,
    AnchorWriteOutcome, DriftStatus,
};
pub use composer::{BundleBrief, ComposedBundle, Composer, RetrievalBrief};
pub use context_match::{match_context, match_context_top_k, ContextMatch};
pub use error::MemoryError;
pub use handoff::{list_handoffs_for_mission, persist_handoff, HandoffNote};
pub use hierarchy::{
    ListFilter, MoesiState, Note, NoteAuthor, NoteSummary, FAULT_BUDGET_PER_MISSION,
    NOTE_BODY_CAP_BYTES, T1_MAX_TOKENS_DEFAULT,
};
pub use intent_router::{route_intent, route_propose};
pub use intent_sink::{KernelIntentSink, MemoryIntentSink};
pub use kernel::{
    MemoryBundleRow, MemoryEventRow, MemoryKernel, PinInput, PinOutcome, ProposalInput,
    ProposalOutcome, RatificationDecision, RatifyInput, RatifyOutcome, RenderedBundle,
    RetrievalTelemetry,
};
pub use reflection::ReflectionOutcome;
pub use registry::MemoryRegistry;
pub use scanner::{redact_preview, scan, MatchReason, ScanResult};
pub use scoring::{confidence, confidence_cached, confidence_now, AGE_W, CONF_W, WIT_W};
pub use store::{MemoryStore, NewNote};
pub use witnesses::{has_user_authored, qualifying_count, Recorded, Witness};

// Re-export the wire-format vocabulary so callers don't need to reach
// into `event-schema` directly.
pub use event_schema::memory::{
    AuthorSource, BarrierKind, NoteKind, NoteState, ProposalRejectReason, RatifyDecision, Scope,
    ScopeKind, StandardNoteKind, WitnessKind, MEMORY_SCHEMA_VERSION,
};
pub use policy::{
    fallback_threshold, predicate, promotion_threshold, HoldReason, PromotionDecision,
};
