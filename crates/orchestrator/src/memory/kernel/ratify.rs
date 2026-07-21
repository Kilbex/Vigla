//! `MemoryKernel::ratify` and the per-decision `ratify_one` helper.

use event_schema::memory::{
    MemoryNormalized, MemoryRatified, MemoryRejected, NoteKind, RatifyDecision, Scope, WitnessKind,
    MEMORY_SCHEMA_VERSION,
};

use super::super::error::MemoryError;
use super::super::ids;
use super::super::store::NewNote;
use super::super::witnesses;
use super::types::{RatificationDecision, RatifyInput, RatifyOutcome};
use super::MemoryKernel;

/// Classify a `derived_from` source string. P2 uses simple prefix
/// heuristics; P5 may swap in a richer policy with a path allowlist.
/// Anything starting with `url:`, `vendor_file:`, or `external:` is
/// treated as outside the worktree. Sources beginning with
/// `worktree:` are trusted.
fn is_untrusted_source(s: &str) -> bool {
    let lower = s.to_ascii_lowercase();
    lower.starts_with("url:")
        || lower.starts_with("vendor_file:")
        || lower.starts_with("external:")
        || lower.starts_with("http://")
        || lower.starts_with("https://")
}

impl MemoryKernel {
    /// Supervisor's batched critique (V3 §11). Each decision is
    /// applied independently — accepted ones mint a note in
    /// `state=owned`, rejected ones emit `MemoryRejected`. Returns
    /// outcomes in the same order as input for the UI to display.
    pub async fn ratify(
        &self,
        decisions: Vec<RatifyInput>,
    ) -> Result<Vec<RatifyOutcome>, MemoryError> {
        let mut out = Vec::with_capacity(decisions.len());
        for d in decisions {
            out.push(self.ratify_one(d).await?);
        }
        Ok(out)
    }

    /// Apply a single ratification decision atomically (Tier-1 fix).
    ///
    /// For `Accept`: emits `MemoryNormalized` (if body changed),
    /// `MemoryRatified`, mints the note row + provenance, and updates
    /// the pending row — **all in one transaction**. The note's
    /// `created_event_id` is the `MemoryRatified` event id, so the
    /// genesis link resolves to a real event row. After commit, the
    /// initial witnesses (`WorkerProposed`, optional
    /// `DerivedFromUntrustedFile`) are recorded with the ratified
    /// event id as their causal source.
    async fn ratify_one(&self, d: RatifyInput) -> Result<RatifyOutcome, MemoryError> {
        type PendingRow = (
            String,
            String,
            String,
            String,
            Option<String>,
            String,
            String,
            String,
            String,
        );
        let row: Option<PendingRow> = sqlx::query_as(
            "SELECT mission_id, worker_id, kind, scope_kind, scope_value, body, \
                    derived_from, evidence, state \
             FROM memory_pending WHERE proposal_id = ?",
        )
        .bind(&d.proposal_id)
        .fetch_optional(&self.pool)
        .await?;
        let (
            mission_id,
            worker_id,
            kind_str,
            scope_kind,
            scope_value,
            original_body,
            derived_from_json,
            _ev,
            state,
        ) = row.ok_or_else(|| MemoryError::NoteNotFound(d.proposal_id.clone()))?;
        if state != "proposed" {
            return Err(MemoryError::RowCorrupt(format!(
                "ratify on non-proposed state: {state}"
            )));
        }

        match d.decision {
            RatificationDecision::Accept { normalized_body } => {
                let final_body = normalized_body.unwrap_or_else(|| original_body.clone());
                let kind = NoteKind::from_str(&kind_str);
                let scope = Scope {
                    kind: event_schema::memory::ScopeKind::from_str(&scope_kind)
                        .ok_or_else(|| MemoryError::RowCorrupt(scope_kind.clone()))?,
                    value: scope_value,
                };
                let new_note = NewNote {
                    kind: kind.clone(),
                    scope: scope.clone(),
                    body: final_body.clone(),
                };

                // Step 1: validate + write body file outside any tx.
                // After this returns, the file is durable on disk; if
                // we crash before the tx commit, it's an orphan file
                // (no row references it).
                let prepared = self.store.prepare_note(&new_note).await?;
                let now = crate::ids::rfc3339_now();
                let ratified_event_id = ids::new_memory_event_id();

                // Step 2: one transaction emits MemoryNormalized
                // (if body changed), MemoryRatified, mints the note
                // row + provenance, and updates the pending state.
                // created_event_id on the note points at the
                // ratified event — provenance is real, no dangling
                // pointers.
                let mut tx = self.pool.begin().await?;

                if final_body != original_body {
                    let norm_event_id = ids::new_memory_event_id();
                    let payload = MemoryNormalized {
                        proposal_id: d.proposal_id.clone(),
                        kind: kind.clone(),
                        scope: scope.clone(),
                        normalized_body: final_body.clone(),
                    };
                    sqlx::query(
                        "INSERT INTO memory_events \
                         (event_id, mission_id, worker_id, ts, type, payload_json, schema_version) \
                         VALUES (?, ?, ?, ?, 'normalized', ?, ?)",
                    )
                    .bind(&norm_event_id)
                    .bind(&mission_id)
                    .bind(&worker_id)
                    .bind(&now)
                    .bind(serde_json::to_string(&payload)?)
                    .bind(MEMORY_SCHEMA_VERSION)
                    .execute(&mut *tx)
                    .await?;
                }

                let payload = MemoryRatified {
                    proposal_id: d.proposal_id.clone(),
                    note_id: Some(prepared.note_id.clone()),
                    decision: RatifyDecision::Accept,
                    reason: d.reason.clone(),
                    merged_into_note_id: None,
                };
                sqlx::query(
                    "INSERT INTO memory_events \
                     (event_id, mission_id, worker_id, ts, type, payload_json, schema_version) \
                     VALUES (?, ?, ?, ?, 'ratified', ?, ?)",
                )
                .bind(&ratified_event_id)
                .bind(&mission_id)
                .bind(&worker_id)
                .bind(&now)
                .bind(serde_json::to_string(&payload)?)
                .bind(MEMORY_SCHEMA_VERSION)
                .execute(&mut *tx)
                .await?;

                let note_id = self
                    .store
                    .mint_note_in_tx(
                        &mut tx,
                        &new_note,
                        &prepared,
                        &ratified_event_id,
                        super::super::store::NoteOrigin::Ratified,
                    )
                    .await?;

                let updated = sqlx::query(
                    "UPDATE memory_pending SET state = 'ratified' \
                     WHERE proposal_id = ? AND state = 'proposed'",
                )
                .bind(&d.proposal_id)
                .execute(&mut *tx)
                .await?;
                if updated.rows_affected() == 0 {
                    // Another ratify won the race; drop(tx) rolls back.
                    return Err(MemoryError::AlreadyRatified(d.proposal_id));
                }

                // Step 3: record initial witnesses inside the same tx so
                // note + provenance + witnesses are either all committed or
                // all rolled back (F-012 fix). The ? propagates errors;
                // the Recorded enum return value is intentionally discarded.
                witnesses::record_in_tx(
                    &mut tx,
                    &note_id,
                    WitnessKind::WorkerProposed,
                    &ratified_event_id,
                )
                .await?;
                // Malformed JSON means we cannot enumerate the
                // proposal's sources. The threat model treats an
                // unknown provenance as at least as risky as a known
                // untrusted source, so we record the negative witness
                // rather than silently downgrading to empty. The
                // proposal's confidence score absorbs the penalty
                // instead of the kernel accepting an unaudited input.
                let parse_result: Result<Vec<String>, _> = serde_json::from_str(&derived_from_json);
                let derived_from_untrusted = match &parse_result {
                    Ok(v) => v.iter().any(|s| is_untrusted_source(s)),
                    Err(err) => {
                        tracing::error!(
                            "vigla: malformed derived_from JSON on proposal {}: {} \
                             (raw={:?}); recording DerivedFromUntrustedFile witness",
                            d.proposal_id,
                            err,
                            derived_from_json
                        );
                        true
                    }
                };
                if derived_from_untrusted {
                    witnesses::record_in_tx(
                        &mut tx,
                        &note_id,
                        WitnessKind::DerivedFromUntrustedFile,
                        &ratified_event_id,
                    )
                    .await?;
                }

                tx.commit().await?;

                Ok(RatifyOutcome::Accepted {
                    proposal_id: d.proposal_id,
                    note_id,
                })
            }
            RatificationDecision::Reject { reason } => {
                let now = crate::ids::rfc3339_now();
                let event_id = ids::new_memory_event_id();
                let payload = MemoryRejected {
                    proposal_id: d.proposal_id.clone(),
                    reason: reason.clone(),
                };
                let mut tx = self.pool.begin().await?;
                let updated = sqlx::query(
                    "UPDATE memory_pending SET state = 'rejected' \
                     WHERE proposal_id = ? AND state = 'proposed'",
                )
                .bind(&d.proposal_id)
                .execute(&mut *tx)
                .await?;
                if updated.rows_affected() == 0 {
                    // Another ratify (accept or reject) won the race; drop(tx) rolls back.
                    return Err(MemoryError::AlreadyRatified(d.proposal_id));
                }
                sqlx::query(
                    "INSERT INTO memory_events \
                     (event_id, mission_id, worker_id, ts, type, payload_json, schema_version) \
                     VALUES (?, ?, ?, ?, 'rejected', ?, ?)",
                )
                .bind(&event_id)
                .bind(&mission_id)
                .bind(&worker_id)
                .bind(&now)
                .bind(serde_json::to_string(&payload)?)
                .bind(MEMORY_SCHEMA_VERSION)
                .execute(&mut *tx)
                .await?;
                tx.commit().await?;
                Ok(RatifyOutcome::Rejected {
                    proposal_id: d.proposal_id,
                    reason,
                })
            }
        }
    }
}
