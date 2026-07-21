//! Hybrid BM25 + embedding scoring (V1.2 — Phase 2).
//!
//! Pure, kernel-free scoring helpers. The orchestration glue —
//! pulling a candidate set from BM25, loading vectors from
//! `memory_note_embeddings`, embedding the query, blending, and
//! re-ranking — lives in [`crate::memory::context_match`] so this
//! module stays trivially unit-testable.
//!
//! # Design contract
//!
//! * `alpha = 1.0` reproduces the V1.1 BM25-only ranking *exactly*
//!   (modulo determinism tie-break, which is preserved). This is the
//!   safety net for the V1.2 → V1.1 regression guard the harness
//!   asserts.
//! * `alpha = 0.0` is pure cosine. Useful only as a debugging /
//!   property-test extreme.
//! * Normalisation is min-max over the *candidate set*, not the
//!   global corpus. A score of `1.0` therefore means "best in this
//!   query's top-N", not "best in absolute terms". This is the
//!   right semantic for `α` to be interpretable as a relative
//!   weighting between the two signals.
//! * Cosine inputs are assumed already L2-normalised (see
//!   [`crate::memory::retrieval::embed::EmbedModel::embed`]). The
//!   helper here therefore reduces to a dot product. Mismatched
//!   dimensions return `0.0` rather than panicking — graceful
//!   degradation matters more than the marginal correctness signal.
//!
//! # Tunables
//!
//! Tunables are code-owned so a ranking change is reviewed and evaluated.
//! The defaults below come from
//! the design doc §6 (alpha = 0.6 mid-blend; candidate pool of 20
//! gives BM25 enough room without paying for full-corpus cosine):
//!
//! * [`DEFAULT_ALPHA`] = 0.6 — BM25 contribution share.
//! * [`DEFAULT_CANDIDATE_POOL`] = 20 — top-N BM25 docs to re-rank.
//!
//! Changes require the retrieval golden evaluation and benchmark gates.

/// V1.3 default MMR diversity-vs-relevance balance. `0.7` mirrors
/// the design doc §3 budget — leans toward relevance with a clear
/// diversity nudge so near-duplicate notes don't crowd a bundle.
pub const DEFAULT_MMR_LAMBDA: f64 = 0.7;

/// V1.3 default hybrid candidate pool size handed to MMR. Bigger
/// than [`DEFAULT_CANDIDATE_POOL`] (which sizes BM25 → cosine
/// re-rank) because MMR needs headroom to swap in diverse choices
/// that the relevance-only pass would have rejected.
pub const DEFAULT_MMR_POOL: usize = 10;

/// V1.2 default BM25 weight. `0.6` favours lexical signal slightly
/// over semantic — appropriate for a small corpus (≤ a few hundred
/// promoted notes) where BM25 is already strong and embeddings act
/// as a paraphrase booster rather than a replacement.
pub const DEFAULT_ALPHA: f64 = 0.6;

/// V1.2 default candidate pool size. BM25 returns its top-N, and
/// hybrid re-ranks within that set. 20 is large enough to recover
/// paraphrases that BM25 ranks 11th-15th but small enough to keep
/// vector loads bounded.
pub const DEFAULT_CANDIDATE_POOL: usize = 20;

/// Min-max normalise raw BM25 scores into `[0.0, 1.0]` over the
/// candidate set. Returns a new vector preserving input order.
///
/// Edge cases:
/// * Empty input → empty output.
/// * Single element → `[1.0]` (the lone candidate is, by
///   definition, the top of its set).
/// * All scores equal (e.g. tied BM25 → all `0.5`) → all `1.0`. We
///   pick `1.0` over `0.0` so the cosine term still contributes
///   sensibly rather than collapsing the blend to pure embedding.
pub fn normalize_bm25(scores: &[f64]) -> Vec<f64> {
    if scores.is_empty() {
        return Vec::new();
    }
    if scores.len() == 1 {
        return vec![1.0];
    }
    let min = scores.iter().copied().fold(f64::INFINITY, f64::min);
    let max = scores.iter().copied().fold(f64::NEG_INFINITY, f64::max);
    let range = max - min;
    if range <= f64::EPSILON {
        return vec![1.0; scores.len()];
    }
    scores.iter().map(|s| (s - min) / range).collect()
}

/// Cosine similarity for L2-normalised vectors. Equivalent to the
/// dot product when both inputs are unit-length, which the embedder
/// guarantees. Mismatched dimensions return `0.0` so a corrupt row
/// in `memory_note_embeddings` degrades gracefully instead of
/// panicking the supervisor loop.
pub fn cosine_normalized(a: &[f32], b: &[f32]) -> f64 {
    if a.is_empty() || a.len() != b.len() {
        return 0.0;
    }
    let mut sum = 0.0f64;
    for i in 0..a.len() {
        sum += (a[i] as f64) * (b[i] as f64);
    }
    // Float arithmetic can push a unit-vector dot product just past
    // ±1.0; clamp so downstream scoring stays in the documented
    // range.
    sum.clamp(-1.0, 1.0)
}

/// Blend a normalised BM25 score with a cosine similarity under
/// linear weighting. `alpha = 1.0` is pure BM25 (V1.1 ranking
/// reproduced); `alpha = 0.0` is pure cosine. `alpha` is clamped
/// to `[0, 1]` so caller bugs can't break the ranking invariant.
pub fn hybrid_score(bm25_norm: f64, cosine: f64, alpha: f64) -> f64 {
    let a = alpha.clamp(0.0, 1.0);
    a * bm25_norm + (1.0 - a) * cosine
}

#[cfg(test)]
mod tests {
    use super::*;

    // ---- normalize_bm25 ----

    #[test]
    fn normalize_empty_is_empty() {
        assert!(normalize_bm25(&[]).is_empty());
    }

    #[test]
    fn normalize_single_is_one() {
        assert_eq!(normalize_bm25(&[3.7]), vec![1.0]);
    }

    #[test]
    fn normalize_min_max_to_zero_one() {
        let out = normalize_bm25(&[1.0, 3.0, 5.0]);
        assert!((out[0] - 0.0).abs() < 1e-9);
        assert!((out[1] - 0.5).abs() < 1e-9);
        assert!((out[2] - 1.0).abs() < 1e-9);
    }

    #[test]
    fn normalize_all_equal_collapses_to_one() {
        // Avoids div-by-zero and keeps the cosine term meaningful
        // when BM25 has no ranking signal.
        let out = normalize_bm25(&[2.5, 2.5, 2.5]);
        assert_eq!(out, vec![1.0, 1.0, 1.0]);
    }

    #[test]
    fn normalize_preserves_order() {
        let out = normalize_bm25(&[5.0, 1.0, 3.0]);
        assert!(out[0] > out[2] && out[2] > out[1]);
    }

    // ---- cosine_normalized ----

    #[test]
    fn cosine_identical_vectors_is_one() {
        // L2-normalised (1/√3, 1/√3, 1/√3).
        let v = vec![1.0 / 3f32.sqrt(); 3];
        assert!((cosine_normalized(&v, &v) - 1.0).abs() < 1e-6);
    }

    #[test]
    fn cosine_orthogonal_vectors_is_zero() {
        let a = vec![1.0f32, 0.0, 0.0];
        let b = vec![0.0f32, 1.0, 0.0];
        assert!(cosine_normalized(&a, &b).abs() < 1e-9);
    }

    #[test]
    fn cosine_opposite_vectors_is_minus_one() {
        let a = vec![1.0f32, 0.0, 0.0];
        let b = vec![-1.0f32, 0.0, 0.0];
        assert!((cosine_normalized(&a, &b) + 1.0).abs() < 1e-9);
    }

    #[test]
    fn cosine_dim_mismatch_returns_zero() {
        assert_eq!(cosine_normalized(&[1.0, 2.0], &[1.0, 2.0, 3.0]), 0.0);
        assert_eq!(cosine_normalized(&[], &[1.0]), 0.0);
    }

    #[test]
    fn cosine_clamps_to_unit_range() {
        // A tiny float overshoot from a "perfectly" normalised pair
        // shouldn't escape the documented range.
        let a = vec![1.0f32, 0.0];
        let b = vec![1.0000001f32, 0.0];
        let c = cosine_normalized(&a, &b);
        assert!((-1.0..=1.0).contains(&c));
    }

    // ---- hybrid_score ----

    #[test]
    fn alpha_one_is_bm25_only() {
        // The α=1.0 property is the V1.2 → V1.1 regression guard.
        assert_eq!(hybrid_score(0.7, 0.2, 1.0), 0.7);
        assert_eq!(hybrid_score(0.0, 0.9, 1.0), 0.0);
    }

    #[test]
    fn alpha_zero_is_cosine_only() {
        assert_eq!(hybrid_score(0.7, 0.2, 0.0), 0.2);
        assert_eq!(hybrid_score(0.0, 0.9, 0.0), 0.9);
    }

    #[test]
    fn alpha_half_is_midpoint() {
        let s = hybrid_score(0.8, 0.4, 0.5);
        assert!((s - 0.6).abs() < 1e-9);
    }

    #[test]
    fn alpha_clamped_above_one() {
        assert_eq!(hybrid_score(0.5, 0.9, 1.5), 0.5);
    }

    #[test]
    fn alpha_clamped_below_zero() {
        assert_eq!(hybrid_score(0.5, 0.9, -0.3), 0.9);
    }

    // ---- end-to-end property: α=1.0 preserves BM25 ranking ----

    /// Property test: feeding a set of (bm25_raw, cosine) pairs
    /// through normalise + hybrid with α=1.0 must produce the same
    /// *ranking* as sorting the raw BM25 scores. This is the
    /// invariant the V1.1 regression guard depends on.
    #[test]
    fn alpha_one_preserves_bm25_ranking() {
        let raw_bm25 = [2.1, 5.4, 0.7, 3.3, 4.0];
        // Adversarial cosines: inversely correlated with BM25.
        let cosines = [0.9, 0.1, 0.95, 0.4, 0.2];
        let norm = normalize_bm25(&raw_bm25);
        let hybrid: Vec<f64> = norm
            .iter()
            .zip(cosines.iter())
            .map(|(b, c)| hybrid_score(*b, *c, 1.0))
            .collect();

        // Ranking from raw BM25:
        let mut bm25_order: Vec<usize> = (0..raw_bm25.len()).collect();
        bm25_order.sort_by(|a, b| raw_bm25[*b].partial_cmp(&raw_bm25[*a]).unwrap());
        // Ranking from hybrid scores with α=1:
        let mut hybrid_order: Vec<usize> = (0..hybrid.len()).collect();
        hybrid_order.sort_by(|a, b| hybrid[*b].partial_cmp(&hybrid[*a]).unwrap());

        assert_eq!(
            bm25_order, hybrid_order,
            "α=1.0 must reproduce BM25 ranking exactly"
        );
    }

    #[test]
    fn alpha_zero_preserves_cosine_ranking() {
        let raw_bm25 = [2.1, 5.4, 0.7, 3.3, 4.0];
        let cosines = [0.9, 0.1, 0.95, 0.4, 0.2];
        let norm = normalize_bm25(&raw_bm25);
        let hybrid: Vec<f64> = norm
            .iter()
            .zip(cosines.iter())
            .map(|(b, c)| hybrid_score(*b, *c, 0.0))
            .collect();

        let mut cos_order: Vec<usize> = (0..cosines.len()).collect();
        cos_order.sort_by(|a, b| cosines[*b].partial_cmp(&cosines[*a]).unwrap());
        let mut hybrid_order: Vec<usize> = (0..hybrid.len()).collect();
        hybrid_order.sort_by(|a, b| hybrid[*b].partial_cmp(&hybrid[*a]).unwrap());

        assert_eq!(cos_order, hybrid_order);
    }
}
