//! Post-mission consolidation (V3 §4.9, §7.7).
//!
//! ## Idempotence
//!
//! Each barrier (accept / scrub) is emitted at most once per
//! `(mission_id, kind)`. `BEGIN IMMEDIATE` makes the check and all
//! reflection effects a single SQLite writer transaction, including
//! across processes sharing the database. A completed barrier is a
//! no-op on retry; a failed attempt rolls back its witness, confidence,
//! state, audit, and barrier rows together.

use std::collections::HashSet;

use sqlx::{Sqlite, SqlitePool, Transaction};

use event_schema::memory::{
    BarrierKind, MemoryBarrier, MemoryConfidenceComputed, MemoryDemoted, MemoryPromoted, NoteState,
    WitnessKind, MEMORY_SCHEMA_VERSION,
};

use super::error::MemoryError;
use super::hierarchy::Note;
use super::ids;
use super::policy::{fallback_threshold, predicate, promotion_threshold, PromotionDecision};
use super::scoring;
use super::store::MemoryStore;
use super::witnesses;

/// Run the accept-barrier reflection pass. Idempotent at the mission
/// level — re-invoking against a mission that already has an accept
/// barrier returns an empty outcome.
pub async fn on_accept(
    pool: &SqlitePool,
    store: &MemoryStore,
    mission_id: &str,
) -> Result<ReflectionOutcome, MemoryError> {
    run_barrier(pool, store, mission_id, BarrierKind::Accept).await
}

pub async fn on_scrub(
    pool: &SqlitePool,
    store: &MemoryStore,
    mission_id: &str,
) -> Result<ReflectionOutcome, MemoryError> {
    run_barrier(pool, store, mission_id, BarrierKind::Scrub).await
}

#[derive(Debug, Clone)]
pub struct ReflectionOutcome {
    pub touched_notes: Vec<String>,
    pub witnesses_recorded: u32,
    /// Promotions on accept; demotions on scrub.
    pub promotions: u32,
    /// True if the barrier had already been processed for this
    /// mission and the call was a no-op.
    pub already_processed: bool,
}

async fn run_barrier(
    pool: &SqlitePool,
    store: &MemoryStore,
    mission_id: &str,
    kind: BarrierKind,
) -> Result<ReflectionOutcome, MemoryError> {
    run_barrier_with_rendezvous(pool, store, mission_id, kind, None).await
}

#[cfg(test)]
pub(crate) async fn on_accept_with_concurrency_rendezvous(
    pool: &SqlitePool,
    store: &MemoryStore,
    mission_id: &str,
    rendezvous: &tokio::sync::Barrier,
) -> Result<ReflectionOutcome, MemoryError> {
    run_barrier_with_rendezvous(
        pool,
        store,
        mission_id,
        BarrierKind::Accept,
        Some(rendezvous),
    )
    .await
}

async fn run_barrier_with_rendezvous(
    pool: &SqlitePool,
    store: &MemoryStore,
    mission_id: &str,
    kind: BarrierKind,
    concurrency_rendezvous: Option<&tokio::sync::Barrier>,
) -> Result<ReflectionOutcome, MemoryError> {
    if barrier_already_emitted(pool, mission_id, kind).await? {
        return Ok(already_processed_outcome());
    }

    if let Some(rendezvous) = concurrency_rendezvous {
        rendezvous.wait().await;
    }

    // Body files live outside SQLite, so validate and load them before taking
    // the database write reservation. No durable reflection effect exists yet;
    // a transient file error therefore leaves a clean attempt to retry.
    let mut touched = notes_touched_by_mission(pool, mission_id).await?;
    touched.sort();
    let mut notes = Vec::with_capacity(touched.len());
    for note_id in &touched {
        notes.push(store.note_show(note_id).await?);
    }

    // BEGIN IMMEDIATE is the cross-process exactly-once gate. SQLite grants a
    // single writer reservation for the shared database file; after waiting,
    // every contender re-checks the durable barrier in the same transaction.
    // Witness, confidence, state transition, audit event, and barrier seal all
    // commit together, so a failed attempt leaves nothing for a retry to
    // duplicate.
    let mut tx = pool.begin_with("BEGIN IMMEDIATE").await?;
    if barrier_already_emitted_in_tx(&mut tx, mission_id, kind).await? {
        tx.rollback().await?;
        return Ok(already_processed_outcome());
    }

    let barrier_event_id = ids::new_memory_event_id();
    let witness_kind = match kind {
        BarrierKind::Accept | BarrierKind::Explicit => WitnessKind::UserAccepted,
        BarrierKind::Scrub => WitnessKind::UserScrubbed,
    };
    let mut witnesses_recorded = 0u32;
    let mut state_changes = 0u32;

    for (note_id, note) in touched.iter().zip(notes.iter_mut()) {
        if let witnesses::Recorded::Inserted(_) =
            witnesses::record_in_tx(&mut tx, note_id, witness_kind, &barrier_event_id).await?
        {
            witnesses_recorded += 1;
        }
        let transition_event_id = ids::new_memory_event_id();
        match kind {
            BarrierKind::Accept | BarrierKind::Explicit => {
                if try_promote_in_tx(&mut tx, note, &transition_event_id, Some(mission_id))
                    .await?
                    .is_some()
                {
                    state_changes += 1;
                }
            }
            BarrierKind::Scrub => {
                if try_demote_in_tx(&mut tx, note).await?.is_some() {
                    state_changes += 1;
                }
            }
        }
    }

    emit_barrier_in_tx(&mut tx, mission_id, kind, &barrier_event_id).await?;
    tx.commit().await?;

    Ok(ReflectionOutcome {
        touched_notes: touched.into_iter().collect(),
        witnesses_recorded,
        promotions: state_changes,
        already_processed: false,
    })
}

fn already_processed_outcome() -> ReflectionOutcome {
    ReflectionOutcome {
        touched_notes: Vec::new(),
        witnesses_recorded: 0,
        promotions: 0,
        already_processed: true,
    }
}

async fn try_promote_in_tx(
    tx: &mut Transaction<'_, Sqlite>,
    note: &mut Note,
    event_id: &str,
    mission_id: Option<&str>,
) -> Result<Option<String>, MemoryError> {
    let state = note_state_in_tx(tx, &note.id).await?;
    note.state = state;
    if state != NoteState::Owned {
        return Ok(None);
    }

    let ws = witnesses_for_note_in_tx(tx, &note.id).await?;
    let now_ms = unix_ms_now();
    let confidence = scoring::confidence_cached(&note.id, &ws, now_ms);
    record_confidence_event_in_tx(tx, &note.id, confidence, &ws).await?;
    let threshold = promotion_threshold_in_tx(tx, &note.kind).await?;
    if blocking_conflict_exists_in_tx(tx, &note.id, confidence, now_ms).await?
        || !matches!(
            predicate(note, confidence, threshold, &ws),
            PromotionDecision::Promote
        )
    {
        return Ok(None);
    }

    let payload = MemoryPromoted {
        note_id: note.id.clone(),
        from_state: NoteState::Owned,
        to_state: NoteState::Promoted,
        confidence,
    };
    sqlx::query("UPDATE memory_notes SET state = 'promoted' WHERE id = ? AND state = 'owned'")
        .bind(&note.id)
        .execute(&mut **tx)
        .await?;
    sqlx::query(
        "INSERT INTO memory_events \
         (event_id, mission_id, worker_id, ts, type, payload_json, schema_version) \
         VALUES (?, ?, NULL, ?, 'promoted', ?, ?)",
    )
    .bind(event_id)
    .bind(mission_id)
    .bind(crate::ids::rfc3339_now())
    .bind(serde_json::to_string(&payload)?)
    .bind(MEMORY_SCHEMA_VERSION)
    .execute(&mut **tx)
    .await?;
    note.state = NoteState::Promoted;
    Ok(Some(event_id.to_owned()))
}

async fn try_demote_in_tx(
    tx: &mut Transaction<'_, Sqlite>,
    note: &mut Note,
) -> Result<Option<String>, MemoryError> {
    let state = note_state_in_tx(tx, &note.id).await?;
    note.state = state;
    if state != NoteState::Promoted {
        return Ok(None);
    }

    let ws = witnesses_for_note_in_tx(tx, &note.id).await?;
    let confidence = scoring::confidence_cached(&note.id, &ws, unix_ms_now());
    let threshold = promotion_threshold_in_tx(tx, &note.kind).await?;
    record_confidence_event_in_tx(tx, &note.id, confidence, &ws).await?;
    if confidence + f64::EPSILON >= threshold {
        return Ok(None);
    }

    let event_id = ids::new_memory_event_id();
    let payload = MemoryDemoted {
        note_id: note.id.clone(),
        from_state: NoteState::Promoted,
        to_state: NoteState::Owned,
        confidence,
    };
    sqlx::query("UPDATE memory_notes SET state = 'owned' WHERE id = ? AND state = 'promoted'")
        .bind(&note.id)
        .execute(&mut **tx)
        .await?;
    sqlx::query(
        "INSERT INTO memory_events \
         (event_id, mission_id, worker_id, ts, type, payload_json, schema_version) \
         VALUES (?, NULL, NULL, ?, 'demoted', ?, ?)",
    )
    .bind(&event_id)
    .bind(crate::ids::rfc3339_now())
    .bind(serde_json::to_string(&payload)?)
    .bind(MEMORY_SCHEMA_VERSION)
    .execute(&mut **tx)
    .await?;
    note.state = NoteState::Owned;
    Ok(Some(event_id))
}

async fn note_state_in_tx(
    tx: &mut Transaction<'_, Sqlite>,
    note_id: &str,
) -> Result<NoteState, MemoryError> {
    let state: Option<(String,)> = sqlx::query_as("SELECT state FROM memory_notes WHERE id = ?")
        .bind(note_id)
        .fetch_optional(&mut **tx)
        .await?;
    let state = state.ok_or_else(|| MemoryError::NoteNotFound(note_id.to_owned()))?;
    NoteState::from_str(&state.0)
        .ok_or_else(|| MemoryError::RowCorrupt(format!("state {}", state.0)))
}

async fn witnesses_for_note_in_tx(
    tx: &mut Transaction<'_, Sqlite>,
    note_id: &str,
) -> Result<Vec<witnesses::Witness>, MemoryError> {
    let rows: Vec<(String, String, String, f64, String, String)> = sqlx::query_as(
        "SELECT id, note_id, kind, weight, source_event_id, observed_at \
         FROM memory_witnesses WHERE note_id = ? ORDER BY observed_at ASC",
    )
    .bind(note_id)
    .fetch_all(&mut **tx)
    .await?;
    rows.into_iter()
        .map(
            |(id, note_id, kind, weight, source_event_id, observed_at)| {
                let kind = WitnessKind::from_str(&kind)
                    .ok_or_else(|| MemoryError::RowCorrupt(format!("witness kind {kind}")))?;
                Ok(witnesses::Witness {
                    id,
                    note_id,
                    kind,
                    weight,
                    source_event_id,
                    observed_at,
                })
            },
        )
        .collect()
}

async fn promotion_threshold_in_tx(
    tx: &mut Transaction<'_, Sqlite>,
    kind: &event_schema::memory::NoteKind,
) -> Result<f64, MemoryError> {
    let row: Option<(Option<f64>,)> = sqlx::query_as(
        "SELECT promote_threshold FROM memory_taxonomy WHERE category = 'kind' AND name = ?",
    )
    .bind(kind.as_str())
    .fetch_optional(&mut **tx)
    .await?;
    Ok(row
        .and_then(|(threshold,)| threshold)
        .unwrap_or_else(|| fallback_threshold(kind)))
}

async fn blocking_conflict_exists_in_tx(
    tx: &mut Transaction<'_, Sqlite>,
    note_id: &str,
    confidence: f64,
    now_ms: u64,
) -> Result<bool, MemoryError> {
    let candidates: Vec<(String,)> = sqlx::query_as(
        "SELECT dst_note_id FROM memory_links \
         WHERE src_note_id = ? AND link_kind = 'conflicts_with'",
    )
    .bind(note_id)
    .fetch_all(&mut **tx)
    .await?;
    for (other_id,) in candidates {
        let state: Option<(String,)> =
            sqlx::query_as("SELECT state FROM memory_notes WHERE id = ?")
                .bind(&other_id)
                .fetch_optional(&mut **tx)
                .await?;
        if state.as_ref().map(|row| row.0.as_str()) != Some("promoted") {
            continue;
        }
        let other_ws = witnesses_for_note_in_tx(tx, &other_id).await?;
        if scoring::confidence(&other_ws, now_ms) >= confidence {
            return Ok(true);
        }
    }
    Ok(false)
}

fn unix_ms_now() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|duration| duration.as_millis() as u64)
        .unwrap_or(0)
}

/// Try to promote a single note. Pure delegation to predicate + the
/// conflict check; emits `MemoryPromoted` and flips state if all
/// conjuncts hold. Returns the promotion event id on success.
pub(crate) async fn try_promote(
    pool: &SqlitePool,
    store: &MemoryStore,
    note_id: &str,
    event_id: &str,
    mission_id: Option<&str>,
) -> Result<Option<String>, MemoryError> {
    let note = store.note_show(note_id).await?;
    if note.state != NoteState::Owned {
        return Ok(None);
    }
    let ws = witnesses::for_note(pool, note_id).await?;
    let now_ms = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0);
    let conf = scoring::confidence_cached(note_id, &ws, now_ms);
    record_confidence_event(pool, note_id, conf, &ws).await?;

    let threshold = promotion_threshold(pool, &note.kind).await?;

    if blocking_conflict_exists(pool, store, note_id).await? {
        return Ok(None);
    }

    let decision = predicate(&note, conf, threshold, &ws);
    if !matches!(decision, PromotionDecision::Promote) {
        return Ok(None);
    }

    let now = crate::ids::rfc3339_now();
    let payload = MemoryPromoted {
        note_id: note_id.to_owned(),
        from_state: NoteState::Owned,
        to_state: NoteState::Promoted,
        confidence: conf,
    };
    let mut tx = pool.begin().await?;
    sqlx::query("UPDATE memory_notes SET state = 'promoted' WHERE id = ?")
        .bind(note_id)
        .execute(&mut *tx)
        .await?;
    sqlx::query(
        "INSERT INTO memory_events \
         (event_id, mission_id, worker_id, ts, type, payload_json, schema_version) \
         VALUES (?, ?, NULL, ?, 'promoted', ?, ?)",
    )
    .bind(event_id)
    .bind(mission_id)
    .bind(&now)
    .bind(serde_json::to_string(&payload)?)
    .bind(MEMORY_SCHEMA_VERSION)
    .execute(&mut *tx)
    .await?;
    tx.commit().await?;
    Ok(Some(event_id.to_owned()))
}

// ---------------------------------------------------------------------
// Internal helpers
// ---------------------------------------------------------------------

async fn notes_touched_by_mission(
    pool: &SqlitePool,
    mission_id: &str,
) -> Result<Vec<String>, MemoryError> {
    let mut set: HashSet<String> = HashSet::new();

    let pending: Vec<(String,)> = sqlx::query_as(
        "SELECT p.proposal_id FROM memory_pending p \
         WHERE p.mission_id = ? AND p.state = 'ratified'",
    )
    .bind(mission_id)
    .fetch_all(pool)
    .await?;
    for (proposal_id,) in pending {
        let rows: Vec<(String,)> = sqlx::query_as(
            "SELECT note_id FROM memory_provenance \
             WHERE event_id IN (SELECT event_id FROM memory_events \
                                WHERE type = 'ratified' \
                                AND payload_json LIKE ?)",
        )
        .bind(format!("%\"proposal_id\":\"{proposal_id}\"%"))
        .fetch_all(pool)
        .await?;
        for (n,) in rows {
            set.insert(n);
        }
    }

    let bundles: Vec<(String,)> =
        sqlx::query_as("SELECT page_table_json FROM memory_bundles WHERE mission_id = ?")
            .bind(mission_id)
            .fetch_all(pool)
            .await?;
    for (page_table_json,) in bundles {
        // F-018: log parse errors so ops can detect corrupt bundle rows;
        // behavior is unchanged (skip the bundle) but now visible.
        match serde_json::from_str::<Vec<serde_json::Value>>(&page_table_json) {
            Ok(parsed) => {
                for entry in parsed {
                    if let Some(id) = entry.get("note_id").and_then(|v| v.as_str()) {
                        set.insert(id.to_owned());
                    }
                }
            }
            Err(e) => {
                tracing::error!(
                    "vigla: malformed page_table_json for mission {mission_id} bundle: {e}"
                );
            }
        }
    }

    Ok(set.into_iter().collect())
}

async fn record_confidence_event(
    pool: &SqlitePool,
    note_id: &str,
    confidence: f64,
    ws: &[witnesses::Witness],
) -> Result<(), MemoryError> {
    let event_id = ids::new_memory_event_id();
    let now = crate::ids::rfc3339_now();
    let payload = MemoryConfidenceComputed {
        note_id: note_id.to_owned(),
        confidence,
        contributing_witnesses: ws.iter().map(|w| w.id.clone()).collect(),
    };
    sqlx::query(
        "INSERT INTO memory_events \
         (event_id, mission_id, worker_id, ts, type, payload_json, schema_version) \
         VALUES (?, NULL, NULL, ?, 'confidence_computed', ?, ?)",
    )
    .bind(&event_id)
    .bind(&now)
    .bind(serde_json::to_string(&payload)?)
    .bind(MEMORY_SCHEMA_VERSION)
    .execute(pool)
    .await?;
    Ok(())
}

async fn record_confidence_event_in_tx(
    tx: &mut Transaction<'_, Sqlite>,
    note_id: &str,
    confidence: f64,
    ws: &[witnesses::Witness],
) -> Result<(), MemoryError> {
    let payload = MemoryConfidenceComputed {
        note_id: note_id.to_owned(),
        confidence,
        contributing_witnesses: ws.iter().map(|witness| witness.id.clone()).collect(),
    };
    sqlx::query(
        "INSERT INTO memory_events \
         (event_id, mission_id, worker_id, ts, type, payload_json, schema_version) \
         VALUES (?, NULL, NULL, ?, 'confidence_computed', ?, ?)",
    )
    .bind(ids::new_memory_event_id())
    .bind(crate::ids::rfc3339_now())
    .bind(serde_json::to_string(&payload)?)
    .bind(MEMORY_SCHEMA_VERSION)
    .execute(&mut **tx)
    .await?;
    Ok(())
}

async fn blocking_conflict_exists(
    pool: &SqlitePool,
    store: &MemoryStore,
    note_id: &str,
) -> Result<bool, MemoryError> {
    let candidates: Vec<(String,)> = sqlx::query_as(
        "SELECT dst_note_id FROM memory_links \
         WHERE src_note_id = ? AND link_kind = 'conflicts_with'",
    )
    .bind(note_id)
    .fetch_all(pool)
    .await?;
    if candidates.is_empty() {
        return Ok(false);
    }
    let mine_ws = witnesses::for_note(pool, note_id).await?;
    let now_ms = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0);
    let mine = scoring::confidence(&mine_ws, now_ms);
    for (other,) in candidates {
        let other_note = store.note_show(&other).await?;
        if other_note.state != NoteState::Promoted {
            continue;
        }
        let other_ws = witnesses::for_note(pool, &other).await?;
        let other_conf = scoring::confidence(&other_ws, now_ms);
        if other_conf >= mine {
            return Ok(true);
        }
    }
    Ok(false)
}

async fn barrier_already_emitted(
    pool: &SqlitePool,
    mission_id: &str,
    kind: BarrierKind,
) -> Result<bool, MemoryError> {
    let kind_str = barrier_kind_str(kind);
    // payload_json carries `{"mission_id": ..., "kind": "..."}`.
    let (count,): (i64,) = sqlx::query_as(
        "SELECT COUNT(*) FROM memory_events \
         WHERE type = 'barrier' AND mission_id = ? AND payload_json LIKE ?",
    )
    .bind(mission_id)
    .bind(format!("%\"kind\":\"{kind_str}\"%"))
    .fetch_one(pool)
    .await?;
    Ok(count > 0)
}

async fn barrier_already_emitted_in_tx(
    tx: &mut Transaction<'_, Sqlite>,
    mission_id: &str,
    kind: BarrierKind,
) -> Result<bool, MemoryError> {
    let kind_str = barrier_kind_str(kind);
    let (count,): (i64,) = sqlx::query_as(
        "SELECT COUNT(*) FROM memory_events \
         WHERE type = 'barrier' AND mission_id = ? AND payload_json LIKE ?",
    )
    .bind(mission_id)
    .bind(format!("%\"kind\":\"{kind_str}\"%"))
    .fetch_one(&mut **tx)
    .await?;
    Ok(count > 0)
}

fn barrier_kind_str(kind: BarrierKind) -> &'static str {
    match kind {
        BarrierKind::Accept => "accept",
        BarrierKind::Scrub => "scrub",
        BarrierKind::Explicit => "explicit",
    }
}

async fn emit_barrier_in_tx(
    tx: &mut Transaction<'_, Sqlite>,
    mission_id: &str,
    kind: BarrierKind,
    event_id: &str,
) -> Result<(), MemoryError> {
    let payload = MemoryBarrier {
        mission_id: mission_id.to_owned(),
        kind,
    };
    sqlx::query(
        "INSERT INTO memory_events \
         (event_id, mission_id, worker_id, ts, type, payload_json, schema_version) \
         VALUES (?, ?, NULL, ?, 'barrier', ?, ?)",
    )
    .bind(event_id)
    .bind(mission_id)
    .bind(crate::ids::rfc3339_now())
    .bind(serde_json::to_string(&payload)?)
    .bind(MEMORY_SCHEMA_VERSION)
    .execute(&mut **tx)
    .await?;
    Ok(())
}
