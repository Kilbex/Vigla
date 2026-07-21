//! Tauri command surface for the Memory Kernel.
//!
//! Commands shipped through Tier-2E:
//!
//!   * [`memory_pin_note`] (Tier-2C) — the user-oracle path. The
//!     frontend hands in a body + kind (+ optional scope value); the
//!     kernel runs the scanner, persists the note, records the
//!     `UserAuthored` witness, and (when policy allows) promotes it
//!     immediately. This is the "talking to Vigla teaches it"
//!     surface.
//!
//!   * [`memory_list_notes`] (Tier-2C) — read-only view of the
//!     codex used by the command-panel pin button to show what
//!     already exists (and what's promoted).
//!
//!   * [`memory_recent_events_for_mission`] (Tier-2E) — recent
//!     mission-scoped memory events for the receiving-surface drawer:
//!     proposals, ratifications, promotions, barriers, drift, etc.
//!     Returns a typed, UI-friendly DTO derived from the raw
//!     `memory_events` log without exposing the kernel's internal
//!     wire types.
//!
//!   * [`memory_latest_bundle_for_mission`] (Tier-2E) — the most
//!     recently composed bundle for a mission, so the drawer can show
//!     "what memory is currently attached" without needing to recompose.
//!
//! All commands are thin shells around pure async `*_impl` functions
//! that take an `Arc<MemoryKernel>`. The split lets `cargo test`
//! exercise the kernel side without spinning up Tauri state.

use std::sync::Arc;

use std::path::Path;

use orchestrator::memory::{
    ListFilter, MemoryBundleRow, MemoryError, MemoryEventRow, MemoryKernel, MemoryRegistry,
    NoteKind, NoteState, NoteSummary, PinInput, PinOutcome, ProposalRejectReason, Scope, ScopeKind,
    StandardNoteKind,
};
use serde::{Deserialize, Serialize};
use specta::Type;
use tauri::State;

// ---------------------------------------------------------------------
// Request / response DTOs
// ---------------------------------------------------------------------

/// User-controlled subset of note kinds. The kernel's
/// `NoteKind::Other(String)` variant is intentionally hidden from the
/// IPC surface — pinning a freshly-coined kind would require a
/// taxonomy migration first, and that's an admin-class operation, not
/// a "pin a note" one. Constraining the wire form to the four
/// standard kinds keeps the UI dropdown finite and the kernel happy.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Type)]
#[serde(rename_all = "snake_case")]
pub enum PinNoteKind {
    Fact,
    Decision,
    Procedure,
    Hazard,
}

impl PinNoteKind {
    fn into_kernel(self) -> NoteKind {
        NoteKind::Standard(match self {
            PinNoteKind::Fact => StandardNoteKind::Fact,
            PinNoteKind::Decision => StandardNoteKind::Decision,
            PinNoteKind::Procedure => StandardNoteKind::Procedure,
            PinNoteKind::Hazard => StandardNoteKind::Hazard,
        })
    }
}

/// User-controlled scope. `repo` is the default; `vendor` requires a
/// vendor name. `path`/`supervisor`/`worker` scopes need a value
/// supplied by the frontend.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Type)]
#[serde(rename_all = "snake_case")]
pub enum PinNoteScopeKind {
    Repo,
    Vendor,
    Path,
}

impl PinNoteScopeKind {
    fn into_kernel(self) -> ScopeKind {
        match self {
            PinNoteScopeKind::Repo => ScopeKind::Repo,
            PinNoteScopeKind::Vendor => ScopeKind::Vendor,
            PinNoteScopeKind::Path => ScopeKind::Path,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, Type)]
pub struct PinNoteRequest {
    pub kind: PinNoteKind,
    pub scope_kind: PinNoteScopeKind,
    /// Required for any scope other than `repo`. Validated by the
    /// kernel; the IPC surface only forwards it.
    #[serde(default)]
    pub scope_value: Option<String>,
    pub body: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, Type)]
#[serde(tag = "outcome", rename_all = "snake_case")]
pub enum PinNoteResponse {
    /// Note created. `promoted == true` when the policy shortcut for
    /// `UserAuthored` witnesses cleared the bar (single-pin learning
    /// loop succeeded).
    Pinned { note_id: String, promoted: bool },
    /// Scanner / oversize check rejected the body. No note created;
    /// only the redacted preview survives in the event log.
    Rejected {
        reason: PinNoteRejectReason,
        redacted_preview: String,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Type)]
#[serde(rename_all = "snake_case")]
pub enum PinNoteRejectReason {
    Secret,
    Oversize,
    Malformed,
}

impl From<ProposalRejectReason> for PinNoteRejectReason {
    fn from(r: ProposalRejectReason) -> Self {
        match r {
            ProposalRejectReason::Secret => PinNoteRejectReason::Secret,
            ProposalRejectReason::Oversize => PinNoteRejectReason::Oversize,
            ProposalRejectReason::Malformed => PinNoteRejectReason::Malformed,
        }
    }
}

/// Lightweight summary returned by [`memory_list_notes`]. Bodies are
/// omitted on purpose — the list endpoint should stay cheap; the
/// drawer in Tier-2E will introduce a separate "show note" command
/// when the body is actually needed.
#[derive(Debug, Clone, Serialize, Deserialize, Type)]
pub struct MemoryNoteSummaryDto {
    pub id: String,
    pub kind: String,
    pub scope_kind: String,
    pub scope_value: Option<String>,
    pub state: String,
    pub created_at: String,
}

impl From<NoteSummary> for MemoryNoteSummaryDto {
    fn from(n: NoteSummary) -> Self {
        Self {
            id: n.id,
            kind: n.kind.as_str().to_owned(),
            scope_kind: n.scope.kind.as_str().to_owned(),
            scope_value: n.scope.value,
            state: n.state.as_str().to_owned(),
            created_at: n.created_at,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Type)]
#[serde(rename_all = "snake_case")]
pub enum NoteStateFilter {
    Owned,
    Promoted,
    Disputed,
    Invalid,
}

impl NoteStateFilter {
    fn into_kernel(self) -> NoteState {
        match self {
            NoteStateFilter::Owned => NoteState::Owned,
            NoteStateFilter::Promoted => NoteState::Promoted,
            NoteStateFilter::Disputed => NoteState::Disputed,
            NoteStateFilter::Invalid => NoteState::Invalid,
        }
    }
}

// ---------------------------------------------------------------------
// Pure async impls — tested directly without Tauri state
// ---------------------------------------------------------------------

pub(crate) async fn pin_note_impl(
    kernel: &Arc<MemoryKernel>,
    request: PinNoteRequest,
) -> Result<PinNoteResponse, String> {
    let input = PinInput {
        kind: request.kind.into_kernel(),
        scope: Scope {
            kind: request.scope_kind.into_kernel(),
            value: request.scope_value.filter(|v| !v.is_empty()),
        },
        body: request.body,
        // Pinning over IPC is the "Pin as note" command-panel surface,
        // which fits the `UiPin` author source. CLI-typed pins (a
        // future `vigla note add` invocation) would use `Cli`.
        source: orchestrator::memory::AuthorSource::UiPin,
    };
    let outcome = kernel
        .pin_note(input)
        .await
        .map_err(memory_error_to_string)?;
    Ok(match outcome {
        PinOutcome::Pinned { note_id, promoted } => PinNoteResponse::Pinned { note_id, promoted },
        PinOutcome::Rejected {
            reason,
            redacted_preview,
        } => PinNoteResponse::Rejected {
            reason: reason.into(),
            redacted_preview,
        },
    })
}

pub(crate) async fn list_notes_impl(
    kernel: &Arc<MemoryKernel>,
    state_filter: Option<NoteStateFilter>,
    limit: usize,
) -> Result<Vec<MemoryNoteSummaryDto>, String> {
    let filter = ListFilter {
        state: state_filter.map(NoteStateFilter::into_kernel),
        ..Default::default()
    };
    let mut notes = kernel
        .store
        .note_list(filter)
        .await
        .map_err(memory_error_to_string)?;
    notes.truncate(limit);
    Ok(notes.into_iter().map(MemoryNoteSummaryDto::from).collect())
}

/// Conversion that strips internal types out of the wire error — the
/// kernel error variants carry SQL / I/O detail that's noisy for the
/// UI. The user-facing form is "memory backend error: <message>".
fn memory_error_to_string(e: MemoryError) -> String {
    format!("memory backend error: {e}")
}

/// A2 helper: resolve the per-repo [`MemoryKernel`] from the wire
/// `cwd` parameter. Empty `cwd` is rejected up-front so the frontend
/// surfaces a meaningful message instead of falling through to a
/// canonicalize-of-empty-path I/O error.
async fn resolve_kernel(registry: &MemoryRegistry, cwd: &str) -> Result<Arc<MemoryKernel>, String> {
    if cwd.is_empty() {
        return Err("no active repository — start or select a mission to attach memory".into());
    }
    registry
        .get_or_open(Path::new(cwd))
        .await
        .map_err(memory_error_to_string)
}

// ---------------------------------------------------------------------
// Tauri command shells
// ---------------------------------------------------------------------

#[tauri::command]
#[specta::specta]
pub async fn memory_pin_note(
    cwd: String,
    request: PinNoteRequest,
    runtime: State<'_, crate::RuntimeHandle>,
) -> Result<PinNoteResponse, String> {
    let registry = runtime.ready()?.memory_registry.clone();
    let kernel = resolve_kernel(&registry, &cwd).await?;
    pin_note_impl(&kernel, request).await
}

#[tauri::command]
#[specta::specta]
pub async fn memory_list_notes(
    cwd: String,
    state_filter: Option<NoteStateFilter>,
    limit: Option<u32>,
    runtime: State<'_, crate::RuntimeHandle>,
) -> Result<Vec<MemoryNoteSummaryDto>, String> {
    let cap = limit.unwrap_or(50).clamp(1, 500) as usize;
    let registry = runtime.ready()?.memory_registry.clone();
    let kernel = resolve_kernel(&registry, &cwd).await?;
    list_notes_impl(&kernel, state_filter, cap).await
}

// ---------------------------------------------------------------------
// Tier-2E — recent memory events for a mission + latest bundle
// ---------------------------------------------------------------------

/// UI-friendly memory event projection. Each variant carries only
/// the fields the drawer needs; the raw payload stays in
/// `memory_events.payload_json` for anyone who needs more.
#[derive(Debug, Clone, Serialize, Deserialize, Type)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum MemoryEventKindDto {
    Proposed {
        proposal_id: String,
        kind: String,
        body_preview: String,
    },
    ProposalRejected {
        proposal_id: String,
        reason: String,
    },
    Normalized {
        proposal_id: String,
    },
    Ratified {
        proposal_id: String,
        note_id: Option<String>,
        decision: String,
    },
    Rejected {
        proposal_id: String,
        reason: String,
    },
    Promoted {
        note_id: String,
        confidence: f64,
    },
    Barrier {
        kind: String,
    },
    BundleComposed {
        bundle_id: String,
        worker_id: String,
        turn: u32,
        note_count: u32,
    },
    BundleRendered {
        bundle_id: String,
    },
    DriftDetected {
        bundle_id: String,
    },
    /// Catch-all for events the UI doesn't render specifically.
    /// Carries the wire type name so a future UI build can opt in
    /// without a new server round-trip.
    Other {
        event_type: String,
    },
}

#[derive(Debug, Clone, Serialize, Deserialize, Type)]
pub struct MemoryEventDto {
    pub event_id: String,
    pub mission_id: Option<String>,
    pub worker_id: Option<String>,
    pub ts: String,
    pub kind: MemoryEventKindDto,
}

#[derive(Debug, Clone, Serialize, Deserialize, Type)]
pub struct MemoryBundleDto {
    pub bundle_id: String,
    pub mission_id: String,
    pub worker_id: String,
    pub turn: u32,
    pub vendor: String,
    pub note_ids: Vec<String>,
}

/// Length cap on body previews returned in `Proposed` events. The
/// raw body lives in `memory_pending` (kernel side); the preview is
/// for at-a-glance UI rendering. Multi-byte safe — we count chars,
/// not bytes, and append a `…` on truncation.
const BODY_PREVIEW_CHARS: usize = 120;

pub(crate) async fn recent_events_impl(
    kernel: &Arc<MemoryKernel>,
    mission_id: &str,
    limit: usize,
) -> Result<Vec<MemoryEventDto>, String> {
    let rows = kernel
        .recent_events_for_mission(mission_id, limit)
        .await
        .map_err(memory_error_to_string)?;
    Ok(rows.into_iter().map(memory_event_row_to_dto).collect())
}

pub(crate) async fn latest_bundle_impl(
    kernel: &Arc<MemoryKernel>,
    mission_id: &str,
) -> Result<Option<MemoryBundleDto>, String> {
    let row = kernel
        .latest_bundle_for_mission(mission_id)
        .await
        .map_err(memory_error_to_string)?;
    Ok(row.map(memory_bundle_row_to_dto))
}

fn memory_event_row_to_dto(row: MemoryEventRow) -> MemoryEventDto {
    let kind = project_event_kind(&row.event_type, &row.payload_json);
    MemoryEventDto {
        event_id: row.event_id,
        mission_id: row.mission_id,
        worker_id: row.worker_id,
        ts: row.ts,
        kind,
    }
}

fn memory_bundle_row_to_dto(row: MemoryBundleRow) -> MemoryBundleDto {
    let note_ids: Vec<String> =
        serde_json::from_str::<Vec<serde_json::Value>>(&row.page_table_json)
            .unwrap_or_default()
            .into_iter()
            .filter_map(|v| v.get("note_id").and_then(|s| s.as_str()).map(str::to_owned))
            .collect();
    MemoryBundleDto {
        bundle_id: row.bundle_id,
        mission_id: row.mission_id,
        worker_id: row.worker_id,
        turn: row.turn,
        vendor: row.vendor,
        note_ids,
    }
}

/// Partial-parse a raw `memory_events.payload_json` row into the
/// UI's typed projection. Unknown types route through
/// `MemoryEventKindDto::Other` so future kernel-side event variants
/// surface in the drawer without a UI build.
///
/// All parsing is tolerant: missing fields default sanely (empty
/// strings, `0`, `None`). The kernel writes well-formed payloads —
/// this guards against drift between the kernel and the host.
fn project_event_kind(event_type: &str, payload_json: &str) -> MemoryEventKindDto {
    let v: serde_json::Value =
        serde_json::from_str(payload_json).unwrap_or(serde_json::Value::Null);
    let s = |key: &str| -> String {
        v.get(key)
            .and_then(|x| x.as_str())
            .unwrap_or_default()
            .to_owned()
    };
    let opt_s =
        |key: &str| -> Option<String> { v.get(key).and_then(|x| x.as_str()).map(str::to_owned) };

    match event_type {
        "proposed" => MemoryEventKindDto::Proposed {
            proposal_id: s("proposal_id"),
            kind: s("kind"),
            body_preview: truncate_chars(&s("body"), BODY_PREVIEW_CHARS),
        },
        "proposal_rejected" => MemoryEventKindDto::ProposalRejected {
            proposal_id: s("proposal_id"),
            reason: s("reason"),
        },
        "normalized" => MemoryEventKindDto::Normalized {
            proposal_id: s("proposal_id"),
        },
        "ratified" => MemoryEventKindDto::Ratified {
            proposal_id: s("proposal_id"),
            note_id: opt_s("note_id"),
            decision: s("decision"),
        },
        "rejected" => MemoryEventKindDto::Rejected {
            proposal_id: s("proposal_id"),
            reason: s("reason"),
        },
        "promoted" => MemoryEventKindDto::Promoted {
            note_id: s("note_id"),
            confidence: v.get("confidence").and_then(|x| x.as_f64()).unwrap_or(0.0),
        },
        "barrier" => MemoryEventKindDto::Barrier { kind: s("kind") },
        "bundle_composed" => {
            let note_count = v
                .get("page_table")
                .and_then(|x| x.as_array())
                .map(|a| a.len())
                .unwrap_or(0);
            MemoryEventKindDto::BundleComposed {
                bundle_id: s("bundle_id"),
                worker_id: s("worker_id"),
                turn: v.get("turn").and_then(|x| x.as_u64()).unwrap_or(0) as u32,
                note_count: note_count as u32,
            }
        }
        "bundle_rendered" => MemoryEventKindDto::BundleRendered {
            bundle_id: s("bundle_id"),
        },
        "drift_detected" => MemoryEventKindDto::DriftDetected {
            bundle_id: s("bundle_id"),
        },
        other => MemoryEventKindDto::Other {
            event_type: other.to_owned(),
        },
    }
}

/// Cap a string at `max` characters (not bytes). Appends `…` on
/// truncation. Empty input returns empty — never panics, never
/// allocates on the happy path.
fn truncate_chars(s: &str, max: usize) -> String {
    if s.chars().count() <= max {
        return s.to_owned();
    }
    let mut out: String = s.chars().take(max).collect();
    out.push('…');
    out
}

#[tauri::command]
#[specta::specta]
pub async fn memory_recent_events_for_mission(
    cwd: String,
    mission_id: String,
    limit: Option<u32>,
    runtime: State<'_, crate::RuntimeHandle>,
) -> Result<Vec<MemoryEventDto>, String> {
    let cap = limit.unwrap_or(100).clamp(1, 500) as usize;
    let registry = runtime.ready()?.memory_registry.clone();
    let kernel = resolve_kernel(&registry, &cwd).await?;
    recent_events_impl(&kernel, &mission_id, cap).await
}

#[tauri::command]
#[specta::specta]
pub async fn memory_latest_bundle_for_mission(
    cwd: String,
    mission_id: String,
    runtime: State<'_, crate::RuntimeHandle>,
) -> Result<Option<MemoryBundleDto>, String> {
    let registry = runtime.ready()?.memory_registry.clone();
    let kernel = resolve_kernel(&registry, &cwd).await?;
    latest_bundle_impl(&kernel, &mission_id).await
}

// ---------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
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
        sqlx::migrate!("../../crates/orchestrator/migrations")
            .run(&pool)
            .await
            .unwrap();
        let dir = TempDir::new().unwrap();
        let kernel = MemoryKernel::open(pool, dir.path().to_path_buf())
            .await
            .unwrap();
        (Arc::new(kernel), dir)
    }

    fn req(kind: PinNoteKind, body: &str) -> PinNoteRequest {
        PinNoteRequest {
            kind,
            scope_kind: PinNoteScopeKind::Repo,
            scope_value: None,
            body: body.into(),
        }
    }

    #[tokio::test]
    async fn pin_note_promotes_user_authored_immediately() {
        let (kernel, _dir) = fresh_kernel().await;
        let response = pin_note_impl(
            &kernel,
            req(
                PinNoteKind::Hazard,
                "Always run `cargo build --workspace` before commits.",
            ),
        )
        .await
        .unwrap();
        match response {
            PinNoteResponse::Pinned { note_id, promoted } => {
                assert!(promoted, "user-authored hazard must promote on pin");
                assert!(!note_id.is_empty());
            }
            other => panic!("expected Pinned, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn pin_note_rejects_aws_key_with_redacted_preview() {
        let (kernel, _dir) = fresh_kernel().await;
        let response = pin_note_impl(
            &kernel,
            req(
                PinNoteKind::Fact,
                "Deploy with AKIAIOSFODNN7EXAMPLE in env.",
            ),
        )
        .await
        .unwrap();
        match response {
            PinNoteResponse::Rejected {
                reason: PinNoteRejectReason::Secret,
                redacted_preview,
            } => {
                assert!(!redacted_preview.contains("AKIAIOSFODNN7EXAMPLE"));
                assert!(redacted_preview.contains("[REDACTED:"));
            }
            other => panic!("expected Rejected(Secret), got {other:?}"),
        }
    }

    #[tokio::test]
    async fn pin_note_oversize_body_rejected_without_storing_body() {
        let (kernel, _dir) = fresh_kernel().await;
        let huge = "x".repeat(orchestrator::memory::NOTE_BODY_CAP_BYTES + 1);
        let response = pin_note_impl(&kernel, req(PinNoteKind::Fact, &huge))
            .await
            .unwrap();
        assert!(matches!(
            response,
            PinNoteResponse::Rejected {
                reason: PinNoteRejectReason::Oversize,
                ..
            }
        ));
    }

    #[tokio::test]
    async fn list_notes_returns_pinned_note() {
        let (kernel, _dir) = fresh_kernel().await;
        let _ = pin_note_impl(
            &kernel,
            req(PinNoteKind::Decision, "Release branches cut from main."),
        )
        .await
        .unwrap();
        let notes = list_notes_impl(&kernel, None, 10).await.unwrap();
        assert_eq!(notes.len(), 1);
        assert_eq!(notes[0].kind, "decision");
        assert_eq!(notes[0].state, "promoted");
    }

    #[tokio::test]
    async fn list_notes_filters_by_state() {
        let (kernel, _dir) = fresh_kernel().await;
        // Pin promotes immediately, so all notes will be in `promoted`.
        let _ = pin_note_impl(&kernel, req(PinNoteKind::Hazard, "x"))
            .await
            .unwrap();
        let owned = list_notes_impl(&kernel, Some(NoteStateFilter::Owned), 10)
            .await
            .unwrap();
        assert!(owned.is_empty());
        let promoted = list_notes_impl(&kernel, Some(NoteStateFilter::Promoted), 10)
            .await
            .unwrap();
        assert_eq!(promoted.len(), 1);
    }

    #[tokio::test]
    async fn list_notes_respects_limit() {
        let (kernel, _dir) = fresh_kernel().await;
        for i in 0..5 {
            let _ = pin_note_impl(&kernel, req(PinNoteKind::Fact, &format!("note number {i}")))
                .await
                .unwrap();
        }
        let three = list_notes_impl(&kernel, None, 3).await.unwrap();
        assert_eq!(three.len(), 3);
    }

    #[tokio::test]
    async fn pin_note_scope_value_is_normalized_empty_to_none() {
        let (kernel, _dir) = fresh_kernel().await;
        let response = pin_note_impl(
            &kernel,
            PinNoteRequest {
                kind: PinNoteKind::Fact,
                scope_kind: PinNoteScopeKind::Repo,
                scope_value: Some("".into()), // empty string from JS should map to None
                body: "y".into(),
            },
        )
        .await
        .unwrap();
        assert!(matches!(response, PinNoteResponse::Pinned { .. }));
    }

    // -----------------------------------------------------------------
    // Tier-2E — recent_events_impl + latest_bundle_impl + projection
    // -----------------------------------------------------------------

    #[test]
    fn truncate_chars_handles_unicode_and_under_cap() {
        assert_eq!(truncate_chars("", 10), "");
        assert_eq!(truncate_chars("short", 10), "short");
        // 4-char unicode string, cap 3 → trimmed to 3 plus ellipsis.
        let out = truncate_chars("αβγδ", 3);
        assert_eq!(out.chars().count(), 4); // 3 kept + '…'
        assert!(out.ends_with('…'));
    }

    #[test]
    fn project_event_kind_maps_proposed_with_preview() {
        let payload = serde_json::json!({
            "proposal_id": "p1",
            "kind": "hazard",
            "scope": { "kind": "repo" },
            "body": "x".repeat(BODY_PREVIEW_CHARS + 50)
        })
        .to_string();
        let dto = project_event_kind("proposed", &payload);
        match dto {
            MemoryEventKindDto::Proposed {
                proposal_id,
                kind,
                body_preview,
            } => {
                assert_eq!(proposal_id, "p1");
                assert_eq!(kind, "hazard");
                assert!(body_preview.ends_with('…'));
                assert!(body_preview.chars().count() <= BODY_PREVIEW_CHARS + 1);
            }
            _ => panic!("expected Proposed"),
        }
    }

    #[test]
    fn project_event_kind_unknown_type_routes_to_other() {
        let dto = project_event_kind("future_type_we_dont_know", "{}");
        match dto {
            MemoryEventKindDto::Other { event_type } => {
                assert_eq!(event_type, "future_type_we_dont_know");
            }
            _ => panic!("expected Other"),
        }
    }

    #[test]
    fn project_event_kind_malformed_payload_recovers_to_defaults() {
        // Garbage JSON for a known event type → fields default to empty.
        let dto = project_event_kind("ratified", "{not valid json");
        match dto {
            MemoryEventKindDto::Ratified {
                proposal_id,
                note_id,
                decision,
            } => {
                assert_eq!(proposal_id, "");
                assert!(note_id.is_none());
                assert_eq!(decision, "");
            }
            _ => panic!("expected Ratified"),
        }
    }

    #[tokio::test]
    async fn recent_events_returns_proposal_after_worker_proposes() {
        let (kernel, _dir) = fresh_kernel().await;
        let input = orchestrator::memory::ProposalInput {
            mission_id: "mission-1".into(),
            worker_id: "worker-a".into(),
            kind: orchestrator::memory::NoteKind::Standard(
                orchestrator::memory::StandardNoteKind::Hazard,
            ),
            scope: orchestrator::memory::Scope {
                kind: orchestrator::memory::ScopeKind::Repo,
                value: None,
            },
            body: "Resume tokens are host-bound.".into(),
            derived_from: vec!["worktree:src/x.rs:42".into()],
            evidence_event_ids: vec![],
        };
        kernel.on_proposal(input).await.unwrap();

        let events = recent_events_impl(&kernel, "mission-1", 50).await.unwrap();
        assert!(events.iter().any(|e| matches!(
            &e.kind,
            MemoryEventKindDto::Proposed { kind, .. } if kind == "hazard"
        )));
        // Mission filter is exact — events for a different mission
        // don't leak in.
        let none = recent_events_impl(&kernel, "mission-other", 50)
            .await
            .unwrap();
        assert!(none.is_empty());
    }

    #[tokio::test]
    async fn recent_events_respects_limit_and_ts_desc_ordering() {
        let (kernel, _dir) = fresh_kernel().await;
        for i in 0..5 {
            let input = orchestrator::memory::ProposalInput {
                mission_id: "mission-1".into(),
                worker_id: format!("w-{i}"),
                kind: orchestrator::memory::NoteKind::Standard(
                    orchestrator::memory::StandardNoteKind::Fact,
                ),
                scope: orchestrator::memory::Scope {
                    kind: orchestrator::memory::ScopeKind::Repo,
                    value: None,
                },
                body: format!("note {i}"),
                derived_from: vec![],
                evidence_event_ids: vec![],
            };
            kernel.on_proposal(input).await.unwrap();
            // Ensure distinct timestamps so ORDER BY ts DESC has work.
            tokio::time::sleep(std::time::Duration::from_millis(2)).await;
        }
        let two = recent_events_impl(&kernel, "mission-1", 2).await.unwrap();
        assert_eq!(two.len(), 2);
        // Newest first — second event we wrote (i=4) should come before earlier.
        // Verify ordering by checking that ts is monotonically descending.
        for w in two.windows(2) {
            assert!(w[0].ts >= w[1].ts);
        }
    }

    #[tokio::test]
    async fn latest_bundle_returns_none_when_no_bundle_exists() {
        let (kernel, _dir) = fresh_kernel().await;
        assert!(latest_bundle_impl(&kernel, "no-such-mission")
            .await
            .unwrap()
            .is_none());
    }

    #[tokio::test]
    async fn latest_bundle_returns_highest_turn_for_mission() {
        let (kernel, _dir) = fresh_kernel().await;
        // Compose two bundles for the same mission at turns 0 and 1.
        let _ = kernel
            .pin_note(orchestrator::memory::PinInput {
                kind: orchestrator::memory::NoteKind::Standard(
                    orchestrator::memory::StandardNoteKind::Hazard,
                ),
                scope: orchestrator::memory::Scope {
                    kind: orchestrator::memory::ScopeKind::Repo,
                    value: None,
                },
                body: "y".into(),
                source: orchestrator::memory::AuthorSource::Cli,
            })
            .await
            .unwrap();
        let promoted = kernel
            .store
            .note_list(orchestrator::memory::ListFilter {
                state: Some(orchestrator::memory::NoteState::Promoted),
                ..Default::default()
            })
            .await
            .unwrap();
        let ids: Vec<String> = promoted.iter().map(|n| n.id.clone()).collect();
        let tempdir = tempfile::TempDir::new().unwrap();
        let adapter = orchestrator::memory::ClaudeMemoryAdapter;
        let brief_t0 = orchestrator::memory::BundleBrief {
            mission_id: "m1".into(),
            worker_id: "w1".into(),
            turn: 0,
            vendor: event_schema::Vendor::Claude,
        };
        let brief_t1 = orchestrator::memory::BundleBrief {
            mission_id: "m1".into(),
            worker_id: "w2".into(),
            turn: 1,
            vendor: event_schema::Vendor::Claude,
        };
        kernel
            .render_for_worker(&brief_t0, &adapter, &ids, tempdir.path())
            .await
            .unwrap();
        kernel
            .render_for_worker(&brief_t1, &adapter, &ids, tempdir.path())
            .await
            .unwrap();
        let latest = latest_bundle_impl(&kernel, "m1").await.unwrap().unwrap();
        assert_eq!(latest.turn, 1);
        assert!(!latest.note_ids.is_empty());
        assert_eq!(latest.vendor, "claude");
    }

    // -----------------------------------------------------------------
    // A2 — per-repo isolation through the registry
    // -----------------------------------------------------------------

    fn fresh_repo() -> tempfile::TempDir {
        let dir = tempfile::TempDir::new().unwrap();
        std::fs::create_dir_all(dir.path().join(".git")).unwrap();
        dir
    }

    /// Pinning a note in repo A must be invisible in repo B — the
    /// release-gate property A2 exists to enforce.
    #[tokio::test]
    async fn pinned_note_does_not_leak_across_repos_through_registry() {
        let registry = MemoryRegistry::new();
        let repo_a = fresh_repo();
        let repo_b = fresh_repo();

        // Pin into repo A via the registry-resolved kernel.
        let kernel_a = registry.get_or_open(repo_a.path()).await.unwrap();
        pin_note_impl(
            &kernel_a,
            PinNoteRequest {
                kind: PinNoteKind::Hazard,
                scope_kind: PinNoteScopeKind::Repo,
                scope_value: None,
                body: "repo-A-only hazard".into(),
            },
        )
        .await
        .unwrap();

        // Repo A sees the note.
        let in_a = list_notes_impl(&kernel_a, None, 10).await.unwrap();
        assert_eq!(in_a.len(), 1);

        // Open repo B's kernel — separate SQLite file, separate
        // store. The note from A must not be visible.
        let kernel_b = registry.get_or_open(repo_b.path()).await.unwrap();
        let in_b = list_notes_impl(&kernel_b, None, 10).await.unwrap();
        assert!(in_b.is_empty(), "repo B leaked notes from repo A: {in_b:?}");
        // Two distinct SQLite files on disk — defence in depth on the
        // isolation contract.
        assert!(repo_a.path().join(".vigla/memory/memory.sqlite").exists());
        assert!(repo_b.path().join(".vigla/memory/memory.sqlite").exists());
    }

    /// `resolve_kernel` rejects empty cwd with a clear message rather
    /// than falling through to a canonicalize error the user can't
    /// interpret.
    #[tokio::test]
    async fn resolve_kernel_rejects_empty_cwd_with_clear_message() {
        let registry = MemoryRegistry::new();
        let err = resolve_kernel(&registry, "").await.unwrap_err();
        assert!(err.contains("no active repository"), "got: {err}");
    }

    /// Two calls with the same cwd reuse the same kernel — confirms
    /// the registry caching contract carries through resolve_kernel.
    #[tokio::test]
    async fn resolve_kernel_returns_same_arc_for_repeat_cwd() {
        let registry = MemoryRegistry::new();
        let repo = fresh_repo();
        let cwd = repo.path().to_string_lossy().to_string();
        let k1 = resolve_kernel(&registry, &cwd).await.unwrap();
        let k2 = resolve_kernel(&registry, &cwd).await.unwrap();
        assert!(Arc::ptr_eq(&k1, &k2));
    }
}
