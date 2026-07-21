//! Hybrid retrieval (V1.1 → V1.3).
//!
//! - **V1.1:** alias-expanded BM25 over `title + body`
//!   ([`bm25`], [`alias`]); wired into `context_match::match_context`
//!   with a substring fallback for the V0 regression guard.
//! - **V1.2:** optional local ONNX embeddings ([`embed`]) blended with
//!   BM25 via a hybrid scorer ([`hybrid`]).
//! - **V1.3:** MMR diversity ([`mmr`]) plus the retrieval-driven
//!   `MemoryKernel::compose_retrieval` path. All three layers ship;
//!   builds without the `embeddings` feature degrade to BM25.
//!
//! Surface shape:
//!
//! - [`query::RetrievalQuery`] — input. Pure data; no kernel handle.
//! - [`query::ScoredNote`] — output. Includes `terms_matched` for
//!   test-time explainability so a regression in tokenisation is
//!   immediately visible.
//! - [`tokenize::tokenize`] — the single tokeniser shared by BM25 and
//!   indexing. Whitespace + ASCII punctuation split,
//!   ASCII lowercase, no stemming, no Unicode normalisation.
//!   Determinism matters more than recall at this layer.
//! - [`alias::AliasDict`] / [`alias::expand_aliases`] — order-
//!   preserving, deduplicating alias expansion seeded from
//!   `docs/lexicon.md` highlights.
//! - [`bm25::score_all_promoted`] — kernel-driven entry point that
//!   returns ranked [`query::ScoredNote`]s.
//!
//! The evaluation harness locks ranking quality and graceful degradation.

pub mod alias;
pub mod bm25;
pub mod embed;
pub mod hybrid;
pub mod mmr;
pub mod query;
pub mod storage;
pub mod tokenize;
