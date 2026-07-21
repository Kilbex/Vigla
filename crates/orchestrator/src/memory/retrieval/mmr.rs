//! Maximal Marginal Relevance diversification (V1.3 — Phase 3).
//!
//! Standard MMR re-ranker for the retrieval-driven composer
//! ([`crate::memory::composer`]). Sits *after* the V1.2 hybrid
//! scorer ([`crate::memory::retrieval::hybrid`]) — hybrid picks
//! the top-N relevance-ranked candidates; MMR re-ranks them to
//! reduce redundancy in the final bundle.
//!
//! # Algorithm
//!
//! Iterative greedy selection. At each step, pick the candidate
//! `d` that maximises:
//!
//! ```text
//! score(d) = λ · sim(d, query) − (1 − λ) · max_{s ∈ selected} sim(d, s)
//! ```
//!
//! where `sim` is cosine similarity over L2-normalised embeddings
//! (see [`crate::memory::retrieval::embed::EmbedModel::embed`]). The
//! first selection is purely relevance-driven (no `selected` set
//! yet, so the diversity term is 0).
//!
//! # Edge cases
//!
//! * `λ = 1.0` → degenerates to top-K by relevance (diversity term
//!   collapses to 0). This is the "no MMR" reference point.
//! * `λ = 0.0` → picks the highest-relevance candidate first, then
//!   the candidate *least similar* to whatever was already picked
//!   — usually a poor bundle, but useful as a property-test
//!   extreme.
//! * Empty input → empty output.
//! * `k = 0` → empty output.
//! * `k ≥ candidates.len()` → returns all candidates in MMR order
//!   (not raw input order).
//! * `lambda` is clamped to `[0, 1]` (defensive — caller bugs can't
//!   break the ranking invariant).
//! * Mismatched embedding dims for a candidate → that candidate's
//!   similarity with the query is `0.0` (via
//!   [`crate::memory::retrieval::hybrid::cosine_normalized`]).
//!   Doesn't exclude the candidate; just makes it less attractive.
//!
//! # Determinism
//!
//! Ties broken by `(relevance DESC, note_id ASC)`. The note_id
//! tie-break matches the rest of the retrieval stack
//! ([`crate::memory::retrieval::bm25::score_all_promoted`] and
//! [`crate::memory::context_match::match_context_top_k`]) so a
//! same-corpus same-query MMR pass produces bit-identical
//! orderings across runs.

use crate::memory::retrieval::hybrid::cosine_normalized;

/// Inputs to one MMR re-rank pass. Kept as a borrowed view so the
/// caller can build it cheaply from a Vec<ScoredNote> + a parallel
/// Vec<Vec<f32>> without cloning either.
#[derive(Debug)]
pub struct MmrCandidate<'a> {
    pub note_id: &'a str,
    /// Pre-computed relevance score (hybrid blend; the bigger,
    /// the more relevant).
    pub relevance: f64,
    /// L2-normalised embedding. Empty when the note has no stored
    /// vector — MMR then treats inter-doc similarity as 0 for
    /// this candidate, so it neither helps nor hurts diversity
    /// math.
    pub embedding: &'a [f32],
}

/// Re-rank `candidates` under MMR with the given `lambda` and pick
/// the top `k` in selection order. Returns the chosen `note_id`s.
///
/// `lambda` is clamped to `[0, 1]`. Pass `lambda = 1.0` to get a
/// straight top-K by `relevance` (diversity term collapses to 0).
pub fn mmr_rerank(candidates: &[MmrCandidate<'_>], lambda: f64, k: usize) -> Vec<String> {
    if candidates.is_empty() || k == 0 {
        return Vec::new();
    }
    let lambda = lambda.clamp(0.0, 1.0);
    let take = k.min(candidates.len());
    let mut selected: Vec<usize> = Vec::with_capacity(take);
    let mut remaining: Vec<usize> = (0..candidates.len()).collect();

    while selected.len() < take {
        let mut best_idx: Option<usize> = None;
        let mut best_score: f64 = f64::NEG_INFINITY;
        let mut best_relevance: f64 = f64::NEG_INFINITY;
        let mut best_note_id: &str = "";

        for &i in &remaining {
            let cand = &candidates[i];
            // Diversity penalty: max similarity to anything already
            // selected. `0.0` when nothing is selected yet (first
            // pick) — selection then collapses to pure relevance,
            // exactly matching top-1-by-relevance for the first
            // slot.
            let diversity_penalty = selected
                .iter()
                .map(|&j| cosine_normalized(cand.embedding, candidates[j].embedding))
                .fold(0.0f64, f64::max);
            let score = lambda * cand.relevance - (1.0 - lambda) * diversity_penalty;

            // Tie-break: higher relevance first, then note_id ASC.
            // We use a strict `>` then explicit tie comparisons to
            // make the rules transparent (a single composite key
            // would also work but is harder to audit).
            let take_this = if score > best_score {
                true
            } else if (score - best_score).abs() < f64::EPSILON {
                if cand.relevance > best_relevance {
                    true
                } else if (cand.relevance - best_relevance).abs() < f64::EPSILON {
                    cand.note_id < best_note_id
                } else {
                    false
                }
            } else {
                false
            };
            if take_this {
                best_idx = Some(i);
                best_score = score;
                best_relevance = cand.relevance;
                best_note_id = cand.note_id;
            }
        }

        match best_idx {
            Some(i) => {
                selected.push(i);
                remaining.retain(|&j| j != i);
            }
            None => break, // unreachable given the empty check above
        }
    }

    selected
        .into_iter()
        .map(|i| candidates[i].note_id.to_string())
        .collect()
}

/// Diagnostic helper for the diversity property test — average
/// pairwise cosine over the embeddings of a selection. Lower means
/// more diverse. Returns 0 when fewer than 2 items.
pub fn average_pairwise_similarity(embeddings: &[&[f32]]) -> f64 {
    if embeddings.len() < 2 {
        return 0.0;
    }
    let mut sum = 0.0f64;
    let mut pairs = 0usize;
    for i in 0..embeddings.len() {
        for j in (i + 1)..embeddings.len() {
            sum += cosine_normalized(embeddings[i], embeddings[j]);
            pairs += 1;
        }
    }
    sum / pairs as f64
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 4 toy notes in 3D unit-vector space. n1/n2 are duplicates
    /// (identical embeddings), n3 is orthogonal to n1, n4 is on
    /// the diagonal. Relevance is fed as a free parameter per
    /// test.
    fn unit(x: f32, y: f32, z: f32) -> Vec<f32> {
        let mag = (x * x + y * y + z * z).sqrt();
        if mag == 0.0 {
            return vec![0.0; 3];
        }
        vec![x / mag, y / mag, z / mag]
    }

    fn toy_embeddings() -> Vec<Vec<f32>> {
        vec![
            unit(1.0, 0.0, 0.0), // n1 — x axis
            unit(1.0, 0.0, 0.0), // n2 — duplicate of n1
            unit(0.0, 1.0, 0.0), // n3 — orthogonal to n1
            unit(0.0, 0.0, 1.0), // n4 — orthogonal to both n1 and n3
        ]
    }

    fn build<'a>(
        ids: &'a [&'a str],
        relevances: &'a [f64],
        embs: &'a [Vec<f32>],
    ) -> Vec<MmrCandidate<'a>> {
        ids.iter()
            .zip(relevances.iter().zip(embs.iter()))
            .map(|(id, (r, e))| MmrCandidate {
                note_id: id,
                relevance: *r,
                embedding: e.as_slice(),
            })
            .collect()
    }

    // ---- edge cases ----

    #[test]
    fn empty_input_is_empty_output() {
        assert!(mmr_rerank(&[], 0.5, 5).is_empty());
    }

    #[test]
    fn k_zero_is_empty_output() {
        let embs = toy_embeddings();
        let cands = build(&["n1"], &[1.0], &embs[..1]);
        assert!(mmr_rerank(&cands, 0.5, 0).is_empty());
    }

    #[test]
    fn k_larger_than_input_returns_all() {
        let embs = toy_embeddings();
        let cands = build(&["n1", "n3"], &[0.9, 0.7], &embs[..2]);
        let out = mmr_rerank(&cands, 0.7, 50);
        assert_eq!(out.len(), 2);
    }

    #[test]
    fn lambda_clamped_above_one() {
        let embs = toy_embeddings();
        let cands = build(&["n1", "n2", "n3"], &[0.9, 0.8, 0.7], &embs[..3]);
        // Should behave as lambda=1.0 → straight relevance order.
        assert_eq!(
            mmr_rerank(&cands, 5.0, 3),
            vec!["n1".to_string(), "n2".to_string(), "n3".to_string()]
        );
    }

    #[test]
    fn lambda_clamped_below_zero() {
        let embs = toy_embeddings();
        let cands = build(&["n1", "n2", "n3"], &[0.9, 0.8, 0.7], &embs[..3]);
        // lambda=0.0 → first pick by relevance (n1), then maximally
        // diverse from selected (n3 has lower sim with n1 than n2,
        // since n2 is a duplicate).
        let out = mmr_rerank(&cands, -2.0, 2);
        assert_eq!(out, vec!["n1".to_string(), "n3".to_string()]);
    }

    // ---- lambda extremes ----

    #[test]
    fn lambda_one_reproduces_relevance_top_k() {
        // The plan's V1.3 → V1.2 fallback property: λ=1.0 must
        // degenerate to top-K by relevance.
        let embs = toy_embeddings();
        let cands = build(
            &["n3", "n1", "n2", "n4"], // shuffled input order
            &[0.5, 0.9, 0.7, 0.6],
            &embs,
        );
        let out = mmr_rerank(&cands, 1.0, 3);
        assert_eq!(
            out,
            vec!["n1".to_string(), "n2".to_string(), "n4".to_string()]
        );
    }

    #[test]
    fn lambda_zero_diversifies_after_first_pick() {
        // First pick: highest relevance (n1). Second pick: the
        // remaining candidate least similar to n1. n2 is a
        // duplicate of n1 → high similarity → penalised. n3 is
        // orthogonal → low similarity → selected.
        let embs = toy_embeddings();
        let cands = build(&["n1", "n2", "n3"], &[0.9, 0.85, 0.5], &embs[..3]);
        let out = mmr_rerank(&cands, 0.0, 2);
        assert_eq!(out, vec!["n1".to_string(), "n3".to_string()]);
    }

    // ---- diversity property (the headline V1.3 claim) ----

    #[test]
    fn mmr_reduces_average_pairwise_similarity_vs_top_k() {
        // Build a candidate set where the top relevance picks are
        // intentionally redundant — n1 and n2 are duplicates with
        // the two highest relevances. Without MMR, top-3 picks
        // n1+n2+(third). With MMR at λ=0.5, the second pick should
        // swap to a less-similar candidate, reducing average pairwise
        // similarity.
        let embs = toy_embeddings();
        let cands = build(&["n1", "n2", "n3", "n4"], &[0.95, 0.92, 0.60, 0.55], &embs);

        // Top-3 by pure relevance.
        let top_k = mmr_rerank(&cands, 1.0, 3);
        // Top-3 by MMR.
        let mmr_k = mmr_rerank(&cands, 0.5, 3);

        let lookup = |id: &str| -> &[f32] {
            let i = cands.iter().position(|c| c.note_id == id).unwrap();
            cands[i].embedding
        };
        let top_k_embs: Vec<&[f32]> = top_k.iter().map(|s| lookup(s)).collect();
        let mmr_k_embs: Vec<&[f32]> = mmr_k.iter().map(|s| lookup(s)).collect();
        let topk_sim = average_pairwise_similarity(&top_k_embs);
        let mmr_sim = average_pairwise_similarity(&mmr_k_embs);

        assert!(
            mmr_sim < topk_sim,
            "MMR (avg sim {mmr_sim:.3}) must reduce redundancy vs top-K \
             (avg sim {topk_sim:.3}); mmr_k={mmr_k:?}, top_k={top_k:?}"
        );
    }

    // ---- determinism ----

    #[test]
    fn deterministic_across_runs() {
        let embs = toy_embeddings();
        let cands = build(
            &["n1", "n2", "n3", "n4"],
            &[0.9, 0.9, 0.9, 0.9], // all equal → ties forced
            &embs,
        );
        let a = mmr_rerank(&cands, 0.7, 4);
        let b = mmr_rerank(&cands, 0.7, 4);
        assert_eq!(a, b);
        // All-equal relevance + first pick: alphabetical note_id
        // wins (n1 < n2 < n3 < n4).
        assert_eq!(a[0], "n1");
    }

    #[test]
    fn first_pick_breaks_ties_by_note_id_alpha() {
        let embs = toy_embeddings();
        // n2 listed first, but n1 has the same relevance and same
        // embedding → tie-break on note_id ASC picks n1.
        let cands = build(&["n2", "n1"], &[0.5, 0.5], &embs[..2]);
        let out = mmr_rerank(&cands, 0.8, 1);
        assert_eq!(out, vec!["n1".to_string()]);
    }

    // ---- empty embedding (no vector stored yet) ----

    #[test]
    fn empty_embedding_doesnt_panic() {
        // Candidate with no vector: its similarity with everything
        // is 0 (see `cosine_normalized` dim-mismatch path), so it's
        // a neutral diversity contributor.
        let embs = toy_embeddings();
        let empty: Vec<f32> = Vec::new();
        let cands = vec![
            MmrCandidate {
                note_id: "n1",
                relevance: 0.9,
                embedding: embs[0].as_slice(),
            },
            MmrCandidate {
                note_id: "n_empty",
                relevance: 0.8,
                embedding: empty.as_slice(),
            },
        ];
        let out = mmr_rerank(&cands, 0.5, 2);
        assert_eq!(out.len(), 2);
    }
}
