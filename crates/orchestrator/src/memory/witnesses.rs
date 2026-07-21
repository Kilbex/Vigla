//! Append-only witness signal store (V3 §2, §4.12).
//!
//! Every typed witness is one row in `memory_witnesses` and one
//! `MemoryWitnessRecorded` event. Nothing is ever updated or deleted
//! — confidence is *derived* from these rows by `scoring.rs`, so
//! changing scoring weights needs no data migration.
//!
//! ## Idempotence (Tier-1 fix)
//!
//! `record` takes a `source_event_id` that identifies the *causal*
//! event the witness represents. Together with `(note_id, kind)` this
//! forms a uniqueness key in `memory_witnesses`. A re-invocation
//! against the same source is a no-op — the existing row is left in
//! place and we return `Recorded::AlreadyExists`.
//!
//! This is the durable defense. Callers should *also* guard at the
//! operation level (e.g. `on_accept` checks for an existing accept
//! barrier before iterating), which avoids the extra event emission
//! and keeps the event log uncluttered.

use sqlx::SqlitePool;

use event_schema::memory::{MemoryWitnessRecorded, WitnessKind, MEMORY_SCHEMA_VERSION};

use super::error::MemoryError;
use super::ids;

/// One row in `memory_witnesses`, hydrated.
#[derive(Debug, Clone, PartialEq)]
pub struct Witness {
    pub id: String,
    pub note_id: String,
    pub kind: WitnessKind,
    pub weight: f64,
    /// Causal event id — see [`record`].
    pub source_event_id: String,
    pub observed_at: String,
}

/// Outcome of a [`record`] call. The constraint can race only if
/// two callers happen to pass the same causal event id concurrently;
/// in practice this never happens because the kernel serialises
/// barrier/ratify turns. We still report the outcome so callers can
/// distinguish "newly observed" from "already known."
#[derive(Debug, Clone, PartialEq)]
pub enum Recorded {
    Inserted(Witness),
    AlreadyExists,
}

/// Record a witness signal inside an *existing* transaction.
///
/// Same as [`record`] but operates on a caller-supplied transaction; the
/// caller is responsible for committing (or rolling back). Used by
/// `ratify_one` to keep witness writes atomic with the note + provenance +
/// event writes that precede them (F-012 fix).
pub(crate) async fn record_in_tx(
    tx: &mut sqlx::Transaction<'_, sqlx::Sqlite>,
    note_id: &str,
    kind: WitnessKind,
    source_event_id: &str,
) -> Result<Recorded, MemoryError> {
    let witness_id = ids::new_memory_event_id();
    let event_id = ids::new_memory_event_id();
    let observed_at = crate::ids::rfc3339_now();
    let weight = kind.default_weight();

    let insert = sqlx::query(
        "INSERT INTO memory_witnesses (id, note_id, kind, weight, source_event_id, observed_at) \
         VALUES (?, ?, ?, ?, ?, ?)",
    )
    .bind(&witness_id)
    .bind(note_id)
    .bind(kind.as_str())
    .bind(weight)
    .bind(source_event_id)
    .bind(&observed_at)
    .execute(&mut **tx)
    .await;

    match insert {
        Err(sqlx::Error::Database(db_err)) if db_err.is_unique_violation() => {
            // Already recorded for this causal source — no-op.
            return Ok(Recorded::AlreadyExists);
        }
        Err(e) => return Err(MemoryError::Sqlx(e)),
        Ok(_) => {}
    }

    let payload = MemoryWitnessRecorded {
        witness_id: witness_id.clone(),
        note_id: note_id.to_owned(),
        kind,
        weight,
        source_event_id: source_event_id.to_owned(),
        observed_at: observed_at.clone(),
    };
    sqlx::query(
        "INSERT INTO memory_events \
         (event_id, mission_id, worker_id, ts, type, payload_json, schema_version) \
         VALUES (?, NULL, NULL, ?, 'witness_recorded', ?, ?)",
    )
    .bind(&event_id)
    .bind(&observed_at)
    .bind(serde_json::to_string(&payload)?)
    .bind(MEMORY_SCHEMA_VERSION)
    .execute(&mut **tx)
    .await?;

    Ok(Recorded::Inserted(Witness {
        id: witness_id,
        note_id: note_id.to_owned(),
        kind,
        weight,
        source_event_id: source_event_id.to_owned(),
        observed_at,
    }))
}

/// Record a witness signal against a note, *causally tied* to
/// `source_event_id`. The kind's `default_weight` at observation time
/// is snapshotted on the row — future scoring-weight tuning therefore
/// doesn't rewrite history. Also persists a `MemoryWitnessRecorded`
/// event in the same transaction so replay can reconstruct
/// confidence at any point.
pub async fn record(
    pool: &SqlitePool,
    note_id: &str,
    kind: WitnessKind,
    source_event_id: &str,
) -> Result<Recorded, MemoryError> {
    let mut tx = pool.begin().await?;
    let result = record_in_tx(&mut tx, note_id, kind, source_event_id).await?;
    tx.commit().await?;
    Ok(result)
}

/// All witnesses observed for a note, oldest-first. Lightweight scan —
/// indexed by `(note_id, observed_at)` so even a long-lived note's
/// witness count stays cheap to fetch.
pub async fn for_note(pool: &SqlitePool, note_id: &str) -> Result<Vec<Witness>, MemoryError> {
    let rows: Vec<(String, String, String, f64, String, String)> = sqlx::query_as(
        "SELECT id, note_id, kind, weight, source_event_id, observed_at \
         FROM memory_witnesses WHERE note_id = ? ORDER BY observed_at ASC",
    )
    .bind(note_id)
    .fetch_all(pool)
    .await?;
    let mut out = Vec::with_capacity(rows.len());
    for (id, note_id, kind, weight, source_event_id, observed_at) in rows {
        let kind = WitnessKind::from_str(&kind)
            .ok_or_else(|| MemoryError::RowCorrupt(format!("witness kind {kind}")))?;
        out.push(Witness {
            id,
            note_id,
            kind,
            weight,
            source_event_id,
            observed_at,
        });
    }
    Ok(out)
}

/// Number of qualifying witnesses (V3 §7.5 — "at least one outcome
/// or human witness"). Used by the promotion predicate.
pub fn qualifying_count(witnesses: &[Witness]) -> u32 {
    witnesses.iter().filter(|w| w.kind.is_qualifying()).count() as u32
}

/// Convenience: does this slice contain any `UserAuthored` witness?
/// Drives the predicate's user-oracle shortcut.
pub fn has_user_authored(witnesses: &[Witness]) -> bool {
    witnesses
        .iter()
        .any(|w| w.kind == WitnessKind::UserAuthored)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::memory::hierarchy::{NoteKind, Scope, ScopeKind, StandardNoteKind};
    use crate::memory::{MemoryStore, NewNote};
    use sqlx::sqlite::{SqliteConnectOptions, SqlitePoolOptions};
    use std::str::FromStr;
    use tempfile::TempDir;

    async fn fresh() -> (SqlitePool, MemoryStore, TempDir) {
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
        let dir = TempDir::new().unwrap();
        let store = MemoryStore::open(pool.clone(), dir.path().to_path_buf())
            .await
            .unwrap();
        (pool, store, dir)
    }

    /// Use the test-only seed helper so we don't get a UserAuthored
    /// witness for free. These tests assert on raw witness counts
    /// and need to control them precisely.
    async fn seed_note(store: &MemoryStore) -> String {
        store
            ._test_seed_owned_note(NewNote {
                kind: NoteKind::Standard(StandardNoteKind::Fact),
                scope: Scope {
                    kind: ScopeKind::Repo,
                    value: None,
                },
                body: "x".into(),
            })
            .await
            .unwrap()
    }
    #[tokio::test]
    async fn record_inserts_and_returns_witness() {
        let (pool, store, _dir) = fresh().await;
        let n = seed_note(&store).await;
        let r = record(&pool, &n, WitnessKind::UserAccepted, "ev-001")
            .await
            .unwrap();
        match r {
            Recorded::Inserted(w) => {
                assert_eq!(w.kind, WitnessKind::UserAccepted);
                assert_eq!(w.source_event_id, "ev-001");
                assert_eq!(w.weight, 0.4);
            }
            _ => panic!("expected Inserted"),
        }
    }

    #[tokio::test]
    async fn duplicate_source_is_idempotent_no_op() {
        let (pool, store, _dir) = fresh().await;
        let n = seed_note(&store).await;
        let first = record(&pool, &n, WitnessKind::UserAccepted, "ev-001")
            .await
            .unwrap();
        assert!(matches!(first, Recorded::Inserted(_)));
        let second = record(&pool, &n, WitnessKind::UserAccepted, "ev-001")
            .await
            .unwrap();
        assert_eq!(second, Recorded::AlreadyExists);

        // Only one row in memory_witnesses, only one witness_recorded event.
        let (witness_rows,): (i64,) = sqlx::query_as("SELECT COUNT(*) FROM memory_witnesses")
            .fetch_one(&pool)
            .await
            .unwrap();
        assert_eq!(witness_rows, 1);
        let (event_rows,): (i64,) =
            sqlx::query_as("SELECT COUNT(*) FROM memory_events WHERE type = 'witness_recorded'")
                .fetch_one(&pool)
                .await
                .unwrap();
        assert_eq!(event_rows, 1);
    }

    #[tokio::test]
    async fn different_sources_create_distinct_witnesses() {
        let (pool, store, _dir) = fresh().await;
        let n = seed_note(&store).await;
        record(&pool, &n, WitnessKind::UserAccepted, "ev-A")
            .await
            .unwrap();
        record(&pool, &n, WitnessKind::UserAccepted, "ev-B")
            .await
            .unwrap();
        let ws = for_note(&pool, &n).await.unwrap();
        assert_eq!(ws.len(), 2);
        let sources: std::collections::HashSet<&str> =
            ws.iter().map(|w| w.source_event_id.as_str()).collect();
        assert!(sources.contains("ev-A"));
        assert!(sources.contains("ev-B"));
    }

    #[tokio::test]
    async fn record_persists_source_event_id_in_event() {
        let (pool, store, _dir) = fresh().await;
        let n = seed_note(&store).await;
        record(&pool, &n, WitnessKind::UserAccepted, "barrier-xyz")
            .await
            .unwrap();
        let (payload,): (String,) = sqlx::query_as(
            "SELECT payload_json FROM memory_events WHERE type = 'witness_recorded'",
        )
        .fetch_one(&pool)
        .await
        .unwrap();
        assert!(payload.contains("\"source_event_id\":\"barrier-xyz\""));
    }

    #[test]
    fn qualifying_excludes_negative_witnesses() {
        let make = |kind| Witness {
            id: "i".into(),
            note_id: "n".into(),
            kind,
            weight: 0.0,
            source_event_id: "e".into(),
            observed_at: "t".into(),
        };
        let ws = vec![
            make(WitnessKind::WorkerProposed),
            make(WitnessKind::UserAccepted),
            make(WitnessKind::DerivedFromUntrustedFile),
        ];
        assert_eq!(qualifying_count(&ws), 1);
    }

    #[test]
    fn has_user_authored_returns_true_only_when_present() {
        let w = Witness {
            id: "i".into(),
            note_id: "n".into(),
            kind: WitnessKind::UserAuthored,
            weight: 1.0,
            source_event_id: "e".into(),
            observed_at: "t".into(),
        };
        assert!(has_user_authored(std::slice::from_ref(&w)));
        let none: Vec<Witness> = vec![];
        assert!(!has_user_authored(&none));
    }

    /// F-012 verification: record_in_tx writes a witness inside an existing
    /// transaction. Rolling back the outer tx must also rollback the witness row.
    #[tokio::test]
    async fn record_in_tx_rolls_back_with_outer_tx() {
        let (pool, store, _dir) = fresh().await;
        let n = seed_note(&store).await;

        // Open a tx, record_in_tx, then drop without commit (== rollback).
        {
            let mut tx = pool.begin().await.unwrap();
            record_in_tx(&mut tx, &n, WitnessKind::WorkerProposed, "ev-rb-1")
                .await
                .unwrap();
            // Don't commit — drop(tx) rolls back.
        }

        let count: (i64,) =
            sqlx::query_as("SELECT COUNT(*) FROM memory_witnesses WHERE source_event_id = ?")
                .bind("ev-rb-1")
                .fetch_one(&pool)
                .await
                .unwrap();
        assert_eq!(
            count.0, 0,
            "uncommitted witness should NOT be visible, got {} rows",
            count.0
        );
    }
}
