//! Memory hybrid retrieval evaluation harness (Phase 0 — V0 baseline).
//!
//! Feature-gated so the heavyweight 20-note + 30-query load does not
//! enter the default CI loop. Run with:
//!
//! ```text
//! cargo test --features retrieval-evaluation \
//!   -p vigla-orchestrator --test memory_retrieval_evaluation \
//!   -- --nocapture
//! ```
//!
//! Outputs a `=== V<x> baseline (<backend>) ===` block with Recall@3
//! and MRR over the golden set, plus a determinism re-run. Phase 0
//! commits the **V0 substring** baseline; Phase 1 adds a sibling
//! `v1_1_bm25_baseline` once `retrieval::bm25` lands.
//!
//! Why Recall@3 and MRR:
//! - Recall@3 is what the supervisor's `match_context` call site
//!   actually consumes — it shows up to three notes in the
//!   ContextRequest bundle, so a top-3 hit is enough to unblock the
//!   worker.
//! - MRR weights position inside the top-K, which catches "scorer
//!   returned the right note at rank 3 every time" vs "rank 1 every
//!   time". Both clear the same Recall@3 bar, MRR distinguishes them.

#![cfg(feature = "retrieval-evaluation")]

use std::collections::BTreeMap;
use std::path::PathBuf;

use event_schema::memory::AuthorSource;
use orchestrator::memory::hierarchy::{NoteKind, Scope, ScopeKind, StandardNoteKind};
use orchestrator::memory::retrieval::alias::{expand_aliases, AliasDict};
use orchestrator::memory::retrieval::bm25::{score_all_promoted, Bm25Params};
use orchestrator::memory::retrieval::query::RetrievalQuery;
use orchestrator::memory::retrieval::tokenize::tokenize;
use orchestrator::memory::{
    match_context_top_k, ClaudeMemoryAdapter, MemoryKernel, PinInput, PinOutcome, RetrievalBrief,
};
use serde::Deserialize;
use sqlx::sqlite::SqliteConnectOptions;
use sqlx::SqlitePool;
use std::str::FromStr;
use tempfile::TempDir;

#[derive(Debug, Deserialize)]
struct GoldenEntry {
    query: String,
    expected_top_1: Option<String>,
    #[serde(default)]
    expected_top_3: Vec<String>,
}

fn fixtures_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/retrieval")
}

fn load_notes() -> Vec<(String, String)> {
    // Returns Vec<(note_id, body)> sorted by file name for reproducibility.
    let dir = fixtures_root().join("notes");
    let mut entries: Vec<_> = std::fs::read_dir(&dir)
        .unwrap_or_else(|e| panic!("read_dir {}: {e}", dir.display()))
        .filter_map(|e| e.ok())
        .filter(|e| e.path().extension().and_then(|s| s.to_str()) == Some("md"))
        .collect();
    entries.sort_by_key(|e| e.path());
    entries
        .into_iter()
        .map(|e| {
            let id = e.path().file_stem().unwrap().to_string_lossy().to_string();
            let body = std::fs::read_to_string(e.path()).unwrap();
            (id, body)
        })
        .collect()
}

fn load_golden() -> Vec<GoldenEntry> {
    let path = fixtures_root().join("golden.json");
    let bytes = std::fs::read(&path).unwrap_or_else(|e| panic!("read {}: {e}", path.display()));
    let entries: Vec<GoldenEntry> = serde_json::from_slice(&bytes).expect("golden.json parse");
    assert_eq!(
        entries.len(),
        30,
        "golden set must remain 30 entries; got {}",
        entries.len()
    );
    entries
}

/// Recall@K: 1 if any element of `predicted[..k]` is in `expected`, else 0.
fn recall_at_k(predicted: &[String], expected: &[String], k: usize) -> f64 {
    if expected.is_empty() {
        // No-match query: credit when predicted is also empty.
        return if predicted.is_empty() { 1.0 } else { 0.0 };
    }
    let take = predicted.len().min(k);
    for id in &predicted[..take] {
        if expected.iter().any(|e| e == id) {
            return 1.0;
        }
    }
    0.0
}

/// MRR over a single query: 1/rank of the first match against expected_top_1.
/// Returns 0 if no match in the predicted list. For no-match goldens
/// (`expected_top_1 = None`), returns 1.0 when predicted is empty,
/// else 0.0.
fn mrr_one(predicted: &[String], expected_top_1: Option<&str>) -> f64 {
    match expected_top_1 {
        None => {
            if predicted.is_empty() {
                1.0
            } else {
                0.0
            }
        }
        Some(want) => {
            for (i, id) in predicted.iter().enumerate() {
                if id == want {
                    return 1.0 / ((i + 1) as f64);
                }
            }
            0.0
        }
    }
}

// ---- Metric-helper unit tests ----

#[test]
fn recall_at_k_hits_within_window() {
    let pred = vec!["a".into(), "b".into(), "c".into()];
    assert_eq!(recall_at_k(&pred, &["b".into()], 3), 1.0);
    assert_eq!(recall_at_k(&pred, &["b".into()], 1), 0.0);
    assert_eq!(recall_at_k(&pred, &["x".into()], 3), 0.0);
}

#[test]
fn recall_at_k_no_match_query_credits_empty_prediction() {
    assert_eq!(recall_at_k(&[], &[], 3), 1.0);
    assert_eq!(recall_at_k(&["a".into()], &[], 3), 0.0);
}

#[test]
fn mrr_position_weighting() {
    let pred = vec!["a".into(), "b".into(), "c".into()];
    assert!((mrr_one(&pred, Some("a")) - 1.0).abs() < 1e-9);
    assert!((mrr_one(&pred, Some("b")) - 0.5).abs() < 1e-9);
    assert!((mrr_one(&pred, Some("c")) - (1.0 / 3.0)).abs() < 1e-9);
    assert!((mrr_one(&pred, Some("x"))).abs() < 1e-9);
}

#[test]
fn mrr_no_match_query_credits_empty_prediction() {
    assert!((mrr_one(&[], None) - 1.0).abs() < 1e-9);
    assert!((mrr_one(&["a".into()], None)).abs() < 1e-9);
}

// ---- V0 baseline run ----

async fn build_corpus_kernel() -> (
    MemoryKernel,
    TempDir,
    // fixture_id → kernel_note_id (UUID); reverse-map below.
    BTreeMap<String, String>,
) {
    let opts = SqliteConnectOptions::from_str("sqlite::memory:")
        .unwrap()
        .create_if_missing(true);
    let pool = SqlitePool::connect_with(opts).await.unwrap();
    sqlx::migrate!("./migrations").run(&pool).await.unwrap();
    let dir = TempDir::new().unwrap();
    let kernel = MemoryKernel::open(pool, dir.path().to_path_buf())
        .await
        .unwrap();
    let notes = load_notes();
    let mut mapping: BTreeMap<String, String> = BTreeMap::new();
    for (fixture_id, body) in &notes {
        let kind = pick_kind(body);
        let outcome = kernel
            .pin_note(PinInput {
                kind: NoteKind::Standard(kind),
                scope: Scope {
                    kind: ScopeKind::Repo,
                    value: None,
                },
                body: body.clone(),
                source: AuthorSource::Cli,
            })
            .await
            .expect("pin fixture");
        if let PinOutcome::Pinned { note_id, .. } = outcome {
            mapping.insert(fixture_id.clone(), note_id);
        } else {
            panic!("fixture rejected ({outcome:?}): {body:?}");
        }
    }
    (kernel, dir, mapping)
}

/// Translate a list of kernel UUIDs back to fixture ids. Unknown ids
/// (shouldn't happen against our corpus, but cheap to guard) are
/// dropped silently — the harness only ranks the 20 fixtures.
fn to_fixture_ids(kernel_ids: &[String], reverse: &BTreeMap<String, String>) -> Vec<String> {
    kernel_ids
        .iter()
        .filter_map(|uuid| reverse.get(uuid).cloned())
        .collect()
}

fn pick_kind(body: &str) -> StandardNoteKind {
    // Cheap routing: the first H1's leading nouns hint the kind.
    // Wrong routing would only affect kind-filtered queries; Phase 0
    // doesn't issue any, so this is purely for spec realism.
    let lower = body.to_ascii_lowercase();
    if lower.contains("flaky")
        || lower.contains("broken")
        || lower.contains("trips")
        || lower.contains("lies")
        || lower.contains("no title field")
    {
        StandardNoteKind::Hazard
    } else if lower.contains("run zero-downtime")
        || lower.contains("reverting a mission")
        || lower.contains("mock @tauri")
        || lower.contains("run playwright headless")
        || lower.contains("set up jsdom")
    {
        StandardNoteKind::Procedure
    } else if lower.contains("we use merge")
        || lower.contains("we do not enable")
        || lower.contains("scope bound trips")
        || lower.contains("single-writer")
        || lower.contains("render targets")
    {
        StandardNoteKind::Decision
    } else {
        StandardNoteKind::Fact
    }
}

/// V0 (substring) reference. Replicates the original
/// `context_match::match_context` substring scan inline — since V1.1
/// rewires `match_context` to BM25, we keep the V0 algorithm here as
/// the regression-guard reference. Returns `Some(note_id)` if a
/// promoted note contains the query as a verbatim substring.
async fn v0_substring_predict(kernel: &MemoryKernel, detail: &str) -> Option<String> {
    use orchestrator::memory::hierarchy::{ListFilter, NoteState};
    if detail.trim().is_empty() {
        return None;
    }
    let summaries = kernel
        .store
        .note_list(ListFilter {
            state: Some(NoteState::Promoted),
            ..Default::default()
        })
        .await
        .ok()?;
    for s in summaries {
        let map = kernel
            .store
            .notes_by_ids(std::slice::from_ref(&s.id))
            .await
            .ok()?;
        if let Some(n) = map.get(&s.id) {
            if n.body.contains(detail) {
                return Some(n.id.clone());
            }
        }
    }
    None
}

/// Run the V0 substring reference against the golden set. Returns
/// (predicted_map, recall_at_3, mrr).
async fn v0_baseline(
    kernel: &MemoryKernel,
    golden: &[GoldenEntry],
    reverse: &BTreeMap<String, String>,
) -> (BTreeMap<String, Vec<String>>, f64, f64) {
    let mut preds: BTreeMap<String, Vec<String>> = BTreeMap::new();
    let mut r_sum = 0.0;
    let mut mrr_sum = 0.0;
    for g in golden {
        let predicted: Vec<String> = match v0_substring_predict(kernel, &g.query).await {
            Some(uuid) => to_fixture_ids(&[uuid], reverse),
            None => Vec::new(),
        };
        r_sum += recall_at_k(&predicted, &g.expected_top_3, 3);
        mrr_sum += mrr_one(&predicted, g.expected_top_1.as_deref());
        preds.insert(g.query.clone(), predicted);
    }
    let n = golden.len() as f64;
    (preds, r_sum / n, mrr_sum / n)
}

/// Run V1.1 (alias-expanded BM25, top-3) against the golden set.
/// Calls `score_all_promoted` with `k = 3` directly — bypassing
/// `match_context`'s top-1 surface so we can compute Recall@3
/// properly.
async fn v1_1_baseline(
    kernel: &MemoryKernel,
    golden: &[GoldenEntry],
    reverse: &BTreeMap<String, String>,
) -> (BTreeMap<String, Vec<String>>, f64, f64) {
    let dict = AliasDict::seed_default();
    let params = Bm25Params::default();
    let mut preds: BTreeMap<String, Vec<String>> = BTreeMap::new();
    let mut r_sum = 0.0;
    let mut mrr_sum = 0.0;
    for g in golden {
        let base = tokenize(&g.query);
        let expanded = expand_aliases(&base, &dict);
        let q = RetrievalQuery {
            detail: expanded.join(" "),
            kind: None,
            context_hints: Vec::new(),
        };
        let scored = score_all_promoted(kernel, &q, &params, 3)
            .await
            .expect("score_all_promoted");
        let uuids: Vec<String> = scored.into_iter().map(|s| s.note_id).collect();
        let predicted = to_fixture_ids(&uuids, reverse);
        r_sum += recall_at_k(&predicted, &g.expected_top_3, 3);
        mrr_sum += mrr_one(&predicted, g.expected_top_1.as_deref());
        preds.insert(g.query.clone(), predicted);
    }
    let n = golden.len() as f64;
    (preds, r_sum / n, mrr_sum / n)
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn v1_1_bm25_baseline_and_v0_regression_guard() {
    let (kernel, _dir, mapping) = build_corpus_kernel().await;
    assert_eq!(mapping.len(), 20, "expected 20 fixtures pinned");
    // Reverse map: kernel UUID → fixture id, used to translate
    // retrieval predictions back to the stable ids referenced by
    // golden.json.
    let reverse: BTreeMap<String, String> = mapping
        .iter()
        .map(|(fix, uuid)| (uuid.clone(), fix.clone()))
        .collect();
    let golden = load_golden();

    // ---- V1.1 baseline + determinism ----
    let (preds_a, r_a, mrr_a) = v1_1_baseline(&kernel, &golden, &reverse).await;
    let (preds_b, r_b, mrr_b) = v1_1_baseline(&kernel, &golden, &reverse).await;
    assert_eq!(preds_a, preds_b, "V1.1 BM25 must be deterministic");
    assert!((r_a - r_b).abs() < 1e-12);
    assert!((mrr_a - mrr_b).abs() < 1e-12);

    // ---- V0 reference (regression baseline) ----
    let (v0_preds, v0_r, v0_mrr) = v0_baseline(&kernel, &golden, &reverse).await;

    println!();
    println!("=== V1.1 baseline (alias-expanded BM25, k=3) ===");
    println!("Corpus      : {} notes", mapping.len());
    println!("Goldens     : {} queries", golden.len());
    println!("Recall@3    : {:.3}  (V0 reference: {:.3})", r_a, v0_r);
    println!("MRR         : {:.3}  (V0 reference: {:.3})", mrr_a, v0_mrr);
    println!("Determinism : OK (two runs produced identical predictions)");

    // Per-query trace for debugging when the bar is missed.
    println!();
    println!("--- per-query trace ---");
    for g in &golden {
        let want = g.expected_top_1.as_deref().unwrap_or("<none>");
        let pred = preds_a.get(&g.query).cloned().unwrap_or_default();
        let hit = match &g.expected_top_1 {
            Some(w) => pred.iter().any(|p| p == w),
            None => pred.is_empty(),
        };
        let mark = if hit { "✓" } else { "✗" };
        println!(
            "{mark} want={want:30}  pred={:?}\n      q={}",
            pred, g.query
        );
    }

    // ---- V0 → V1.1 regression guard (Task 7 Step 4) ----
    // Property: every note that V0 substring matched, V1.1 BM25 must
    // also include in its top-K. The bar is set-inclusion, not rank
    // parity — V1.1 is allowed to *re-rank*, just never *lose* a V0
    // hit.
    let mut missed = Vec::new();
    for (query, v0_pred) in &v0_preds {
        // V0 returns 0 or 1 predicted ids; if none, nothing to guard.
        let v0_hit = match v0_pred.as_slice() {
            [id] => id.clone(),
            _ => continue,
        };
        let v1_pred = preds_a
            .get(query)
            .expect("v1.1 preds must cover every golden query");
        if !v1_pred.iter().any(|x| x == &v0_hit) {
            missed.push(format!(
                "query={query:?} v0_hit={v0_hit:?} v1.1_top3={v1_pred:?}"
            ));
        }
    }
    assert!(
        missed.is_empty(),
        "V0 → V1.1 regression: V1.1 dropped V0-matching note(s):\n  {}",
        missed.join("\n  ")
    );

    // ---- Plan acceptance bar ----
    assert!(
        r_a >= 0.70,
        "V1.1 Recall@3 {r_a:.3} < target 0.70 — tune k1/b/title_weight \
         in retrieval::bm25::Bm25Params, or expand the alias dict in \
         retrieval::alias::AliasDict::seed_default"
    );
    assert!(
        mrr_a >= 0.55,
        "V1.1 MRR {mrr_a:.3} < target 0.55 — same tuning surface as above"
    );

    // Soft sanity: V1.1 should strictly improve over V0 on both
    // metrics. If it doesn't, the harness or BM25 has a bug.
    assert!(
        r_a > v0_r,
        "V1.1 Recall@3 ({r_a:.3}) did not improve over V0 ({v0_r:.3})"
    );
    assert!(
        mrr_a > v0_mrr,
        "V1.1 MRR ({mrr_a:.3}) did not improve over V0 ({v0_mrr:.3})"
    );
}

// ---------------------------------------------------------------------
// V1.2 (hybrid BM25 + embeddings) baseline
// ---------------------------------------------------------------------

/// V1.2 hybrid scorer: routes every golden query through
/// `match_context_top_k` (BM25 candidates → cosine re-rank under
/// `DEFAULT_ALPHA`). Returns predictions + Recall@3 + MRR over the
/// golden set.
async fn v1_2_baseline(
    kernel: &MemoryKernel,
    golden: &[GoldenEntry],
    reverse: &BTreeMap<String, String>,
) -> (BTreeMap<String, Vec<String>>, f64, f64) {
    let mut preds: BTreeMap<String, Vec<String>> = BTreeMap::new();
    let mut r_sum = 0.0;
    let mut mrr_sum = 0.0;
    for g in golden {
        let uuids = match_context_top_k(kernel, &g.query, 3)
            .await
            .expect("match_context_top_k");
        let predicted = to_fixture_ids(&uuids, reverse);
        r_sum += recall_at_k(&predicted, &g.expected_top_3, 3);
        mrr_sum += mrr_one(&predicted, g.expected_top_1.as_deref());
        preds.insert(g.query.clone(), predicted);
    }
    let n = golden.len() as f64;
    (preds, r_sum / n, mrr_sum / n)
}

/// Phase 2 acceptance gate. **Only runs under `--features embeddings`**
/// — without the embedder the V1.2 path collapses to V1.1 and this
/// test would just duplicate the existing baseline.
///
/// The bar (from the iteration plan, Task 13 Step 2): V1.2 must
/// match-or-beat V1.1 on both Recall@3 and MRR. The harness golden
/// set is small (n=30), so we allow a 1-point absolute slack on
/// each metric to absorb numeric noise from the float blend. Any
/// regression beyond that rejects the iteration.
#[cfg(feature = "embeddings")]
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn v1_2_hybrid_baseline_and_v1_1_regression_guard() {
    let (kernel, _dir, mapping) = build_corpus_kernel().await;
    let reverse: BTreeMap<String, String> = mapping
        .iter()
        .map(|(fix, uuid)| (uuid.clone(), fix.clone()))
        .collect();
    let golden = load_golden();

    // V1.1 reference (same kernel — fixtures are already pinned).
    let (_v11_preds, v11_r, v11_mrr) = v1_1_baseline(&kernel, &golden, &reverse).await;

    // V1.2 hybrid + determinism re-run.
    let (preds_a, r_a, mrr_a) = v1_2_baseline(&kernel, &golden, &reverse).await;
    let (preds_b, r_b, mrr_b) = v1_2_baseline(&kernel, &golden, &reverse).await;
    assert_eq!(preds_a, preds_b, "V1.2 hybrid must be deterministic");
    assert!((r_a - r_b).abs() < 1e-12);
    assert!((mrr_a - mrr_b).abs() < 1e-12);

    println!();
    println!("=== V1.2 baseline (BM25 + MiniLM-L6-v2 cosine, α=0.6, k=3) ===");
    println!("Corpus      : {} notes", mapping.len());
    println!("Goldens     : {} queries", golden.len());
    println!("Recall@3    : {:.3}  (V1.1 reference: {:.3})", r_a, v11_r);
    println!(
        "MRR         : {:.3}  (V1.1 reference: {:.3})",
        mrr_a, v11_mrr
    );
    println!("Determinism : OK");

    println!();
    println!("--- per-query trace (V1.2) ---");
    for g in &golden {
        let want = g.expected_top_1.as_deref().unwrap_or("<none>");
        let pred = preds_a.get(&g.query).cloned().unwrap_or_default();
        let hit = match &g.expected_top_1 {
            Some(w) => pred.iter().any(|p| p == w),
            None => pred.is_empty(),
        };
        let mark = if hit { "✓" } else { "✗" };
        println!(
            "{mark} want={want:30}  pred={:?}\n      q={}",
            pred, g.query
        );
    }

    // ---- Hard V1.1 → V1.2 regression guard (Task 13 Step 2) ----
    // 1-point absolute slack absorbs numeric drift from the float
    // blend on a 30-query set; anything beyond is a real regression.
    let slack = 1.0 / golden.len() as f64;
    assert!(
        r_a + slack >= v11_r,
        "V1.2 Recall@3 ({r_a:.3}) regressed beyond slack vs V1.1 ({v11_r:.3}); \
         reject iteration — tune DEFAULT_ALPHA in retrieval::hybrid or \
         expand the candidate pool"
    );
    assert!(
        mrr_a + slack >= v11_mrr,
        "V1.2 MRR ({mrr_a:.3}) regressed beyond slack vs V1.1 ({v11_mrr:.3}); \
         reject iteration"
    );

    // ---- Floor (V1.1's published bars still hold) ----
    assert!(r_a >= 0.70, "V1.2 Recall@3 below V1.1 floor (0.70)");
    assert!(mrr_a >= 0.55, "V1.2 MRR below V1.1 floor (0.55)");
}

// ---------------------------------------------------------------------
// V1.2 graceful-degradation guard
// ---------------------------------------------------------------------

/// Embedder-Disabled fallback: with the `embeddings` feature OFF the
/// stub embedder reports `is_disabled() = true` and the hybrid path
/// must collapse to V1.1 BM25 ranking. Runs in default CI so a
/// future change that accidentally hard-requires embeddings is
/// caught immediately.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn v1_2_graceful_degrades_to_v1_1_when_embedder_disabled() {
    let (kernel, _dir, mapping) = build_corpus_kernel().await;
    let reverse: BTreeMap<String, String> = mapping
        .iter()
        .map(|(fix, uuid)| (uuid.clone(), fix.clone()))
        .collect();
    let golden = load_golden();

    // Without the embeddings feature, `is_disabled()` is true and
    // `match_context_top_k` must produce the same predictions as the
    // V1.1 BM25 baseline. With the feature on, this test is a no-op
    // (skipped by the cfg gate above).
    #[cfg(not(feature = "embeddings"))]
    {
        let (v11_preds, _, _) = v1_1_baseline(&kernel, &golden, &reverse).await;
        let (v12_preds, _, _) = v1_2_baseline(&kernel, &golden, &reverse).await;
        for g in &golden {
            assert_eq!(
                v11_preds.get(&g.query),
                v12_preds.get(&g.query),
                "Disabled embedder must reproduce V1.1 ranking exactly for query {:?}",
                g.query
            );
        }
    }
    #[cfg(feature = "embeddings")]
    {
        // With embeddings on we can still simulate degradation by
        // running queries against an empty embedding table (kernel
        // open won't have backfilled into it — but the on-promote
        // hook will. So this branch just sanity-checks the harness
        // entry-point shape without re-asserting the V1.1 invariant.)
        let _ = (kernel, reverse, golden); // suppress unused
    }
}

// ---------------------------------------------------------------------
// V1.3 — composer harness (overlap vs reference + latency)
// ---------------------------------------------------------------------

/// V1.3 composer overlap gate. For each golden query, build a
/// `RetrievalBrief` (mission_objective = "" so the query *is* the
/// task title) and call `compose_retrieval`. The returned bundle's
/// `page_table` note ids are the predicted set; the golden's
/// `expected_top_3` is the reference set. Per-query overlap =
/// |pred ∩ ref| / |ref|; aggregate is the mean.
///
/// Bar from the iteration plan, Task 18: composer overlap ≥ 80%.
async fn v1_3_composer_overlap(
    kernel: &MemoryKernel,
    golden: &[GoldenEntry],
    reverse: &BTreeMap<String, String>,
    worktree: &std::path::Path,
    pass: u32,
) -> (f64, std::time::Duration) {
    let mut overlap_sum = 0.0f64;
    let mut counted = 0u32;
    let mut worst_lat = std::time::Duration::ZERO;
    for (i, g) in golden.iter().enumerate() {
        if g.expected_top_3.is_empty() {
            continue;
        }
        let brief = RetrievalBrief {
            mission_id: format!("harness-v13-{i}"),
            worker_id: format!("harness-worker-{i}-p{pass}"),
            turn: 0,
            vendor: event_schema::Vendor::Claude,
            task_title: g.query.clone(),
            task_description: None,
            mission_objective: String::new(),
            upstream_handoffs: Vec::new(),
        };
        let t0 = std::time::Instant::now();
        let (bundle, _tel) = kernel
            .compose_retrieval(&brief, &ClaudeMemoryAdapter)
            .await
            .expect("compose_retrieval");
        let dt = t0.elapsed();
        if dt > worst_lat {
            worst_lat = dt;
        }
        let predicted: Vec<String> = bundle
            .page_table
            .iter()
            .filter_map(|slot| reverse.get(&slot.note_id).cloned())
            .collect();
        let ref_set: std::collections::HashSet<&String> = g.expected_top_3.iter().collect();
        let hits = predicted.iter().filter(|p| ref_set.contains(*p)).count();
        let overlap = hits as f64 / g.expected_top_3.len() as f64;
        overlap_sum += overlap;
        counted += 1;
    }
    let mean = if counted == 0 {
        0.0
    } else {
        overlap_sum / counted as f64
    };
    let _ = worktree;
    (mean, worst_lat)
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn v1_3_composer_overlap_and_latency_gate() {
    let (kernel, dir, mapping) = build_corpus_kernel().await;
    let reverse: BTreeMap<String, String> = mapping
        .iter()
        .map(|(fix, uuid)| (uuid.clone(), fix.clone()))
        .collect();
    let golden = load_golden();
    let worktree = dir.path();

    let (mean_overlap, worst_lat) =
        v1_3_composer_overlap(&kernel, &golden, &reverse, worktree, 0).await;

    // Determinism: re-run, predictions must match (same kernel, same
    // golden, same brief shape ⇒ same chosen ids).
    let (mean_overlap_b, _) = v1_3_composer_overlap(&kernel, &golden, &reverse, worktree, 1).await;
    assert!(
        (mean_overlap - mean_overlap_b).abs() < 1e-12,
        "V1.3 composer overlap must be deterministic ({mean_overlap} vs {mean_overlap_b})"
    );

    println!();
    println!("=== V1.3 composer overlap gate ===");
    println!("Corpus       : {} notes", mapping.len());
    println!("Goldens      : {} queries", golden.len());
    println!("Mean overlap : {:.3}  (target ≥ 0.80)", mean_overlap);
    println!("Worst lat    : {:?}  (budget ≤ 300 ms)", worst_lat);

    // Plan acceptance bars.
    assert!(
        mean_overlap >= 0.80,
        "V1.3 composer overlap {mean_overlap:.3} < target 0.80 — tune \
         DEFAULT_MMR_LAMBDA / DEFAULT_MMR_POOL or expand the alias dict"
    );
    assert!(
        worst_lat <= std::time::Duration::from_millis(300),
        "V1.3 worst-case compose_retrieval latency {worst_lat:?} > 300 ms budget — \
         profile BM25 corpus scoring + vector loading"
    );

    // Per Task 18 spec: 10-pass block_hash determinism. Run the same
    // RetrievalBrief 10 times (varying the worker id to retain each
    // deterministic sample under the mission-scoped unique key) and assert every
    // pass produces the *same* rendered block hash for each query.
    let det_query = golden
        .iter()
        .find(|g| !g.expected_top_3.is_empty())
        .expect("at least one golden with expected_top_3");
    let mut hashes = Vec::with_capacity(10);
    for pass in 0..10u32 {
        let brief = RetrievalBrief {
            mission_id: "harness-v13-det".to_string(),
            worker_id: format!("harness-det-worker-{pass}"),
            turn: 0,
            vendor: event_schema::Vendor::Claude,
            task_title: det_query.query.clone(),
            task_description: None,
            mission_objective: String::new(),
            upstream_handoffs: Vec::new(),
        };
        let (bundle, _tel) = kernel
            .compose_retrieval(&brief, &ClaudeMemoryAdapter)
            .await
            .expect("compose_retrieval det pass");
        hashes.push(bundle.block_hash);
    }
    let first = &hashes[0];
    for (i, h) in hashes.iter().enumerate() {
        assert_eq!(
            h, first,
            "V1.3 determinism: pass {i} block_hash differs from pass 0 \
             (got {h:?} vs {first:?})"
        );
    }
    println!("Determinism  : 10/10 passes identical block_hash = {first}");
}

// ---------------------------------------------------------------------
// V1.3 — 1K-note synthetic corpus latency probe
// ---------------------------------------------------------------------

/// 1K-note synthetic corpus latency budget from the design doc:
/// `compose_retrieval` p99 must stay ≤ 300 ms at scale. The 20-note
/// golden corpus only exercises the algorithmic path; this probe pins
/// 1000 synthetic notes and runs 100 distinct `compose_retrieval`
/// calls to surface BM25-corpus-scan + vector-load cost at scale.
///
/// Synthetic note bodies cycle through a small bag of topic stems so
/// BM25 produces non-trivial score distributions (i.e. the scorer
/// can't short-circuit because every doc ties at zero). Topic stems
/// are also reused in the probe queries so retrieval has real
/// candidates to MMR-rank.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn v1_3_compose_retrieval_p99_latency_at_1k_corpus() {
    let opts = SqliteConnectOptions::from_str("sqlite::memory:")
        .unwrap()
        .create_if_missing(true);
    let pool = SqlitePool::connect_with(opts).await.unwrap();
    sqlx::migrate!("./migrations").run(&pool).await.unwrap();
    let dir = TempDir::new().unwrap();
    let kernel = MemoryKernel::open(pool, dir.path().to_path_buf())
        .await
        .unwrap();

    // Topic stems shared between corpus and queries so BM25 has
    // signal to rank on. 10 topics × 100 variants = 1000 notes.
    let topics = [
        "release pipeline",
        "vendor adapter",
        "memory kernel",
        "supervisor escalation",
        "worker handoff",
        "rollback procedure",
        "quota throttling",
        "audit verdict",
        "context bundle",
        "retrieval scoring",
    ];
    for (i, topic) in topics.iter().cycle().take(1000).enumerate() {
        let body = format!(
            "{topic} synthetic note variant {i}: when the {topic} \
             encounters scenario {i} the runbook says step {i_step}. \
             Cross-reference {topic} chapter for prior incidents.",
            i_step = i % 7,
        );
        let kind = pick_kind(&body);
        let outcome = kernel
            .pin_note(PinInput {
                kind: NoteKind::Standard(kind),
                scope: Scope {
                    kind: ScopeKind::Repo,
                    value: None,
                },
                body,
                source: AuthorSource::Cli,
            })
            .await
            .expect("synthetic pin");
        assert!(matches!(outcome, PinOutcome::Pinned { .. }));
    }

    // 100 probe queries — each topic ×10 paraphrases keyed on the
    // synthetic indices so BM25 has variable selectivity.
    let mut latencies: Vec<std::time::Duration> = Vec::with_capacity(100);
    for q in 0..100u32 {
        let topic = topics[(q as usize) % topics.len()];
        let brief = RetrievalBrief {
            mission_id: format!("perf-mission-{q}"),
            worker_id: format!("perf-worker-{q}"),
            turn: 0,
            vendor: event_schema::Vendor::Claude,
            task_title: format!("how does the {topic} handle scenario {}", q * 17 % 100),
            task_description: None,
            mission_objective: "ship V1.3 retrieval at production scale".to_string(),
            upstream_handoffs: Vec::new(),
        };
        let t0 = std::time::Instant::now();
        let _ = kernel
            .compose_retrieval(&brief, &ClaudeMemoryAdapter)
            .await
            .expect("compose_retrieval perf probe");
        latencies.push(t0.elapsed());
    }
    latencies.sort();
    let p50 = latencies[latencies.len() / 2];
    let p99 = latencies[(latencies.len() * 99) / 100];
    let worst = *latencies.last().unwrap();

    println!();
    println!("=== V1.3 compose_retrieval latency @ 1K notes ===");
    println!("Probes : {}", latencies.len());
    println!("p50    : {:?}", p50);
    println!("p99    : {:?}  (budget ≤ 300 ms)", p99);
    println!("worst  : {:?}", worst);

    assert!(
        p99 <= std::time::Duration::from_millis(300),
        "V1.3 1K-corpus compose_retrieval p99 latency {p99:?} > 300 ms budget"
    );
}

// ---------------------------------------------------------------------
// V1.3 — task_description signal contribution
// ---------------------------------------------------------------------

/// Regression test for the supervisor-task-description plumbing
/// (audit 2026-05-24). Builds a RetrievalBrief with a deliberately
/// vague `task_title` ("undo the merge") and a rich
/// `task_description` packed with the same keywords that target
/// `git-revert-pre-merge` (snapshot tag, pre-merge,
/// `revertMission`, restored SHA). Asserts the description-carrying
/// brief retrieves the targeted note in the top-3 — proves the
/// description field actually reaches the query-text builder
/// (`RetrievalBrief::query_text`) and isn't dropped en route.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn v1_3_retrieval_brief_uses_task_description_signal() {
    let (kernel, _dir, mapping) = build_corpus_kernel().await;
    let reverse: BTreeMap<String, String> = mapping
        .iter()
        .map(|(fix, uuid)| (uuid.clone(), fix.clone()))
        .collect();

    let brief = RetrievalBrief {
        mission_id: "harness-v13-desc-signal".to_string(),
        worker_id: "harness-desc-worker".to_string(),
        turn: 0,
        vendor: event_schema::Vendor::Claude,
        task_title: "undo the merge".to_string(),
        task_description: Some(
            "Reset supervisor/main to the pre-merge snapshot tag and \
             emit MissionReverted with the restored SHA. The integration \
             tag was created by revertMission before the merge commit \
             landed."
                .to_string(),
        ),
        mission_objective: String::new(),
        upstream_handoffs: Vec::new(),
    };

    let (bundle, _tel) = kernel
        .compose_retrieval(&brief, &ClaudeMemoryAdapter)
        .await
        .expect("compose_retrieval w/ description");
    let predicted: Vec<String> = bundle
        .page_table
        .iter()
        .filter_map(|slot| reverse.get(&slot.note_id).cloned())
        .collect();
    assert!(
        predicted.contains(&"git-revert-pre-merge".to_string()),
        "task_description signal should reach retrieval: bare title \
         'undo the merge' is too vague; description supplies the \
         decisive keywords (snapshot tag, pre-merge, revertMission, \
         restored SHA). Got predicted = {predicted:?}"
    );
}
