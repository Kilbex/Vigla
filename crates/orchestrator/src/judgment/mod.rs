//! Completion Judgment.
//!
//! Mission-level "is this done?" verifier. Entry point is
//! `assemble_verdict` (added in task 6); the heuristics live in
//! `risk_band` (task 2), `doc_coverage` (task 3), and `unresolved`
//! (task 4). Outputs a [`CompletionVerdict`] (see [`verdict`]) which
//! the supervisor emits as
//! [`crate::mission_event::MissionEventKind::CompletionVerdictRendered`]
//! at the end of every mission run.
//!
//! Pure module — no IO, no DB, no event-bus. Easy to property-test.

pub mod assemble;
pub mod doc_coverage;
pub mod risk_band;
pub mod unresolved;
pub mod verdict;

pub use assemble::{assemble_verdict, AssembleInputs};
pub use doc_coverage::score_doc_coverage;
pub use risk_band::score_risk;
pub use unresolved::{collect_unresolved, RecoveryBucket, RecoveryHistorySummary, ScrubRecord};
pub use verdict::{CompletionVerdict, RiskBand, UnresolvedIssue};
