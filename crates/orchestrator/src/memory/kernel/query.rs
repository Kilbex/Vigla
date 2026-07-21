//! `MemoryKernel::recent_events_for_mission` and `MemoryKernel::latest_bundle_for_mission`.

use super::super::archive::{self, ArchivedEvent};
use super::super::error::MemoryError;
use super::list_archive_files_newest_first;
use super::types::{MemoryBundleRow, MemoryEventRow};
use super::MemoryKernel;

impl MemoryKernel {
    /// Recent memory events scoped to a mission, newest-first. Tier-2E
    /// receiving surface for the read-only Memory drawer. Returns
    /// untyped rows — the host crate projects them into a UI DTO
    /// without needing SQL itself.
    ///
    /// A5 (Tier-2G): archive-aware. SQL holds the hot path; if SQL
    /// returns fewer than `limit` rows we fall through to the
    /// monthly archive files (`<events-archive>/YYYY-MM.jsonl.zst`)
    /// to fill out the remainder. Archive reads are filtered by
    /// `mission_id` after decompression — fine at our scale (a few
    /// MB per month) and far more code-direct than indexing every
    /// archive file.
    ///
    /// `limit` is clamped to `[1, 500]` by callers; the kernel does
    /// not enforce additional caps.
    pub async fn recent_events_for_mission(
        &self,
        mission_id: &str,
        limit: usize,
    ) -> Result<Vec<MemoryEventRow>, MemoryError> {
        type Row = (
            String,
            Option<String>,
            Option<String>,
            String,
            String,
            String,
        );
        let rows: Vec<Row> = sqlx::query_as(
            "SELECT event_id, mission_id, worker_id, ts, type, payload_json \
             FROM memory_events \
             WHERE mission_id = ? \
             ORDER BY ts DESC \
             LIMIT ?",
        )
        .bind(mission_id)
        .bind(limit as i64)
        .fetch_all(&self.pool)
        .await?;

        let mut out: Vec<MemoryEventRow> = rows
            .into_iter()
            .map(
                |(event_id, mission_id, worker_id, ts, event_type, payload_json)| MemoryEventRow {
                    event_id,
                    mission_id,
                    worker_id,
                    ts,
                    event_type,
                    payload_json,
                },
            )
            .collect();

        // Hot path: SQL already satisfies the limit. Skip the
        // archive scan entirely (the common case once the events
        // sweep has not yet evicted anything OR the mission is
        // recent enough to live entirely in SQL).
        if out.len() >= limit {
            return Ok(out);
        }

        // Cold path: scan monthly archive files, newest-first,
        // filtering by mission_id. Stop when we have enough.
        let archive_dir = self.vigla_root.join("events-archive");
        let archive_files = list_archive_files_newest_first(&archive_dir).await;
        let need = limit.saturating_sub(out.len());
        let mut from_archive: Vec<MemoryEventRow> = Vec::with_capacity(need);

        // Tracking the oldest hot ts so we never re-add an event
        // that's already in `out` (the sweep DELETEs after a
        // successful merge, so this is defensive — the
        // `merge_events_archive` dedupe also catches overlap).
        let oldest_hot_ts: Option<&str> = out.last().map(|r| r.ts.as_str());

        for path in archive_files {
            if from_archive.len() >= need {
                break;
            }
            let rows: Vec<ArchivedEvent> = match archive::read_jsonl_zst(&path).await {
                Ok(r) => r,
                Err(e) => {
                    // Skip a corrupted archive but keep going —
                    // partial answers beat a hard error here.
                    tracing::error!(
                        "vigla: unable to read events archive {}: {e}",
                        path.display()
                    );
                    continue;
                }
            };
            // Walk newest-first within the file too. Archive files
            // are sorted ts-ASC on disk; reverse for our descending
            // contract.
            for ev in rows.into_iter().rev() {
                if ev.mission_id.as_deref() != Some(mission_id) {
                    continue;
                }
                if let Some(hot_ts) = oldest_hot_ts {
                    if ev.ts.as_str() >= hot_ts {
                        // Already covered by the hot SQL window.
                        continue;
                    }
                }
                from_archive.push(MemoryEventRow {
                    event_id: ev.event_id,
                    mission_id: ev.mission_id,
                    worker_id: ev.worker_id,
                    ts: ev.ts,
                    event_type: ev.event_type,
                    payload_json: ev.payload_json,
                });
                if from_archive.len() >= need {
                    break;
                }
            }
        }

        out.extend(from_archive);
        // Sort newest-first across hot + archive merge. Stable sort
        // preserves SQL's original tie-breaking within hot rows.
        out.sort_by(|a, b| b.ts.cmp(&a.ts));
        out.truncate(limit);
        Ok(out)
    }

    /// The most recently composed bundle for a mission, or `None`.
    /// Tier-2E "Attached memory" section sources its content here.
    pub async fn latest_bundle_for_mission(
        &self,
        mission_id: &str,
    ) -> Result<Option<MemoryBundleRow>, MemoryError> {
        type Row = (String, String, String, i64, String, String);
        let row: Option<Row> = sqlx::query_as(
            "SELECT bundle_id, mission_id, worker_id, turn, vendor, page_table_json \
             FROM memory_bundles \
             WHERE mission_id = ? \
             ORDER BY turn DESC, bundle_id DESC \
             LIMIT 1",
        )
        .bind(mission_id)
        .fetch_optional(&self.pool)
        .await?;
        Ok(row.map(
            |(bundle_id, mission_id, worker_id, turn, vendor, page_table_json)| MemoryBundleRow {
                bundle_id,
                mission_id,
                worker_id,
                turn: turn.max(0) as u32,
                vendor,
                page_table_json,
            },
        ))
    }
}
