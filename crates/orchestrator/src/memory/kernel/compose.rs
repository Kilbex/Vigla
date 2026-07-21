//! `MemoryKernel::render_for_worker` and `MemoryKernel::check_drift`.

use std::path::Path;

use event_schema::memory::{MemoryBundleRendered, MemoryDriftDetected, MEMORY_SCHEMA_VERSION};

use super::super::adapter::MemoryAdapter;
use super::super::coherence::{detect_drift, write_anchor_block, DriftStatus};
use super::super::composer::{BundleBrief, ComposedBundle, RetrievalBrief};
use super::super::error::MemoryError;
use super::super::ids;
use super::super::retrieval::alias::{expand_aliases, AliasDict};
use super::super::retrieval::bm25::{score_all_promoted, Bm25Params};
use super::super::retrieval::embed::MODEL_VERSION;
use super::super::retrieval::hybrid::{
    cosine_normalized, hybrid_score, normalize_bm25, DEFAULT_ALPHA, DEFAULT_MMR_LAMBDA,
    DEFAULT_MMR_POOL,
};
use super::super::retrieval::mmr::{mmr_rerank, MmrCandidate};
use super::super::retrieval::query::RetrievalQuery;
use super::super::retrieval::storage;
use super::super::retrieval::tokenize::tokenize;
use super::types::RenderedBundle;
use super::MemoryKernel;

/// Telemetry returned alongside the chosen note-id list from
/// [`MemoryKernel::compose_retrieval`] /
/// [`MemoryKernel::render_for_worker_with_retrieval`]. Callers use
/// this to populate `MissionEventKind::ContextBundleComposed`
/// accurately:
///
/// * `chosen_count` is the number of notes returned **post-MMR but
///   pre-budget-truncation** — i.e. the size of the ranked id list
///   the composer was handed, *before* `compose_manual` drops the
///   tail to fit the worker's token budget. This is the value the
///   variant doc on `ContextBundleComposed.candidate_count` refers
///   to ("post-MMR for the retrieval path"). Using
///   `rendered.page_table.len()` directly would under-count when
///   the budget kicks in.
/// * `mmr_applied` is `true` iff Stage 3 (MMR re-rank) actually ran.
///   MMR is bypassed when no candidate has a stored embedding —
///   typical during the embedding-backfill warm-up window, or when
///   the `embeddings` cargo feature is off entirely. Callers should
///   surface `mmr_lambda` as `Some(λ)` only when `mmr_applied` is
///   true; `None` otherwise. This lets replay tools distinguish
///   "hybrid retrieval with MMR" from "BM25-only fallback" from a
///   `Retrieval`-source event.
#[derive(Debug, Clone, Copy)]
pub struct RetrievalTelemetry {
    pub chosen_count: u32,
    pub mmr_applied: bool,
}

impl MemoryKernel {
    /// Compose a bundle, archive it, and write the anchor block into
    /// the worker's worktree native file. The composed and rendered
    /// events both land in `memory_events` so replay can re-derive
    /// the exact file the worker was about to read.
    pub async fn render_for_worker(
        &self,
        brief: &BundleBrief,
        adapter: &dyn MemoryAdapter,
        note_ids: &[String],
        worktree_root: &Path,
    ) -> Result<RenderedBundle, MemoryError> {
        let bundle: ComposedBundle = self
            .composer
            .compose_manual(brief, adapter, note_ids)
            .await?;
        let native_path = adapter.native_file_path(worktree_root);
        let write_out = write_anchor_block(
            &native_path,
            adapter.anchor_open(),
            adapter.anchor_close(),
            &bundle.rendered_block,
        )
        .await?;

        // Persist the rendered_event in memory_events + flip the
        // memory_bundles row so subsequent drift checks have the
        // ground truth.
        let event_id = ids::new_memory_event_id();
        let now = crate::ids::rfc3339_now();
        let payload = MemoryBundleRendered {
            bundle_id: bundle.bundle_id.clone(),
            native_file_path: native_path.to_string_lossy().to_string(),
            anchor_open_offset: write_out.anchor_open_offset,
            anchor_close_offset: write_out.anchor_close_offset,
            file_hash: write_out.file_hash.clone(),
        };
        let payload_json = serde_json::to_string(&payload)?;

        let mut tx = self.pool.begin().await?;
        sqlx::query("UPDATE memory_bundles SET rendered_event_id = ? WHERE bundle_id = ?")
            .bind(&event_id)
            .bind(&bundle.bundle_id)
            .execute(&mut *tx)
            .await?;
        sqlx::query(
            "INSERT INTO memory_events \
             (event_id, mission_id, worker_id, ts, type, payload_json, schema_version) \
             VALUES (?, ?, ?, ?, 'bundle_rendered', ?, ?)",
        )
        .bind(&event_id)
        .bind(&brief.mission_id)
        .bind(&brief.worker_id)
        .bind(&now)
        .bind(&payload_json)
        .bind(MEMORY_SCHEMA_VERSION)
        .execute(&mut *tx)
        .await?;
        tx.commit().await?;

        Ok(RenderedBundle {
            bundle_id: bundle.bundle_id,
            block_hash: bundle.block_hash,
            file_hash: write_out.file_hash,
            native_file_path: native_path,
            archived_path: bundle.archived_path,
            anchor_open_offset: write_out.anchor_open_offset,
            anchor_close_offset: write_out.anchor_close_offset,
            page_table: bundle.page_table,
        })
    }

    /// V1.3 retrieval-driven compose. Builds the task-context query
    /// from `brief`, runs the hybrid scorer + MMR re-rank to pick
    /// promoted notes, then delegates to `compose_manual` for the
    /// actual rendering. Returns the same [`ComposedBundle`] shape
    /// as the manual path so downstream code (drift detection,
    /// archive replay, audit) treats both paths uniformly.
    ///
    /// # Fail-soft contract
    ///
    /// Retrieval is best-effort:
    /// * Empty query → empty bundle (no notes selected).
    /// * No promoted notes / no BM25 overlap → empty bundle.
    /// * Embedder disabled OR no vectors stored → falls back to BM25
    ///   top-K (no MMR re-rank; MMR needs vectors to compute
    ///   diversity).
    /// * SQL or storage errors *do* propagate — they indicate a
    ///   broken kernel, not a missed match.
    ///
    /// # Determinism
    ///
    /// Same `brief.query_text()` + same promoted-note set + same
    /// stored embeddings ⇒ same selected ids ⇒ same `bundle_hash`.
    /// The whole pipeline is deterministic: BM25 ties on
    /// `(score DESC, note_id ASC)`, hybrid ties the same way, MMR
    /// ties on `(score DESC, relevance DESC, note_id ASC)`.
    pub async fn compose_retrieval(
        &self,
        brief: &RetrievalBrief,
        adapter: &dyn MemoryAdapter,
    ) -> Result<(ComposedBundle, RetrievalTelemetry), MemoryError> {
        let (chosen, telemetry) = self.pick_retrieval_note_ids(brief).await?;
        let composed = self
            .composer
            .compose_manual(&brief.bundle_brief(), adapter, &chosen)
            .await?;
        Ok((composed, telemetry))
    }

    /// V1.3 retrieval-driven render. Same as
    /// [`Self::render_for_worker`] but routes through
    /// [`Self::compose_retrieval`] to pick the note ids. Caller
    /// supplies the worktree root for anchor-block write-out.
    /// Returns the [`RenderedBundle`] alongside the
    /// [`RetrievalTelemetry`] describing the retrieval pipeline
    /// (post-MMR chosen count + whether MMR actually ran) so
    /// callers can emit accurate `ContextBundleComposed` events.
    pub async fn render_for_worker_with_retrieval(
        &self,
        brief: &RetrievalBrief,
        adapter: &dyn MemoryAdapter,
        worktree_root: &Path,
    ) -> Result<(RenderedBundle, RetrievalTelemetry), MemoryError> {
        let (chosen, telemetry) = self.pick_retrieval_note_ids(brief).await?;
        let rendered = self
            .render_for_worker(&brief.bundle_brief(), adapter, &chosen, worktree_root)
            .await?;
        Ok((rendered, telemetry))
    }

    /// Pick note ids for a retrieval brief: hybrid candidates → MMR
    /// re-rank. Shared by `compose_retrieval` and
    /// `render_for_worker_with_retrieval`. Returns the chosen ids
    /// **before** any composer-side budget truncation, alongside a
    /// [`RetrievalTelemetry`] record describing the pipeline so
    /// callers can emit accurate `ContextBundleComposed` events.
    async fn pick_retrieval_note_ids(
        &self,
        brief: &RetrievalBrief,
    ) -> Result<(Vec<String>, RetrievalTelemetry), MemoryError> {
        let detail = brief.query_text();
        if detail.trim().is_empty() {
            return Ok((
                Vec::new(),
                RetrievalTelemetry {
                    chosen_count: 0,
                    mmr_applied: false,
                },
            ));
        }

        // ---- Stage 1: BM25 candidate generation ----
        let base_tokens = tokenize(&detail);
        let dict = AliasDict::seed_default();
        let expanded = expand_aliases(&base_tokens, &dict);
        let q = RetrievalQuery {
            detail: expanded.join(" "),
            kind: None,
            context_hints: Vec::new(),
        };
        let params = Bm25Params::default();
        let pool_size = DEFAULT_MMR_POOL.max(8);
        let candidates = score_all_promoted(self, &q, &params, pool_size).await?;
        if candidates.is_empty() {
            return Ok((
                Vec::new(),
                RetrievalTelemetry {
                    chosen_count: 0,
                    mmr_applied: false,
                },
            ));
        }

        // ---- Stage 2: hybrid relevance blend (BM25 + cosine) ----
        //
        // Reuses the V1.2 path: cosine = 0 for any candidate whose
        // vector isn't stored yet; if no vector at all is seen,
        // skip Stage 3 (MMR) since the diversity term has nothing
        // to chew on.
        let mut relevances: Vec<f64> = Vec::with_capacity(candidates.len());
        let mut embeddings: Vec<Vec<f32>> = Vec::with_capacity(candidates.len());
        let mut any_vector_seen = false;
        let q_vec_opt = if self.embedder.is_disabled() {
            None
        } else {
            self.embedder.embed(&detail)
        };
        let bm25_norm = normalize_bm25(&candidates.iter().map(|c| c.score).collect::<Vec<_>>());
        for (i, c) in candidates.iter().enumerate() {
            let vec = match q_vec_opt.as_ref() {
                Some(_) => storage::get_embedding(&self.pool, &c.note_id, MODEL_VERSION).await?,
                None => None,
            };
            let cosine = match (q_vec_opt.as_ref(), vec.as_ref()) {
                (Some(q), Some(v)) => {
                    any_vector_seen = true;
                    cosine_normalized(q, v)
                }
                _ => 0.0,
            };
            relevances.push(hybrid_score(bm25_norm[i], cosine, DEFAULT_ALPHA));
            embeddings.push(vec.unwrap_or_default());
        }

        // ---- Stage 3: MMR re-rank (only when vectors are present) ----
        //
        // Without vectors, MMR's diversity term is identically 0 and
        // the result collapses to top-K by relevance — which we get
        // for free by sorting `relevances`. Skipping MMR avoids
        // burning a quadratic loop for no benefit.
        let chosen_ids: Vec<String> = if any_vector_seen {
            let mmr_in: Vec<MmrCandidate<'_>> = candidates
                .iter()
                .enumerate()
                .map(|(i, c)| MmrCandidate {
                    note_id: c.note_id.as_str(),
                    relevance: relevances[i],
                    embedding: embeddings[i].as_slice(),
                })
                .collect();
            mmr_rerank(&mmr_in, DEFAULT_MMR_LAMBDA, DEFAULT_MMR_POOL)
        } else {
            let mut idx: Vec<usize> = (0..candidates.len()).collect();
            idx.sort_by(|&a, &b| {
                relevances[b]
                    .partial_cmp(&relevances[a])
                    .unwrap_or(std::cmp::Ordering::Equal)
                    .then_with(|| candidates[a].note_id.cmp(&candidates[b].note_id))
            });
            idx.into_iter()
                .take(DEFAULT_MMR_POOL)
                .map(|i| candidates[i].note_id.clone())
                .collect()
        };
        let telemetry = RetrievalTelemetry {
            chosen_count: chosen_ids.len() as u32,
            mmr_applied: any_vector_seen,
        };
        Ok((chosen_ids, telemetry))
    }

    /// Erase a previously-archived bundle so a fresh
    /// [`Self::render_for_worker`] for the same `(worker_id, turn)`
    /// can succeed. S9 (composer re-render path) uses this when an
    /// initial render exceeds the worker's token budget: the
    /// over-budget archive is deleted, then a truncated re-render is
    /// archived in its place. The `ContextBudgetExceeded` event on
    /// the mission stream still records that the overflow happened —
    /// only the bundle-level archive (composed + rendered events for
    /// THIS bundle, plus the bundle row) is removed.
    ///
    /// Idempotent: deleting a non-existent bundle is a no-op.
    pub async fn delete_bundle(&self, bundle_id: &str) -> Result<(), MemoryError> {
        let row: Option<(String, Option<String>)> = sqlx::query_as(
            "SELECT composed_event_id, rendered_event_id \
             FROM memory_bundles WHERE bundle_id = ?",
        )
        .bind(bundle_id)
        .fetch_optional(&self.pool)
        .await?;

        let (composed_event_id, rendered_event_id) = match row {
            Some(r) => r,
            None => return Ok(()),
        };

        let mut tx = self.pool.begin().await?;
        sqlx::query("DELETE FROM memory_events WHERE event_id = ?")
            .bind(&composed_event_id)
            .execute(&mut *tx)
            .await?;
        if let Some(rid) = &rendered_event_id {
            sqlx::query("DELETE FROM memory_events WHERE event_id = ?")
                .bind(rid)
                .execute(&mut *tx)
                .await?;
        }
        sqlx::query("DELETE FROM memory_bundles WHERE bundle_id = ?")
            .bind(bundle_id)
            .execute(&mut *tx)
            .await?;
        tx.commit().await?;
        Ok(())
    }

    /// Check the worker's native file for drift against the recorded
    /// bundle. Emits `MemoryDriftDetected` if the body bytes inside
    /// the anchors no longer match.
    pub async fn check_drift(
        &self,
        bundle_id: &str,
        adapter: &dyn MemoryAdapter,
        worktree_root: &Path,
    ) -> Result<DriftStatus, MemoryError> {
        let row: Option<(String, String, String, Option<String>, String)> = sqlx::query_as(
            "SELECT bundle_id, mission_id, worker_id, rendered_event_id, hash \
             FROM memory_bundles WHERE bundle_id = ?",
        )
        .bind(bundle_id)
        .fetch_optional(&self.pool)
        .await?;
        let (_bid, mission_id, worker_id, _rendered_event_id, expected_block_hash) =
            row.ok_or_else(|| MemoryError::NoteNotFound(bundle_id.to_owned()))?;

        let native_path = adapter.native_file_path(worktree_root);
        let status = detect_drift(
            &native_path,
            adapter.anchor_open(),
            adapter.anchor_close(),
            &expected_block_hash,
        )
        .await?;

        if let DriftStatus::Drift {
            observed_hash,
            anchor_open_offset,
            ..
        } = &status
        {
            // Record the drift as an event so replay shows when the
            // kernel noticed and what the observed hash was.
            let event_id = ids::new_memory_event_id();
            let now = crate::ids::rfc3339_now();
            let payload = MemoryDriftDetected {
                bundle_id: bundle_id.to_owned(),
                native_file_path: native_path.to_string_lossy().to_string(),
                expected_hash: expected_block_hash.clone(),
                observed_hash: observed_hash.clone(),
                file_offset: *anchor_open_offset,
            };
            sqlx::query(
                "INSERT INTO memory_events \
                 (event_id, mission_id, worker_id, ts, type, payload_json, schema_version) \
                 VALUES (?, ?, ?, ?, 'drift_detected', ?, ?)",
            )
            .bind(&event_id)
            .bind(&mission_id)
            .bind(&worker_id)
            .bind(&now)
            .bind(serde_json::to_string(&payload)?)
            .bind(MEMORY_SCHEMA_VERSION)
            .execute(&self.pool)
            .await?;
        }

        Ok(status)
    }
}
