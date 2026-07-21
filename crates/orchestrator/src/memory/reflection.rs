//! Post-mission consolidation (V3 §4.9, §7.7).
//!
//! ## Idempotence (Tier-1 fix)
//!
//! Each barrier (accept / scrub) is emitted at most once per
//! `(mission_id, kind)`. The kernel checks for an existing barrier
//! event before starting reflection; if it exists, the call is a
//! no-op and returns an empty outcome. This is what makes barrier
//! reflection safe to retry — the user noted that without it,
//! re-running `on_accept` would re-record `UserAccepted` witnesses
//! and shift confidence on every call.
//!
//! The witness-row UNIQUE constraint is the durable defense, but
//! checking the barrier first keeps the event log uncluttered and
//! avoids generating spurious confidence-computed events.

use std::collections::HashSet;

use sqlx::SqlitePool;

use event_schema::memory::{
    BarrierKind, MemoryBarrier, MemoryConfidenceComputed, MemoryDemoted, MemoryPromoted, NoteState,
    WitnessKind, MEMORY_SCHEMA_VERSION,
};

use super::error::MemoryError;
use super::ids;
use super::policy::{predicate, promotion_threshold, PromotionDecision};
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
    // Idempotence gate: if a barrier of this kind already exists for
    // the mission, return immediately. Cheap query, indexed by
    // (mission_id, ts).
    if barrier_already_emitted(pool, mission_id, kind).await? {
        return Ok(ReflectionOutcome {
            touched_notes: Vec::new(),
            witnesses_recorded: 0,
            promotions: 0,
            already_processed: true,
        });
    }

    // Emit the barrier event FIRST so its id is the causal anchor for
    // every witness recorded below. This is what makes
    // `source_event_id` meaningful for replay.
    let barrier_event_id = ids::new_memory_event_id();
    emit_barrier(pool, mission_id, kind, &barrier_event_id).await?;

    let touched = notes_touched_by_mission(pool, mission_id).await?;
    let witness_kind = match kind {
        BarrierKind::Accept | BarrierKind::Explicit => WitnessKind::UserAccepted,
        BarrierKind::Scrub => WitnessKind::UserScrubbed,
    };
    let mut witnesses_recorded = 0u32;
    let mut state_changes = 0u32;

    for note_id in &touched {
        if let witnesses::Recorded::Inserted(_) =
            witnesses::record(pool, note_id, witness_kind, &barrier_event_id).await?
        {
            witnesses_recorded += 1;
        }
        let transition_event_id = ids::new_memory_event_id();
        match kind {
            BarrierKind::Accept | BarrierKind::Explicit => {
                if try_promote(pool, store, note_id, &transition_event_id, Some(mission_id))
                    .await?
                    .is_some()
                {
                    state_changes += 1;
                }
            }
            BarrierKind::Scrub => {
                if try_demote(pool, store, note_id).await?.is_some() {
                    state_changes += 1;
                }
            }
        }
    }

    Ok(ReflectionOutcome {
        touched_notes: touched.into_iter().collect(),
        witnesses_recorded,
        promotions: state_changes,
        already_processed: false,
    })
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

async fn try_demote(
    pool: &SqlitePool,
    store: &MemoryStore,
    note_id: &str,
) -> Result<Option<String>, MemoryError> {
    let note = store.note_show(note_id).await?;
    if note.state != NoteState::Promoted {
        return Ok(None);
    }
    let ws = witnesses::for_note(pool, note_id).await?;
    let now_ms = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0);
    let conf = scoring::confidence_cached(note_id, &ws, now_ms);
    let threshold = promotion_threshold(pool, &note.kind).await?;
    if conf + f64::EPSILON < threshold {
        // F-015: wrap confidence event + MemoryDemoted event + state UPDATE
        // in a single transaction so they either all land or all roll back.
        let mut tx = pool.begin().await?;

        // Confidence event (was a bare pool.execute — now inside the tx).
        let conf_event_id = ids::new_memory_event_id();
        let conf_payload = MemoryConfidenceComputed {
            note_id: note_id.to_owned(),
            confidence: conf,
            contributing_witnesses: ws.iter().map(|w| w.id.clone()).collect(),
        };
        sqlx::query(
            "INSERT INTO memory_events \
             (event_id, mission_id, worker_id, ts, type, payload_json, schema_version) \
             VALUES (?, NULL, NULL, ?, 'confidence_computed', ?, ?)",
        )
        .bind(&conf_event_id)
        .bind(crate::ids::rfc3339_now())
        .bind(serde_json::to_string(&conf_payload)?)
        .bind(MEMORY_SCHEMA_VERSION)
        .execute(&mut *tx)
        .await?;

        // F-014: emit MemoryDemoted event (was missing before).
        let demote_event_id = ids::new_memory_event_id();
        let demote_payload = MemoryDemoted {
            note_id: note_id.to_owned(),
            from_state: NoteState::Promoted,
            to_state: NoteState::Owned,
            confidence: conf,
        };
        sqlx::query(
            "INSERT INTO memory_events \
             (event_id, mission_id, worker_id, ts, type, payload_json, schema_version) \
             VALUES (?, NULL, NULL, ?, 'demoted', ?, ?)",
        )
        .bind(&demote_event_id)
        .bind(crate::ids::rfc3339_now())
        .bind(serde_json::to_string(&demote_payload)?)
        .bind(MEMORY_SCHEMA_VERSION)
        .execute(&mut *tx)
        .await?;

        // State transition.
        sqlx::query("UPDATE memory_notes SET state = 'owned' WHERE id = ?")
            .bind(note_id)
            .execute(&mut *tx)
            .await?;

        tx.commit().await?;
        Ok(Some(note_id.to_owned()))
    } else {
        // No demotion: still record confidence for audit trail (outside tx is fine).
        record_confidence_event(pool, note_id, conf, &ws).await?;
        Ok(None)
    }
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
    let kind_str = match kind {
        BarrierKind::Accept => "accept",
        BarrierKind::Scrub => "scrub",
        BarrierKind::Explicit => "explicit",
    };
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

async fn emit_barrier(
    pool: &SqlitePool,
    mission_id: &str,
    kind: BarrierKind,
    event_id: &str,
) -> Result<(), MemoryError> {
    let now = crate::ids::rfc3339_now();
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
    .bind(&now)
    .bind(serde_json::to_string(&payload)?)
    .bind(MEMORY_SCHEMA_VERSION)
    .execute(pool)
    .await?;
    Ok(())
}
