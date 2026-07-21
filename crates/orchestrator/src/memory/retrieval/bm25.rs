//! BM25 scoring with title-weighted field boost (V1.1).
//!
//! Single-shot scorer: builds a [`CorpusStats`] over all promoted
//! notes on each call, scores every doc against the query, returns
//! the top-K. No inverted index yet — V1.1 sizes the corpus at
//! hundreds, not thousands, of promoted notes; the [O(N · |query|)]
//! per-call cost is well under the 50 ms p99 target. The plan's open
//! question #1 schedules an inverted-index re-evaluation at the V1.2
//! boundary if N · |query| crosses 50 ms in the wild.
//!
//! Title weighting: title tokens contribute `title_weight ×` to the
//! per-document term frequency (we treat title as a "boosted field"
//! folded into the same bag). At equal body TF this makes title hits
//! outrank body hits by exactly `title_weight`.
//!
//! Pure functions ([`bm25_score`], [`build_corpus_stats`]) are unit
//! tested with a toy corpus; the kernel-driven entry point
//! ([`score_all_promoted`]) is exercised by the V1.1 evaluation
//! harness.
//!
//! Determinism: every step iterates [`std::collections::BTreeMap`]s
//! or sort()-ed [`Vec`]s, so two runs over the same corpus + query
//! produce byte-identical [`ScoredNote`] ordering — including the
//! tie-breaker on `note_id` ascending.

use std::collections::{BTreeMap, HashMap};

use crate::memory::error::MemoryError;
use crate::memory::hierarchy::{ListFilter, NoteState};
use crate::memory::retrieval::query::{RetrievalQuery, ScoredNote};
use crate::memory::retrieval::tokenize::tokenize;
use crate::memory::MemoryKernel;

/// BM25 tunables. Defaults match Robertson's empirical recommendations
/// (`k1 = 1.2`, `b = 0.75`) plus a generous title boost since notes
/// are short and the H1 carries a lot of signal.
#[derive(Debug, Clone, Copy)]
pub struct Bm25Params {
    pub k1: f64,
    pub b: f64,
    pub title_weight: f64,
}

impl Default for Bm25Params {
    fn default() -> Self {
        Self {
            k1: 1.2,
            b: 0.75,
            title_weight: 3.0,
        }
    }
}

/// Corpus-level statistics. Built once per `score_all_promoted` call.
#[derive(Debug, Clone)]
pub struct CorpusStats {
    pub n_docs: usize,
    pub avg_doc_len: f64,
    /// Document frequency: how many docs contain each term at least
    /// once. `BTreeMap` for determinism in iteration order during
    /// debug printing / tests.
    pub df: BTreeMap<String, usize>,
}

/// One indexed document: weighted token bag + the body length used by
/// BM25's length normalisation. `tf[term]` is the *boosted* count
/// (title tokens count `title_weight ×`).
#[derive(Debug, Clone)]
pub struct IndexedDoc {
    pub note_id: String,
    pub tf: HashMap<String, f64>,
    pub doc_len: f64,
}

impl IndexedDoc {
    /// Build an [`IndexedDoc`] from a title (optional) + body and
    /// the configured title weight. Lowercased ASCII tokenisation
    /// via [`tokenize`].
    pub fn build(
        note_id: impl Into<String>,
        title: Option<&str>,
        body: &str,
        title_weight: f64,
    ) -> Self {
        let mut tf: HashMap<String, f64> = HashMap::new();
        let mut doc_len = 0.0f64;
        if let Some(t) = title {
            for tok in tokenize(t) {
                *tf.entry(tok).or_default() += title_weight;
                doc_len += title_weight;
            }
        }
        for tok in tokenize(body) {
            *tf.entry(tok).or_default() += 1.0;
            doc_len += 1.0;
        }
        Self {
            note_id: note_id.into(),
            tf,
            doc_len,
        }
    }
}

/// Build a [`CorpusStats`] from the indexed corpus.
///
/// Empty corpus yields `n_docs = 0` and `avg_doc_len = 0.0`; downstream
/// scoring returns an empty `Vec<ScoredNote>` rather than producing
/// NaNs from a /0.
pub fn build_corpus_stats(docs: &[IndexedDoc]) -> CorpusStats {
    let n_docs = docs.len();
    if n_docs == 0 {
        return CorpusStats {
            n_docs: 0,
            avg_doc_len: 0.0,
            df: BTreeMap::new(),
        };
    }
    let total_len: f64 = docs.iter().map(|d| d.doc_len).sum();
    let avg_doc_len = total_len / n_docs as f64;
    let mut df: BTreeMap<String, usize> = BTreeMap::new();
    for d in docs {
        for term in d.tf.keys() {
            *df.entry(term.clone()).or_default() += 1;
        }
    }
    CorpusStats {
        n_docs,
        avg_doc_len,
        df,
    }
}

/// BM25 (Robertson/Spärck Jones) score of one doc against one
/// query, summed over query terms. Reference formula:
///
/// `score = Σ_term IDF(term) · ((k1+1) · tf) / (k1·(1-b + b·|d|/avgdl) + tf)`
///
/// where `IDF(term) = ln((N - df + 0.5) / (df + 0.5) + 1)` (the
/// "Okapi-corrected" form that keeps IDF ≥ 0 even when the term
/// appears in more than half the corpus).
///
/// Terms not in the doc contribute 0; terms not in the corpus
/// (df = 0) also contribute 0.
pub fn bm25_score(
    query_terms: &[String],
    doc: &IndexedDoc,
    stats: &CorpusStats,
    params: &Bm25Params,
) -> f64 {
    if stats.n_docs == 0 || stats.avg_doc_len == 0.0 {
        return 0.0;
    }
    let mut total = 0.0f64;
    let n = stats.n_docs as f64;
    let len_norm = params.k1 * (1.0 - params.b + params.b * (doc.doc_len / stats.avg_doc_len));
    for term in query_terms {
        let tf = match doc.tf.get(term) {
            Some(&v) if v > 0.0 => v,
            _ => continue,
        };
        let df = *stats.df.get(term).unwrap_or(&0) as f64;
        if df <= 0.0 {
            continue;
        }
        let idf = ((n - df + 0.5) / (df + 0.5) + 1.0).ln();
        let numer = (params.k1 + 1.0) * tf;
        let denom = len_norm + tf;
        total += idf * (numer / denom);
    }
    total
}

/// Rank all promoted notes in `kernel` against `query`, return the
/// top-K by score. Ties broken by `note_id` ascending so the result
/// is deterministic across runs.
///
/// `k` is the number of results to return; callers asking for top-1
/// pass `k = 1`. `params` is `Bm25Params::default()` in the common
/// case (V1.1 ships a single param set).
pub async fn score_all_promoted(
    kernel: &MemoryKernel,
    query: &RetrievalQuery,
    params: &Bm25Params,
    k: usize,
) -> Result<Vec<ScoredNote>, MemoryError> {
    let query_terms = tokenize(&query.detail);
    if query_terms.is_empty() {
        return Ok(Vec::new());
    }

    let summaries = kernel
        .store
        .note_list(ListFilter {
            state: Some(NoteState::Promoted),
            ..Default::default()
        })
        .await?;
    if summaries.is_empty() {
        return Ok(Vec::new());
    }

    let ids: Vec<String> = summaries.iter().map(|s| s.id.clone()).collect();
    let by_id = kernel.store.notes_by_ids(&ids).await?;

    let mut docs: Vec<IndexedDoc> = Vec::with_capacity(by_id.len());
    for id in &ids {
        if let Some(n) = by_id.get(id) {
            docs.push(IndexedDoc::build(
                n.id.clone(),
                n.title.as_deref(),
                &n.body,
                params.title_weight,
            ));
        }
    }
    let stats = build_corpus_stats(&docs);

    let mut scored: Vec<ScoredNote> = docs
        .iter()
        .map(|d| {
            let score = bm25_score(&query_terms, d, &stats, params);
            let terms_matched: Vec<String> = query_terms
                .iter()
                .filter(|t| d.tf.contains_key(*t))
                .cloned()
                .collect();
            ScoredNote {
                note_id: d.note_id.clone(),
                score,
                terms_matched,
            }
        })
        .filter(|s| s.score > 0.0)
        .collect();

    // Determinism: stable score-descending, note_id-ascending order.
    scored.sort_by(|a, b| {
        b.score
            .partial_cmp(&a.score)
            .unwrap_or(std::cmp::Ordering::Equal)
            .then_with(|| a.note_id.cmp(&b.note_id))
    });
    scored.truncate(k);
    Ok(scored)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn toy_docs() -> Vec<IndexedDoc> {
        // Three small docs over an authentication / database vocab.
        vec![
            IndexedDoc::build(
                "n1",
                Some("Authentication overview"),
                "JWT validation and OAuth tokens.",
                3.0,
            ),
            IndexedDoc::build(
                "n2",
                Some("Database pool tuning"),
                "Connection pool sizing for SQLite.",
                3.0,
            ),
            IndexedDoc::build(
                "n3",
                Some("Notes about logging"),
                "tracing crate and span context. JWT also appears here once.",
                3.0,
            ),
        ]
    }

    #[test]
    fn corpus_stats_basic_shape() {
        let docs = toy_docs();
        let s = build_corpus_stats(&docs);
        assert_eq!(s.n_docs, 3);
        assert!(s.avg_doc_len > 0.0);
        // "jwt" appears in n1 (body) and n3 (body) — df = 2.
        assert_eq!(*s.df.get("jwt").unwrap(), 2);
        // "authentication" appears only in n1's title — df = 1.
        assert_eq!(*s.df.get("authentication").unwrap(), 1);
    }

    #[test]
    fn empty_corpus_returns_zero_score_without_nan() {
        let stats = build_corpus_stats(&[]);
        let doc = IndexedDoc::build("none", None, "", 3.0);
        let s = bm25_score(&["jwt".into()], &doc, &stats, &Bm25Params::default());
        assert_eq!(s, 0.0);
    }

    #[test]
    fn empty_query_is_zero() {
        let docs = toy_docs();
        let stats = build_corpus_stats(&docs);
        let s = bm25_score(&[], &docs[0], &stats, &Bm25Params::default());
        assert_eq!(s, 0.0);
    }

    #[test]
    fn title_weight_boosts_title_only_match_over_body_only_match() {
        // Two docs of similar length: doc A has the rare term in its
        // title; doc B has the same term once in its body. At equal
        // document frequency, A must outrank B.
        let docs = vec![
            IndexedDoc::build(
                "a",
                Some("Quokka husbandry"),
                "Marsupial sanctuary care.",
                3.0,
            ),
            IndexedDoc::build(
                "b",
                Some("Marsupial sanctuary care"),
                "Quokka husbandry.",
                3.0,
            ),
        ];
        let stats = build_corpus_stats(&docs);
        let q: Vec<String> = vec!["quokka".into()];
        let p = Bm25Params::default();
        let sa = bm25_score(&q, &docs[0], &stats, &p);
        let sb = bm25_score(&q, &docs[1], &stats, &p);
        assert!(sa > sb, "title-only ({sa}) should outrank body-only ({sb})");
    }

    #[test]
    fn rare_term_outranks_common_term() {
        // Term present in fewer docs should produce a higher score
        // (IDF effect). Build a corpus where "rareword" appears once
        // and "common" appears in all three.
        let docs = vec![
            IndexedDoc::build("a", None, "rareword common common common", 3.0),
            IndexedDoc::build("b", None, "common common common common", 3.0),
            IndexedDoc::build("c", None, "common common common common", 3.0),
        ];
        let stats = build_corpus_stats(&docs);
        let p = Bm25Params::default();
        let s_rare = bm25_score(&["rareword".into()], &docs[0], &stats, &p);
        let s_common = bm25_score(&["common".into()], &docs[0], &stats, &p);
        assert!(
            s_rare > s_common,
            "rare ({s_rare}) should outrank common ({s_common}) due to IDF"
        );
    }

    #[test]
    fn deterministic_ordering_with_tie_break_on_id() {
        // Same body in two docs → identical scores → tie-break by id
        // ascending. Verifies the sort is stable for the eval harness.
        let docs = vec![
            IndexedDoc::build("z", Some("doc"), "alpha beta", 3.0),
            IndexedDoc::build("a", Some("doc"), "alpha beta", 3.0),
        ];
        let stats = build_corpus_stats(&docs);
        let p = Bm25Params::default();
        let mut scored: Vec<ScoredNote> = docs
            .iter()
            .map(|d| ScoredNote {
                note_id: d.note_id.clone(),
                score: bm25_score(&["alpha".into()], d, &stats, &p),
                terms_matched: vec!["alpha".into()],
            })
            .collect();
        scored.sort_by(|x, y| {
            y.score
                .partial_cmp(&x.score)
                .unwrap_or(std::cmp::Ordering::Equal)
                .then_with(|| x.note_id.cmp(&y.note_id))
        });
        assert_eq!(scored[0].note_id, "a", "tie broken by id ascending");
    }
}
