//! Pure router: worker [`MemoryIntent`] → [`MemoryKernel::on_proposal`].
//!
//! Adapter-level parsing lives in `adapter_core::memory_intent`;
//! kernel-level persistence + scanner + scoring lives in
//! `memory::kernel`. This module is the thin, side-effect-free bridge:
//! convert one typed intent (string `kind`, string `scope`) into the
//! kernel's typed [`ProposalInput`] and call `on_proposal`. No I/O of
//! its own, no error swallowing — the caller decides how to react.
//!
//! The split mirrors `memory_commands::pin_note_impl`: a pure async
//! function with no Tauri / sink coupling, easy to unit-test against
//! a real kernel without spinning up worker plumbing.

use std::sync::Arc;

use adapter_core::{MemoryIntent, ProposeIntent};

use super::error::MemoryError;
use super::hierarchy::{NoteKind, Scope, ScopeKind, StandardNoteKind};
use super::kernel::{MemoryKernel, ProposalInput, ProposalOutcome};

/// Apply a single intent against the kernel. Returns the kernel's
/// outcome so the caller can decide whether to log / surface to the
/// frontend / count metrics.
pub async fn route_intent(
    kernel: &Arc<MemoryKernel>,
    mission_id: &str,
    worker_id: &str,
    intent: MemoryIntent,
) -> Result<ProposalOutcome, MemoryError> {
    match intent {
        MemoryIntent::Propose(p) => route_propose(kernel, mission_id, worker_id, p).await,
    }
}

/// Convert a typed propose intent into a kernel `ProposalInput` and
/// call `on_proposal`. The conversion validates the string-typed
/// fields (`kind`, `scope.kind`) and returns a structured error for
/// unknown values — this is the wide gate at the IPC boundary that
/// keeps the kernel from ever seeing malformed taxonomy strings.
pub async fn route_propose(
    kernel: &Arc<MemoryKernel>,
    mission_id: &str,
    worker_id: &str,
    intent: ProposeIntent,
) -> Result<ProposalOutcome, MemoryError> {
    let input = ProposalInput {
        mission_id: mission_id.to_owned(),
        worker_id: worker_id.to_owned(),
        kind: kind_from_string(&intent.kind)?,
        scope: scope_from_intent(&intent.scope.kind, intent.scope.value)?,
        body: intent.body,
        derived_from: intent.derived_from,
        evidence_event_ids: intent.evidence_event_ids,
    };
    kernel.on_proposal(input).await
}

fn kind_from_string(s: &str) -> Result<NoteKind, MemoryError> {
    Ok(NoteKind::Standard(match s {
        "fact" => StandardNoteKind::Fact,
        "decision" => StandardNoteKind::Decision,
        "procedure" => StandardNoteKind::Procedure,
        "hazard" => StandardNoteKind::Hazard,
        other => {
            return Err(MemoryError::UnknownTaxonomy {
                category: "kind".into(),
                name: other.to_owned(),
            })
        }
    }))
}

fn scope_from_intent(kind: &str, value: Option<String>) -> Result<Scope, MemoryError> {
    let scope_kind = match kind {
        "repo" => ScopeKind::Repo,
        "path" => ScopeKind::Path,
        "vendor" => ScopeKind::Vendor,
        "supervisor" => ScopeKind::Supervisor,
        "worker" => ScopeKind::Worker,
        other => {
            return Err(MemoryError::UnknownTaxonomy {
                category: "scope_kind".into(),
                name: other.to_owned(),
            })
        }
    };
    Ok(Scope {
        kind: scope_kind,
        value: value.filter(|v| !v.is_empty()),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use adapter_core::ScopeIntent;
    use sqlx::sqlite::{SqliteConnectOptions, SqlitePoolOptions};
    use std::str::FromStr;
    use tempfile::TempDir;

    async fn fresh_kernel() -> (Arc<MemoryKernel>, TempDir) {
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
        let kernel = MemoryKernel::open(pool, dir.path().to_path_buf())
            .await
            .unwrap();
        (Arc::new(kernel), dir)
    }

    fn propose(kind: &str, body: &str) -> MemoryIntent {
        MemoryIntent::Propose(ProposeIntent {
            kind: kind.into(),
            scope: ScopeIntent {
                kind: "repo".into(),
                value: None,
            },
            body: body.into(),
            derived_from: vec!["worktree:src/x.rs:42".into()],
            evidence_event_ids: vec![],
        })
    }

    #[tokio::test]
    async fn routes_well_formed_propose_into_pending_state() {
        let (kernel, _dir) = fresh_kernel().await;
        let outcome = route_intent(&kernel, "m1", "w1", propose("hazard", "x"))
            .await
            .unwrap();
        assert!(matches!(outcome, ProposalOutcome::Accepted { .. }));
        let (pending,): (i64,) = sqlx::query_as(
            "SELECT COUNT(*) FROM memory_pending WHERE mission_id = ? AND worker_id = ?",
        )
        .bind("m1")
        .bind("w1")
        .fetch_one(kernel.pool())
        .await
        .unwrap();
        assert_eq!(pending, 1);
    }

    #[tokio::test]
    async fn unknown_kind_is_rejected_at_router() {
        let (kernel, _dir) = fresh_kernel().await;
        let err = route_intent(&kernel, "m1", "w1", propose("lesson", "x"))
            .await
            .unwrap_err();
        assert!(matches!(
            err,
            MemoryError::UnknownTaxonomy { ref category, .. } if category == "kind"
        ));
        // No pending row was written — the kernel never saw the call.
        let (pending,): (i64,) = sqlx::query_as("SELECT COUNT(*) FROM memory_pending")
            .fetch_one(kernel.pool())
            .await
            .unwrap();
        assert_eq!(pending, 0);
    }

    #[tokio::test]
    async fn unknown_scope_is_rejected_at_router() {
        let (kernel, _dir) = fresh_kernel().await;
        let intent = MemoryIntent::Propose(ProposeIntent {
            kind: "fact".into(),
            scope: ScopeIntent {
                kind: "galactic".into(),
                value: None,
            },
            body: "x".into(),
            derived_from: vec![],
            evidence_event_ids: vec![],
        });
        let err = route_intent(&kernel, "m1", "w1", intent).await.unwrap_err();
        assert!(matches!(
            err,
            MemoryError::UnknownTaxonomy { ref category, .. } if category == "scope_kind"
        ));
    }

    #[tokio::test]
    async fn router_propagates_kernel_scanner_rejection_as_accepted_outcome() {
        // The router's contract is to deliver the intent to the
        // kernel; the kernel itself rejects secret bodies and returns
        // ProposalOutcome::Rejected. The router does not pre-filter.
        let (kernel, _dir) = fresh_kernel().await;
        let intent = MemoryIntent::Propose(ProposeIntent {
            kind: "fact".into(),
            scope: ScopeIntent {
                kind: "repo".into(),
                value: None,
            },
            body: "Deploy AKIAIOSFODNN7EXAMPLE in env.".into(),
            derived_from: vec![],
            evidence_event_ids: vec![],
        });
        let outcome = route_intent(&kernel, "m1", "w1", intent).await.unwrap();
        assert!(matches!(outcome, ProposalOutcome::Rejected { .. }));
    }

    #[tokio::test]
    async fn empty_scope_value_string_normalized_to_none() {
        let (kernel, _dir) = fresh_kernel().await;
        let intent = MemoryIntent::Propose(ProposeIntent {
            kind: "fact".into(),
            scope: ScopeIntent {
                kind: "repo".into(),
                value: Some("".into()),
            },
            body: "y".into(),
            derived_from: vec![],
            evidence_event_ids: vec![],
        });
        // Should not error — empty string → None for scope.value.
        let outcome = route_intent(&kernel, "m1", "w1", intent).await.unwrap();
        assert!(matches!(outcome, ProposalOutcome::Accepted { .. }));
    }
}
