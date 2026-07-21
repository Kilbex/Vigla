//! `MemoryKernel::sweep_old_events`.

use super::super::archive::{self, ArchivedEvent};
use super::super::error::MemoryError;
use super::MemoryKernel;

impl MemoryKernel {
    /// A5: sweep `memory_events` rows older than `retention_days`
    /// into monthly `<events-archive>/YYYY-MM.jsonl.zst` files and
    /// drop the SQL rows.
    ///
    /// Processes rows in 1000-row batches so an arbitrarily large
    /// backlog doesn't OOM. Pass [`DEFAULT_EVENTS_RETENTION_DAYS`]
    /// for the standard 90-day horizon. Returns the total number
    /// of events moved off the hot table.
    pub async fn sweep_old_events(&self, retention_days: u32) -> Result<usize, MemoryError> {
        let cutoff = archive::retention_cutoff(retention_days);
        let mut total_archived = 0usize;
        const BATCH: i64 = 1000;
        let archive_dir = self.vigla_root.join("events-archive");

        loop {
            type Row = (
                String,
                Option<String>,
                Option<String>,
                String,
                String,
                String,
                String,
            );
            let rows: Vec<Row> = sqlx::query_as(
                "SELECT event_id, mission_id, worker_id, ts, type, payload_json, \
                        schema_version \
                 FROM memory_events \
                 WHERE ts < ? \
                 ORDER BY ts ASC \
                 LIMIT ?",
            )
            .bind(&cutoff)
            .bind(BATCH)
            .fetch_all(&self.pool)
            .await?;

            if rows.is_empty() {
                break;
            }

            // Group by year-month so each archive file holds
            // logically-clustered events for cheap targeted reads.
            let mut by_month: std::collections::BTreeMap<String, Vec<ArchivedEvent>> =
                std::collections::BTreeMap::new();
            for r in &rows {
                let month = archive::month_key(&r.3);
                by_month.entry(month).or_default().push(ArchivedEvent {
                    event_id: r.0.clone(),
                    mission_id: r.1.clone(),
                    worker_id: r.2.clone(),
                    ts: r.3.clone(),
                    event_type: r.4.clone(),
                    payload_json: r.5.clone(),
                    schema_version: r.6.clone(),
                });
            }

            for (month, events) in by_month {
                let path = archive_dir.join(format!("{month}.jsonl.zst"));
                archive::merge_events_archive(&path, events).await?;
            }

            // Delete the just-archived rows.
            let event_ids: Vec<String> = rows.iter().map(|r| r.0.clone()).collect();
            let mut tx = self.pool.begin().await?;
            for chunk in event_ids.chunks(500) {
                let placeholders = std::iter::repeat_n("?", chunk.len())
                    .collect::<Vec<_>>()
                    .join(",");
                let sql = format!("DELETE FROM memory_events WHERE event_id IN ({placeholders})");
                let mut q = sqlx::query(&sql);
                for id in chunk {
                    q = q.bind(id);
                }
                q.execute(&mut *tx).await?;
            }
            tx.commit().await?;
            total_archived += rows.len();
        }

        Ok(total_archived)
    }
}
