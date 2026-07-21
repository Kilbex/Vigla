//! Match a worker's [`ContextRequest`] against the memory kernel.
//!
//! Used by the supervisor-side response loop in mission_loop:
//! when a worker emits `RequestContext { kind, detail }`, the
//! supervisor first asks the matcher for a match. On `Found`
//! the supervisor supplies the matched body via the next
//! worker turn (the existing rework-directive channel). On
//! `Missing` the supervisor emits
//! `MissionEventKind::ContextRequestUnmet` followed by an
//! `ArbiterDecided { bound: Some(Scope), evidence }` escalation
//! so the user sees the gap.
//!
//! # Pipeline by phase
//!
//! * **V0** (legacy): substring scan, first-hit-wins.
//! * **V1.1** (current default): alias-expanded BM25 over promoted
//!   notes ([`crate::memory::retrieval::bm25`] +
//!   [`crate::memory::retrieval::alias`]).
//! * **V1.2** (this commit; gated behind a per-candidate vector
//!   lookup, not a feature flag): take BM25's top-N, load each
//!   candidate's stored embedding, embed the query, normalise BM25
//!   over the candidate set, blend with cosine under
//!   [`crate::memory::retrieval::hybrid::DEFAULT_ALPHA`], re-rank,
//!   return top-1. The V1.2 path is *only* taken when the embedder
//!   is enabled AND at least one candidate has a vector — every
//!   other branch falls through to the V1.1 top-1.
//!
//! The substring scan that V0 used remains as a last-ditch fallback
//! when BM25 returns no overlap — this preserves the V0 → V1.1 →
//! V1.2 regression property the evaluation harness checks: every
//! note V0 matched, later phases must also match. Logged at
//! `tracing::debug` when the fallback engages so production
//! telemetry can spot drift.

use crate::memory::error::MemoryError;
use crate::memory::hierarchy::{ListFilter, NoteState};
use crate::memory::retrieval::alias::{expand_aliases, AliasDict};
use crate::memory::retrieval::bm25::{score_all_promoted, Bm25Params};
use crate::memory::retrieval::embed::MODEL_VERSION;
use crate::memory::retrieval::hybrid::{
    cosine_normalized, hybrid_score, normalize_bm25, DEFAULT_ALPHA, DEFAULT_CANDIDATE_POOL,
};
use crate::memory::retrieval::query::RetrievalQuery;
use crate::memory::retrieval::storage;
use crate::memory::retrieval::tokenize::tokenize;
use crate::memory::MemoryKernel;
use crate::recovery::types::ContextRequest;

/// Outcome of [`match_context`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ContextMatch {
    Found {
        note_id: String,
        body: String,
        /// Index of the first byte where `request.detail` (or one
        /// of its substring tokens) appears in `body`. Useful for
        /// snippet rendering. Zero when the whole body is the
        /// match.
        offset: usize,
    },
    Missing,
}

/// Rank promoted notes against `request.detail` using V1.2 hybrid
/// (BM25 + cosine) when the embedder is online and stored vectors
/// are available; otherwise V1.1 alias-expanded BM25; otherwise
/// the V0 substring scan. Returns the top-1. Empty `detail`
/// returns `Missing` to avoid a "match every note" degenerate case.
/// Store errors propagate; callers in the supervisor loop should
/// map them to `Missing` so a transient SQL error doesn't escalate
/// the mission.
pub async fn match_context(
    kernel: &MemoryKernel,
    request: &ContextRequest,
) -> Result<ContextMatch, MemoryError> {
    if request.detail.trim().is_empty() {
        return Ok(ContextMatch::Missing);
    }

    // ---- V1.1+ BM25 + alias candidate generation ----
    let base_tokens = tokenize(&request.detail);
    let dict = AliasDict::seed_default();
    let expanded = expand_aliases(&base_tokens, &dict);
    let q = RetrievalQuery {
        detail: expanded.join(" "),
        kind: None,
        context_hints: Vec::new(),
    };
    let params = Bm25Params::default();
    // Ask for a wide candidate pool; V1.1 callers historically asked
    // for k=1 here. V1.2 needs the top-N so cosine has room to
    // re-rank. When the hybrid path doesn't trigger we still take
    // candidates[0] as the V1.1 top-1, so this widening is free.
    let candidates = score_all_promoted(kernel, &q, &params, DEFAULT_CANDIDATE_POOL).await?;

    if candidates.is_empty() {
        // V0 substring fallback (regression guard).
        tracing::debug!(
            detail = %request.detail,
            "BM25 produced no overlap; falling back to substring scan"
        );
        return substring_fallback(kernel, &request.detail).await;
    }

    // ---- V1.2 hybrid re-rank, when embeddings are available ----
    //
    // Conditions for taking the hybrid branch:
    //   1. The embedder is enabled (feature on AND model loaded).
    //   2. The query encodes successfully.
    //   3. At least one candidate has a vector under MODEL_VERSION.
    //
    // Any miss falls through to V1.1's "first candidate wins"
    // behaviour. That fall-through is the V1.1 regression guard:
    // a Disabled embedder must not change ranking, only let the
    // BM25 path through unchanged.
    let final_id = if !kernel.embedder.is_disabled() {
        match hybrid_rerank_top1(kernel, &request.detail, &candidates).await {
            Ok(Some(id)) => id,
            Ok(None) => candidates[0].note_id.clone(),
            Err(e) => {
                tracing::warn!(
                    target: "memory.retrieval.hybrid",
                    error = %e,
                    "hybrid re-rank failed; falling back to BM25 top-1"
                );
                candidates[0].note_id.clone()
            }
        }
    } else {
        candidates[0].note_id.clone()
    };

    // Fetch the winning note's body for the returned shape.
    let map = kernel
        .store
        .notes_by_ids(std::slice::from_ref(&final_id))
        .await?;
    if let Some(n) = map.get(&final_id) {
        let offset = first_matching_term_offset(&n.body, &base_tokens, &expanded);
        return Ok(ContextMatch::Found {
            note_id: n.id.clone(),
            body: n.body.clone(),
            offset,
        });
    }

    // Race: chosen note vanished between scoring and fetch. Fall
    // through to the substring path rather than erroring.
    tracing::debug!(
        detail = %request.detail,
        "winning candidate vanished; falling back to substring scan"
    );
    substring_fallback(kernel, &request.detail).await
}

/// V1.2 inner loop: embed the query, load each candidate's stored
/// vector, blend with normalised BM25, return the winning note id.
///
/// Returns `Ok(None)` when no candidate has a usable vector — the
/// caller then takes the V1.1 BM25 top-1. Returns `Err` only on SQL
/// or BLOB-corruption errors that the caller is best-equipped to
/// log and recover from.
async fn hybrid_rerank_top1(
    kernel: &MemoryKernel,
    detail: &str,
    candidates: &[crate::memory::retrieval::query::ScoredNote],
) -> Result<Option<String>, MemoryError> {
    let q_vec = match kernel.embedder.embed(detail) {
        Some(v) => v,
        None => return Ok(None), // encode failed; let BM25 stand.
    };

    let mut blended: Vec<(String, f64)> = Vec::with_capacity(candidates.len());
    let bm25_raws: Vec<f64> = candidates.iter().map(|c| c.score).collect();
    let bm25_norm = normalize_bm25(&bm25_raws);

    let mut any_vector_seen = false;
    for (i, cand) in candidates.iter().enumerate() {
        // A `None` here is the common case for an as-yet-un-embedded
        // note (just promoted, backfill hasn't caught up). We let it
        // ride at cosine=0.0 rather than excluding the candidate
        // outright, so the BM25 contribution can still win the
        // ranking when α favours BM25.
        let cosine =
            match storage::get_embedding(&kernel.pool, &cand.note_id, MODEL_VERSION).await? {
                Some(v) => {
                    any_vector_seen = true;
                    cosine_normalized(&q_vec, &v)
                }
                None => 0.0,
            };
        let score = hybrid_score(bm25_norm[i], cosine, DEFAULT_ALPHA);
        blended.push((cand.note_id.clone(), score));
    }

    if !any_vector_seen {
        // No vectors at all — backfill hasn't run / model is broken.
        // Defer to BM25.
        return Ok(None);
    }

    // Sort by hybrid score DESC, note_id ASC for determinism (same
    // tie-break the BM25 pass uses).
    blended.sort_by(|a, b| {
        b.1.partial_cmp(&a.1)
            .unwrap_or(std::cmp::Ordering::Equal)
            .then_with(|| a.0.cmp(&b.0))
    });
    Ok(blended.into_iter().next().map(|(id, _)| id))
}

/// Public top-K hybrid ranker, exposed for the V1.2 evaluation
/// harness. Mirrors [`match_context`]'s pipeline but returns up to
/// `k` note ids in ranked order (no body fetch, no substring
/// fallback). Empty `detail` returns an empty vec.
///
/// Semantics:
/// * When the embedder is online and at least one candidate has a
///   vector, the result is hybrid-blended (BM25 + cosine under
///   `DEFAULT_ALPHA`). Otherwise it's BM25-only (V1.1 behaviour).
/// * Pool size is capped at `max(k, DEFAULT_CANDIDATE_POOL)` so
///   small-k callers still get a meaningful cosine re-rank window.
/// * Determinism: score DESC, note_id ASC (same tie-break as BM25).
pub async fn match_context_top_k(
    kernel: &MemoryKernel,
    detail: &str,
    k: usize,
) -> Result<Vec<String>, MemoryError> {
    if detail.trim().is_empty() || k == 0 {
        return Ok(Vec::new());
    }
    let base_tokens = tokenize(detail);
    let dict = AliasDict::seed_default();
    let expanded = expand_aliases(&base_tokens, &dict);
    let q = RetrievalQuery {
        detail: expanded.join(" "),
        kind: None,
        context_hints: Vec::new(),
    };
    let params = Bm25Params::default();
    let pool = k.max(DEFAULT_CANDIDATE_POOL);
    let candidates = score_all_promoted(kernel, &q, &params, pool).await?;
    if candidates.is_empty() {
        return Ok(Vec::new());
    }

    if kernel.embedder.is_disabled() {
        return Ok(candidates.into_iter().take(k).map(|c| c.note_id).collect());
    }

    let q_vec = match kernel.embedder.embed(detail) {
        Some(v) => v,
        None => return Ok(candidates.into_iter().take(k).map(|c| c.note_id).collect()),
    };
    let raws: Vec<f64> = candidates.iter().map(|c| c.score).collect();
    let bm25_norm = normalize_bm25(&raws);
    let mut blended: Vec<(String, f64)> = Vec::with_capacity(candidates.len());
    let mut any_vector_seen = false;
    for (i, c) in candidates.iter().enumerate() {
        let cosine = match storage::get_embedding(&kernel.pool, &c.note_id, MODEL_VERSION).await? {
            Some(v) => {
                any_vector_seen = true;
                cosine_normalized(&q_vec, &v)
            }
            None => 0.0,
        };
        blended.push((
            c.note_id.clone(),
            hybrid_score(bm25_norm[i], cosine, DEFAULT_ALPHA),
        ));
    }
    if !any_vector_seen {
        return Ok(candidates.into_iter().take(k).map(|c| c.note_id).collect());
    }
    blended.sort_by(|a, b| {
        b.1.partial_cmp(&a.1)
            .unwrap_or(std::cmp::Ordering::Equal)
            .then_with(|| a.0.cmp(&b.0))
    });
    Ok(blended.into_iter().take(k).map(|(id, _)| id).collect())
}

/// Best-effort offset of the first occurrence of any query term in
/// `body`. Tries the verbatim detail tokens first (so existing
/// callers that depend on exact-substring offsets see the same byte
/// position they would have under V0), then alias-expanded tokens.
/// Returns 0 when nothing matches.
fn first_matching_term_offset(body: &str, base: &[String], expanded: &[String]) -> usize {
    let lower_body = body.to_ascii_lowercase();
    for t in base.iter().chain(expanded.iter()) {
        if t.is_empty() {
            continue;
        }
        if let Some(off) = lower_body.find(t.as_str()) {
            return off;
        }
    }
    0
}

/// V0 substring scan, kept for the BM25 zero-overlap fallback.
async fn substring_fallback(
    kernel: &MemoryKernel,
    detail: &str,
) -> Result<ContextMatch, MemoryError> {
    let summaries = kernel
        .store
        .note_list(ListFilter {
            state: Some(NoteState::Promoted),
            ..Default::default()
        })
        .await?;
    for s in summaries {
        let map = match kernel.store.notes_by_ids(std::slice::from_ref(&s.id)).await {
            Ok(m) => m,
            Err(_) => continue,
        };
        let note = match map.get(&s.id) {
            Some(n) => n,
            None => continue,
        };
        if let Some(off) = note.body.find(detail) {
            return Ok(ContextMatch::Found {
                note_id: note.id.clone(),
                body: note.body.clone(),
                offset: off,
            });
        }
    }
    Ok(ContextMatch::Missing)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::memory::hierarchy::{NoteKind, Scope, ScopeKind, StandardNoteKind};
    use crate::memory::{MemoryKernel, PinInput};
    use crate::recovery::types::{ContextRequest, ContextRequestKind};
    use event_schema::memory::AuthorSource;
    use sqlx::sqlite::{SqliteConnectOptions, SqlitePoolOptions};
    use std::str::FromStr;
    use tempfile::TempDir;

    async fn fresh_kernel() -> (MemoryKernel, TempDir) {
        let opts = SqliteConnectOptions::from_str("sqlite::memory:").unwrap();
        let pool = SqlitePoolOptions::new()
            .min_connections(1)
            .max_connections(1)
            .idle_timeout(None)
            .max_lifetime(None)
            .connect_with(opts)
            .await
            .unwrap();
        sqlx::migrate!("./migrations").run(&pool).await.unwrap();
        let root = TempDir::new().unwrap();
        let kernel = MemoryKernel::open(pool, root.path().to_path_buf())
            .await
            .unwrap();
        (kernel, root)
    }

    fn req(detail: &str) -> ContextRequest {
        ContextRequest {
            kind: ContextRequestKind::Documentation,
            detail: detail.to_string(),
        }
    }

    #[tokio::test]
    async fn missing_when_no_notes_present() {
        let (kernel, _root) = fresh_kernel().await;
        let m = match_context(&kernel, &req("anything")).await.unwrap();
        assert!(matches!(m, ContextMatch::Missing));
    }

    #[tokio::test]
    async fn finds_substring_match_in_promoted_note() {
        let (kernel, _root) = fresh_kernel().await;
        let _ = kernel
            .pin_note(PinInput {
                kind: NoteKind::Standard(StandardNoteKind::Fact),
                scope: Scope {
                    kind: ScopeKind::Repo,
                    value: None,
                },
                body: "Use std::pin::Pin not core::pin::Pin in this crate.".into(),
                source: AuthorSource::Cli,
            })
            .await
            .unwrap();

        let m = match_context(&kernel, &req("std::pin::Pin")).await.unwrap();
        match m {
            ContextMatch::Found { body, .. } => {
                assert!(body.contains("std::pin::Pin"));
            }
            _ => panic!("expected Found"),
        }
    }

    #[tokio::test]
    async fn ignores_unpromoted_notes() {
        let (kernel, _root) = fresh_kernel().await;
        // Seed an Owned (not Promoted) note via the store directly.
        // note_add records a UserAuthored witness but does NOT promote;
        // only kernel.pin_note runs try_promote afterwards. So this
        // note stays in NoteState::Owned and the matcher must skip it.
        use crate::memory::NewNote;
        kernel
            .store
            .note_add(
                NewNote {
                    kind: NoteKind::Standard(StandardNoteKind::Fact),
                    scope: Scope {
                        kind: ScopeKind::Repo,
                        value: None,
                    },
                    body: "ephemeral hint about std::pin::Pin".into(),
                },
                crate::memory::NoteAuthor::User {
                    source: AuthorSource::Cli,
                },
            )
            .await
            .unwrap();

        let m = match_context(&kernel, &req("std::pin::Pin")).await.unwrap();
        assert!(matches!(m, ContextMatch::Missing));
    }

    #[tokio::test]
    async fn first_match_wins_over_later_ones() {
        let (kernel, _root) = fresh_kernel().await;
        let _ = kernel
            .pin_note(PinInput {
                kind: NoteKind::Standard(StandardNoteKind::Fact),
                scope: Scope {
                    kind: ScopeKind::Repo,
                    value: None,
                },
                body: "older note about logging".into(),
                source: AuthorSource::Cli,
            })
            .await
            .unwrap();
        let _ = kernel
            .pin_note(PinInput {
                kind: NoteKind::Standard(StandardNoteKind::Fact),
                scope: Scope {
                    kind: ScopeKind::Repo,
                    value: None,
                },
                body: "newer note about logging".into(),
                source: AuthorSource::Cli,
            })
            .await
            .unwrap();

        let m = match_context(&kernel, &req("logging")).await.unwrap();
        match m {
            ContextMatch::Found { body, .. } => {
                assert!(body.contains("note about logging"));
            }
            _ => panic!("expected Found"),
        }
    }

    #[tokio::test]
    async fn empty_detail_returns_missing() {
        let (kernel, _root) = fresh_kernel().await;
        let _ = kernel
            .pin_note(PinInput {
                kind: NoteKind::Standard(StandardNoteKind::Fact),
                scope: Scope {
                    kind: ScopeKind::Repo,
                    value: None,
                },
                body: "any body".into(),
                source: AuthorSource::Cli,
            })
            .await
            .unwrap();
        let m = match_context(&kernel, &req("")).await.unwrap();
        assert!(matches!(m, ContextMatch::Missing));
    }

    // ---- V1.1 properties (BM25 + alias path) ----

    #[tokio::test]
    async fn alias_query_finds_canonical_form() {
        // Query uses "db" but the note's body uses "database". The
        // seeded alias dictionary expands db ↔ database so BM25 hits.
        // V0 substring would have missed (no "db" in body).
        let (kernel, _root) = fresh_kernel().await;
        let _ = kernel
            .pin_note(PinInput {
                kind: NoteKind::Standard(StandardNoteKind::Fact),
                scope: Scope {
                    kind: ScopeKind::Repo,
                    value: None,
                },
                body: "Connection-pool sizing for the project's database server.".into(),
                source: AuthorSource::Cli,
            })
            .await
            .unwrap();

        let m = match_context(&kernel, &req("db")).await.unwrap();
        match m {
            ContextMatch::Found { body, .. } => {
                assert!(body.contains("database"));
            }
            _ => panic!("expected Found via alias expansion"),
        }
    }

    #[tokio::test]
    async fn title_only_outranks_body_only_at_equal_tf() {
        // Two notes of similar size. The query term lives in note A's
        // title only and in note B's body only. BM25 with title_weight
        // = 3.0 must rank A above B.
        let (kernel, _root) = fresh_kernel().await;
        let _ = kernel
            .pin_note(PinInput {
                kind: NoteKind::Standard(StandardNoteKind::Fact),
                scope: Scope {
                    kind: ScopeKind::Repo,
                    value: None,
                },
                body: "# Quokka husbandry guide\n\nMarsupial sanctuary care basics.".into(),
                source: AuthorSource::Cli,
            })
            .await
            .unwrap();
        let _ = kernel
            .pin_note(PinInput {
                kind: NoteKind::Standard(StandardNoteKind::Fact),
                scope: Scope {
                    kind: ScopeKind::Repo,
                    value: None,
                },
                body: "# Marsupial sanctuary care\n\nQuokka husbandry basics.".into(),
                source: AuthorSource::Cli,
            })
            .await
            .unwrap();

        let m = match_context(&kernel, &req("quokka")).await.unwrap();
        match m {
            ContextMatch::Found { body, .. } => {
                // Winner must be the note whose H1 contains "Quokka".
                assert!(
                    body.starts_with("# Quokka husbandry guide"),
                    "title-only note should rank first; got body starting {:?}",
                    &body[..body.len().min(50)]
                );
            }
            _ => panic!("expected Found"),
        }
    }
}
