//! A5 (Tier-2G) — compressed JSONL archives for cooled SQL rows.
//!
//! Two archive shapes live here:
//!
//! 1. **Per-mission `memory_pending`** — fired on mission barrier
//!    (`on_mission_barrier`). Once a mission terminates the pending
//!    rows for it (`state ∈ {proposed, ratified, rejected, normalized}`)
//!    are immutable evidence. We write them to
//!    `<memory_root>/missions/<mid>/pending.jsonl.zst` and drop the
//!    SQL rows. The pending table shrinks back to "active missions
//!    only" — typically 0 or 1 row at rest.
//!
//! 2. **Monthly `memory_events`** — fired on kernel open (once per
//!    repo per session). Events older than the retention horizon
//!    (default 90 days) move into
//!    `<memory_root>/events-archive/YYYY-MM.jsonl.zst` keyed by the
//!    event's own UTC month. The `memory_events` table stays roughly
//!    proportional to recent activity, not session count.
//!
//! ## Format
//!
//! One JSON object per line (line-delimited JSON / NDJSON), the
//! whole stream zstd-compressed at level 3. zstd's frame format is
//! seekable enough for the future "tail this archive" query if we
//! need it; for now we just decompress whole files (~MB-scale).
//!
//! ## Atomicity
//!
//! `write_jsonl_zst` uses the same same-dir tmp + rename protocol the
//! codex body files use. A crash mid-write leaves only the tmp
//! around (which we clean up); the SQL DELETE only runs after the
//! archive file rename succeeds. The worst-case crash window leaves
//! both the archive AND the SQL rows present, which is correct: a
//! re-archive is idempotent (`merge_events_archive` dedupes by
//! `event_id`; `write_pending_archive` for a closed mission is
//! per-mission so re-runs find an empty SELECT and become no-ops).

use std::path::Path;

use serde::{de::DeserializeOwned, Deserialize, Serialize};
use tokio::fs;
use tokio::io::AsyncWriteExt;

use super::error::MemoryError;

/// zstd compression level. 3 is the library default — gives ~95% of
/// max compression at a fraction of the CPU cost. Archive write is
/// off the hot path; the cost is bounded.
const ZSTD_LEVEL: i32 = 3;

/// Default retention for the events sweep. Anything older than this
/// migrates to monthly archive files; anything newer stays hot.
pub const DEFAULT_EVENTS_RETENTION_DAYS: u32 = 90;

// ---------------------------------------------------------------------
// Row types — flat 1:1 with their SQL tables. Kept private to the
// memory module so the public DTOs (`MemoryEventRow`,
// `MemoryBundleRow`) don't grow archive-specific fields.
// ---------------------------------------------------------------------

#[derive(Debug, Clone, Serialize, Deserialize)]
pub(crate) struct ArchivedPending {
    pub proposal_id: String,
    pub mission_id: String,
    pub worker_id: String,
    pub kind: String,
    pub scope_kind: String,
    pub scope_value: Option<String>,
    pub body: String,
    pub derived_from: String,
    pub evidence: String,
    pub state: String,
    pub created_event_id: String,
    pub created_at: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub(crate) struct ArchivedEvent {
    pub event_id: String,
    pub mission_id: Option<String>,
    pub worker_id: Option<String>,
    pub ts: String,
    pub event_type: String,
    pub payload_json: String,
    pub schema_version: String,
}

// ---------------------------------------------------------------------
// Generic JSONL.zst codec
// ---------------------------------------------------------------------

/// Serialize an iterator of records as JSONL, zstd-compress the
/// whole stream, and write atomically. Idempotent if `rows` is empty
/// — produces an empty (but valid) zstd frame, then renames over the
/// destination.
pub(crate) async fn write_jsonl_zst<I, T>(path: &Path, rows: I) -> Result<(), MemoryError>
where
    I: IntoIterator<Item = T>,
    T: Serialize,
{
    let mut jsonl: Vec<u8> = Vec::new();
    for row in rows {
        serde_json::to_writer(&mut jsonl, &row)?;
        jsonl.push(b'\n');
    }
    let compressed = zstd::stream::encode_all(&jsonl[..], ZSTD_LEVEL)
        .map_err(|e| MemoryError::Io(std::io::Error::other(e)))?;
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).await?;
    }
    write_atomic(path, &compressed).await
}

/// Decompress and deserialize a JSONL.zst file written by
/// [`write_jsonl_zst`]. Missing files return an empty Vec — callers
/// can treat "no archive yet" the same as "archive of zero rows".
pub(crate) async fn read_jsonl_zst<T: DeserializeOwned>(
    path: &Path,
) -> Result<Vec<T>, MemoryError> {
    let compressed = match fs::read(path).await {
        Ok(b) => b,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(e) => return Err(MemoryError::Io(e)),
    };
    let jsonl = zstd::stream::decode_all(&compressed[..])
        .map_err(|e| MemoryError::Io(std::io::Error::other(e)))?;
    let mut rows = Vec::new();
    for line in jsonl.split(|b| *b == b'\n') {
        if line.is_empty() {
            continue;
        }
        rows.push(serde_json::from_slice(line)?);
    }
    Ok(rows)
}

/// Merge new event rows into an existing monthly archive file:
/// decompress, append, dedupe by `event_id`, sort by `ts`, rewrite.
/// O(N) in archive size + new rows; fine for monthly chunks bounded
/// by ~tens-of-thousands of events.
pub(crate) async fn merge_events_archive(
    path: &Path,
    new_rows: Vec<ArchivedEvent>,
) -> Result<(), MemoryError> {
    let existing: Vec<ArchivedEvent> = read_jsonl_zst(path).await?;
    let mut combined = existing;
    combined.extend(new_rows);
    // Dedupe by event_id — re-running the sweep against an archived
    // file that still has its corresponding SQL rows shouldn't
    // double-count if the DELETE failed last time. Stable per insertion order.
    let mut seen = std::collections::HashSet::with_capacity(combined.len());
    combined.retain(|r| seen.insert(r.event_id.clone()));
    combined.sort_by(|a, b| a.ts.cmp(&b.ts));
    write_jsonl_zst(path, combined).await
}

/// UTC year-month key for an RFC 3339 timestamp string, e.g.
/// `"2026-05-16T14:22:01.481Z"` → `"2026-05"`. Used to bucket events
/// into monthly archive files. Defensive: a malformed ts falls
/// through to `"undated"` so the sweep can still make progress.
pub(crate) fn month_key(ts: &str) -> String {
    if ts.len() >= 7 && ts.as_bytes()[4] == b'-' {
        ts[..7].to_owned()
    } else {
        "undated".to_owned()
    }
}

/// Pure helper: compute an RFC 3339 cutoff timestamp `retention_days`
/// in the past relative to `now_ms` (Unix epoch in milliseconds). Exists
/// so callers that already hold a "now" snapshot can avoid a second
/// `SystemTime::now()` call — eliminating the race that caused
/// `retention_cutoff_is_in_the_past` to flake.
pub(super) fn retention_cutoff_at(retention_days: u32, now_ms: u64) -> String {
    let cutoff_ms = now_ms.saturating_sub((retention_days as u64) * 86_400_000);
    crate::ids::rfc3339_from_unix_ms_pub(cutoff_ms)
}

/// Compute an RFC 3339 cutoff timestamp `retention_days` in the past
/// (UTC). Anything strictly less than this string sorts as "older
/// than retention" under the lex order RFC 3339 enforces.
pub(crate) fn retention_cutoff(retention_days: u32) -> String {
    use std::time::{SystemTime, UNIX_EPOCH};
    let now_ms = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0);
    retention_cutoff_at(retention_days, now_ms)
}

// ---------------------------------------------------------------------
// Atomic write helper — same protocol as store.rs::write_atomic.
// Duplicated here rather than re-exported so the archive module
// stays self-contained.
// ---------------------------------------------------------------------

async fn write_atomic(dest: &Path, bytes: &[u8]) -> Result<(), MemoryError> {
    let parent = dest.parent().ok_or_else(|| {
        std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            "archive path has no parent",
        )
    })?;
    fs::create_dir_all(parent).await?;
    let tmp_name = format!(
        ".{}.tmp.{}",
        dest.file_name()
            .and_then(|s| s.to_str())
            .unwrap_or("archive"),
        uuid::Uuid::now_v7().simple()
    );
    let tmp = parent.join(tmp_name);
    let mut f = fs::File::create(&tmp).await?;
    f.write_all(bytes).await?;
    f.flush().await?;
    f.sync_all().await?;
    drop(f);
    match fs::rename(&tmp, dest).await {
        Ok(()) => Ok(()),
        Err(e) => {
            let _ = fs::remove_file(&tmp).await;
            Err(MemoryError::Io(e))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    fn pending_row(id: &str) -> ArchivedPending {
        ArchivedPending {
            proposal_id: id.into(),
            mission_id: "mission-1".into(),
            worker_id: "worker-1".into(),
            kind: "fact".into(),
            scope_kind: "repo".into(),
            scope_value: None,
            body: "body".into(),
            derived_from: "[]".into(),
            evidence: "[]".into(),
            state: "ratified".into(),
            created_event_id: "ev-1".into(),
            created_at: "2026-05-16T00:00:00.000Z".into(),
        }
    }

    fn event_row(id: &str, ts: &str) -> ArchivedEvent {
        ArchivedEvent {
            event_id: id.into(),
            mission_id: Some("m-1".into()),
            worker_id: None,
            ts: ts.into(),
            event_type: "barrier".into(),
            payload_json: r#"{"kind":"accept"}"#.into(),
            schema_version: "1.0".into(),
        }
    }

    #[tokio::test]
    async fn pending_archive_roundtrip() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("pending.jsonl.zst");
        let rows = [pending_row("p1"), pending_row("p2")];
        write_jsonl_zst(&path, rows.iter().cloned()).await.unwrap();
        let read_back: Vec<ArchivedPending> = read_jsonl_zst(&path).await.unwrap();
        assert_eq!(read_back.len(), 2);
        assert_eq!(read_back[0].proposal_id, "p1");
        assert_eq!(read_back[1].proposal_id, "p2");
    }

    #[tokio::test]
    async fn empty_archive_roundtrip() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("pending.jsonl.zst");
        write_jsonl_zst::<_, ArchivedPending>(&path, std::iter::empty())
            .await
            .unwrap();
        let read_back: Vec<ArchivedPending> = read_jsonl_zst(&path).await.unwrap();
        assert!(read_back.is_empty());
    }

    #[tokio::test]
    async fn missing_archive_reads_as_empty() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("never-written.jsonl.zst");
        let rows: Vec<ArchivedPending> = read_jsonl_zst(&path).await.unwrap();
        assert!(rows.is_empty());
    }

    #[tokio::test]
    async fn merge_events_dedupes_by_event_id() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("2026-05.jsonl.zst");
        let first = vec![
            event_row("a", "2026-05-01T00:00:00.000Z"),
            event_row("b", "2026-05-02T00:00:00.000Z"),
        ];
        merge_events_archive(&path, first).await.unwrap();

        // Second merge: one duplicate, one new.
        let second = vec![
            event_row("b", "2026-05-02T00:00:00.000Z"),
            event_row("c", "2026-05-03T00:00:00.000Z"),
        ];
        merge_events_archive(&path, second).await.unwrap();

        let read_back: Vec<ArchivedEvent> = read_jsonl_zst(&path).await.unwrap();
        assert_eq!(read_back.len(), 3);
        // Sorted by ts asc.
        let ids: Vec<&str> = read_back.iter().map(|r| r.event_id.as_str()).collect();
        assert_eq!(ids, vec!["a", "b", "c"]);
    }

    #[test]
    fn month_key_extracts_yyyy_mm() {
        assert_eq!(month_key("2026-05-16T14:22:01.481Z"), "2026-05");
        assert_eq!(month_key("2024-12-31T23:59:59.999Z"), "2024-12");
        // Malformed input falls through.
        assert_eq!(month_key("not a date"), "undated");
        assert_eq!(month_key(""), "undated");
    }

    #[test]
    fn retention_cutoff_is_in_the_past() {
        // Capture a single "now" snapshot so both arms of the assertion
        // use the same instant — eliminating the wall-clock race that
        // previously made this test flaky.
        use std::time::{SystemTime, UNIX_EPOCH};
        let now_ms = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_millis() as u64)
            .unwrap_or(0);
        let now_str = crate::ids::rfc3339_from_unix_ms_pub(now_ms);

        let cutoff = retention_cutoff_at(90, now_ms);
        // Lex order on RFC 3339 == chronological order.
        assert!(cutoff < now_str, "cutoff {cutoff} not before now {now_str}");

        // 0-day retention: cutoff must equal now exactly (same snapshot).
        let zero = retention_cutoff_at(0, now_ms);
        assert_eq!(zero, now_str, "zero-day cutoff should equal now");
    }
}
