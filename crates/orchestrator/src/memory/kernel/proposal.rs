//! Worker memory-proposal handler.

use event_schema::memory::{
    MemoryProposalRejected, MemoryProposed, ProposalRejectReason, MEMORY_SCHEMA_VERSION,
};

use super::super::error::MemoryError;
use super::super::hierarchy::NOTE_BODY_CAP_BYTES;
use super::super::ids;
use super::super::scanner;
use super::types::{ProposalInput, ProposalOutcome};
use super::MemoryKernel;

impl MemoryKernel {
    /// Accept a worker's structured memory proposal. Runs scanner +
    /// size check; on rejection emits `MemoryProposalRejected` (the
    /// raw body is *not* persisted). On acceptance, inserts a row in
    /// `memory_pending` with state=proposed and emits `MemoryProposed`.
    /// Returns the proposal_id for the supervisor to reference.
    pub async fn on_proposal(&self, input: ProposalInput) -> Result<ProposalOutcome, MemoryError> {
        let proposal_id = ids::new_proposal_id();
        let now = crate::ids::rfc3339_now();

        // Secret scanner runs *first* — before the size check — so an
        // oversize body can never reach the raw-truncation preview below
        // with a secret still in it. `redact_preview` only truncates; it
        // does NOT redact, so scanning must gate the oversize path too.
        // Mirrors the ordering in `pin.rs`. On match, the raw body is
        // dropped — only the redacted preview lands in the event store.
        let scan = scanner::scan(&input.body);
        if let scanner::ScanResult::Match { redacted, .. } = &scan {
            let event_id = ids::new_memory_event_id();
            let payload = MemoryProposalRejected {
                proposal_id: proposal_id.clone(),
                reason: ProposalRejectReason::Secret,
                redacted_preview: redacted.clone(),
            };
            sqlx::query(
                "INSERT INTO memory_events \
                 (event_id, mission_id, worker_id, ts, type, payload_json, schema_version) \
                 VALUES (?, ?, ?, ?, 'proposal_rejected', ?, ?)",
            )
            .bind(&event_id)
            .bind(&input.mission_id)
            .bind(&input.worker_id)
            .bind(&now)
            .bind(serde_json::to_string(&payload)?)
            .bind(MEMORY_SCHEMA_VERSION)
            .execute(&self.pool)
            .await?;
            return Ok(ProposalOutcome::Rejected {
                proposal_id,
                reason: ProposalRejectReason::Secret,
            });
        }

        // Body size check. The scanner above already cleared the body,
        // so the truncated preview here is guaranteed secret-free.
        if input.body.len() > NOTE_BODY_CAP_BYTES {
            let event_id = ids::new_memory_event_id();
            let preview = scanner::redact_preview(&input.body, 160);
            let payload = MemoryProposalRejected {
                proposal_id: proposal_id.clone(),
                reason: ProposalRejectReason::Oversize,
                redacted_preview: preview,
            };
            sqlx::query(
                "INSERT INTO memory_events \
                 (event_id, mission_id, worker_id, ts, type, payload_json, schema_version) \
                 VALUES (?, ?, ?, ?, 'proposal_rejected', ?, ?)",
            )
            .bind(&event_id)
            .bind(&input.mission_id)
            .bind(&input.worker_id)
            .bind(&now)
            .bind(serde_json::to_string(&payload)?)
            .bind(MEMORY_SCHEMA_VERSION)
            .execute(&self.pool)
            .await?;
            return Ok(ProposalOutcome::Rejected {
                proposal_id,
                reason: ProposalRejectReason::Oversize,
            });
        }

        // Accepted into pending. Tx wraps row + event so a crash leaves
        // neither half.
        let event_id = ids::new_memory_event_id();
        let derived_from_json = serde_json::to_string(&input.derived_from)?;
        let evidence_json = serde_json::to_string(&input.evidence_event_ids)?;
        let proposed_payload = MemoryProposed {
            proposal_id: proposal_id.clone(),
            kind: input.kind.clone(),
            scope: input.scope.clone(),
            body: input.body.clone(),
            derived_from: input.derived_from.clone(),
            evidence_event_ids: input.evidence_event_ids.clone(),
        };
        let mut tx = self.pool.begin().await?;
        sqlx::query(
            "INSERT INTO memory_pending \
             (proposal_id, mission_id, worker_id, kind, scope_kind, scope_value, body, \
              derived_from, evidence, state, created_event_id, created_at) \
             VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, 'proposed', ?, ?)",
        )
        .bind(&proposal_id)
        .bind(&input.mission_id)
        .bind(&input.worker_id)
        .bind(input.kind.as_str())
        .bind(input.scope.kind.as_str())
        .bind(input.scope.value.as_deref())
        .bind(&input.body)
        .bind(&derived_from_json)
        .bind(&evidence_json)
        .bind(&event_id)
        .bind(&now)
        .execute(&mut *tx)
        .await?;
        sqlx::query(
            "INSERT INTO memory_events \
             (event_id, mission_id, worker_id, ts, type, payload_json, schema_version) \
             VALUES (?, ?, ?, ?, 'proposed', ?, ?)",
        )
        .bind(&event_id)
        .bind(&input.mission_id)
        .bind(&input.worker_id)
        .bind(&now)
        .bind(serde_json::to_string(&proposed_payload)?)
        .bind(MEMORY_SCHEMA_VERSION)
        .execute(&mut *tx)
        .await?;
        tx.commit().await?;

        Ok(ProposalOutcome::Accepted { proposal_id })
    }
}
