//! User-oracle pin handler.

use event_schema::memory::{MemoryProposalRejected, ProposalRejectReason, MEMORY_SCHEMA_VERSION};

use super::super::error::MemoryError;
use super::super::hierarchy::{NoteAuthor, NOTE_BODY_CAP_BYTES};
use super::super::ids;
use super::super::reflection;
use super::super::scanner;
use super::super::store::NewNote;
use super::types::{PinInput, PinOutcome};
use super::MemoryKernel;

impl MemoryKernel {
    /// User-oracle pin (Tier-1 surface). Runs the secret scanner,
    /// adds the note via the store's user path (which records a
    /// `UserAuthored` witness), then attempts immediate promotion via
    /// the policy. The user is the highest-confidence signal we have;
    /// the policy shortcut promotes user-authored notes the moment
    /// they clear the user bar (V3 §9).
    ///
    /// Returns `Pinned { note_id, promoted }`. A `promoted: false` is
    /// expected when scope-value rules or conflict links hold back
    /// promotion — the note still exists in `state=owned`.
    pub async fn pin_note(&self, input: PinInput) -> Result<PinOutcome, MemoryError> {
        // Secret scanner runs *before* anything persistent happens.
        // A match means we emit MemoryProposalRejected with a redacted
        // preview and never let the body near the store.
        let scan = scanner::scan(&input.body);
        if let scanner::ScanResult::Match { redacted, .. } = &scan {
            let pseudo_proposal_id = ids::new_proposal_id();
            let event_id = ids::new_memory_event_id();
            let now = crate::ids::rfc3339_now();
            let payload = MemoryProposalRejected {
                proposal_id: pseudo_proposal_id.clone(),
                reason: ProposalRejectReason::Secret,
                redacted_preview: redacted.clone(),
            };
            sqlx::query(
                "INSERT INTO memory_events \
                 (event_id, mission_id, worker_id, ts, type, payload_json, schema_version) \
                 VALUES (?, NULL, NULL, ?, 'proposal_rejected', ?, ?)",
            )
            .bind(&event_id)
            .bind(&now)
            .bind(serde_json::to_string(&payload)?)
            .bind(MEMORY_SCHEMA_VERSION)
            .execute(&self.pool)
            .await?;
            return Ok(PinOutcome::Rejected {
                reason: ProposalRejectReason::Secret,
                redacted_preview: redacted.clone(),
            });
        }
        if input.body.len() > NOTE_BODY_CAP_BYTES {
            let pseudo_proposal_id = ids::new_proposal_id();
            let event_id = ids::new_memory_event_id();
            let now = crate::ids::rfc3339_now();
            let preview = scanner::redact_preview(&input.body, 160);
            let payload = MemoryProposalRejected {
                proposal_id: pseudo_proposal_id.clone(),
                reason: ProposalRejectReason::Oversize,
                redacted_preview: preview.clone(),
            };
            sqlx::query(
                "INSERT INTO memory_events \
                 (event_id, mission_id, worker_id, ts, type, payload_json, schema_version) \
                 VALUES (?, NULL, NULL, ?, 'proposal_rejected', ?, ?)",
            )
            .bind(&event_id)
            .bind(&now)
            .bind(serde_json::to_string(&payload)?)
            .bind(MEMORY_SCHEMA_VERSION)
            .execute(&self.pool)
            .await?;
            return Ok(PinOutcome::Rejected {
                reason: ProposalRejectReason::Oversize,
                redacted_preview: preview,
            });
        }

        // Add the note via the user-authored path; this records the
        // UserAuthored witness automatically.
        let note_id = self
            .store
            .note_add(
                NewNote {
                    kind: input.kind,
                    scope: input.scope,
                    body: input.body,
                },
                NoteAuthor::User {
                    source: input.source,
                },
            )
            .await?;

        // Try immediate promotion. UserAuthored + fresh recency clears
        // the user-bar of 0.5 unless contradicted by a conflict link.
        let promotion_event_id = ids::new_memory_event_id();
        let promoted =
            reflection::try_promote(&self.pool, &self.store, &note_id, &promotion_event_id, None)
                .await?
                .is_some();

        // V1.2 (Phase 2 Task 11) — on-promote embedding hook.
        // Best-effort: a `false` from `embed_and_store` (embedder
        // disabled, empty body, encode returned None) is logged
        // inside the helper and the pin still reports success. The
        // backfill task on next `MemoryKernel::open` catches any
        // notes the live path missed.
        if promoted {
            if let Err(e) = self.embed_and_store(&note_id).await {
                tracing::warn!(
                    target: "memory.retrieval.embed",
                    error = %e,
                    note_id = %note_id,
                    "on-promote embedding failed; backfill will retry on next open"
                );
            }
        }

        Ok(PinOutcome::Pinned { note_id, promoted })
    }
}
