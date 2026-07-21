//! Mission-barrier handler + pending-row archival.

use event_schema::memory::BarrierKind;

use super::super::archive::{self, ArchivedPending};
use super::super::error::MemoryError;
use super::super::reflection;
use super::MemoryKernel;
use super::MISSIONS_DIR;

impl MemoryKernel {
    /// Mission terminal transition. Invokes the reflection pass that
    /// records the appropriate witnesses and promotes/demotes
    /// touched notes.
    pub async fn on_mission_barrier(
        &self,
        mission_id: &str,
        kind: BarrierKind,
    ) -> Result<reflection::ReflectionOutcome, MemoryError> {
        let outcome = match kind {
            BarrierKind::Accept => reflection::on_accept(&self.pool, &self.store, mission_id).await,
            BarrierKind::Scrub => reflection::on_scrub(&self.pool, &self.store, mission_id).await,
            BarrierKind::Explicit => {
                // Treat as accept for P2 — idle reflection in P5 may
                // distinguish further.
                reflection::on_accept(&self.pool, &self.store, mission_id).await
            }
        }?;

        // A5: archive the now-terminal mission's pending rows. Run
        // after reflection so any newly-ratified proposals are in
        // their final state before we move them off the SQL table.
        // Fail-soft — the user-visible mission outcome is already
        // applied; archive failures don't roll it back.
        if let Err(e) = self.archive_mission_pending(mission_id).await {
            tracing::error!("vigla: memory_pending archive failed for {mission_id}: {e}");
        }
        Ok(outcome)
    }

    /// A5: archive `memory_pending` rows for a closed mission. Writes
    /// `<missions>/<mid>/pending.jsonl.zst` and DELETEs the matching
    /// SQL rows. Idempotent — a re-run with no pending rows is a
    /// no-op and overwrites the archive with an empty frame (still
    /// valid JSONL.zst).
    ///
    /// Returns the count of rows archived. `0` is expected when
    /// every proposal was already archived in a previous barrier
    /// for the same mission.
    pub async fn archive_mission_pending(&self, mission_id: &str) -> Result<usize, MemoryError> {
        type Row = (
            String,
            String,
            String,
            String,
            String,
            Option<String>,
            String,
            String,
            String,
            String,
            String,
            String,
        );
        let rows: Vec<Row> = sqlx::query_as(
            "SELECT proposal_id, mission_id, worker_id, kind, scope_kind, scope_value, \
                    body, derived_from, evidence, state, created_event_id, created_at \
             FROM memory_pending WHERE mission_id = ?",
        )
        .bind(mission_id)
        .fetch_all(&self.pool)
        .await?;

        if rows.is_empty() {
            return Ok(0);
        }

        let archived: Vec<ArchivedPending> = rows
            .iter()
            .map(|r| ArchivedPending {
                proposal_id: r.0.clone(),
                mission_id: r.1.clone(),
                worker_id: r.2.clone(),
                kind: r.3.clone(),
                scope_kind: r.4.clone(),
                scope_value: r.5.clone(),
                body: r.6.clone(),
                derived_from: r.7.clone(),
                evidence: r.8.clone(),
                state: r.9.clone(),
                created_event_id: r.10.clone(),
                created_at: r.11.clone(),
            })
            .collect();

        let path = self
            .vigla_root
            .join(MISSIONS_DIR)
            .join(mission_id)
            .join("pending.jsonl.zst");
        archive::write_jsonl_zst(&path, archived).await?;

        // Delete the now-archived rows. Chunked to stay under
        // SQLite's parameter limit even for missions that
        // accumulated thousands of proposals.
        let proposal_ids: Vec<String> = rows.iter().map(|r| r.0.clone()).collect();
        let mut tx = self.pool.begin().await?;
        for chunk in proposal_ids.chunks(500) {
            let placeholders = std::iter::repeat_n("?", chunk.len())
                .collect::<Vec<_>>()
                .join(",");
            let sql = format!("DELETE FROM memory_pending WHERE proposal_id IN ({placeholders})");
            let mut q = sqlx::query(&sql);
            for id in chunk {
                q = q.bind(id);
            }
            q.execute(&mut *tx).await?;
        }
        tx.commit().await?;

        Ok(rows.len())
    }
}
