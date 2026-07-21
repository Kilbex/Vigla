//! Tauri command backing S10's MissionHistory cross-mission view.
//!
//! Wraps `orchestrator::repository::Repository::list_recent_missions`
//! and serialises each row as a `MissionHistoryRow` DTO so specta
//! emits a typed binding for the frontend. The DTO lives here
//! (rather than in the orchestrator crate) because specta `Type`
//! requires a host-side derive and we keep the orchestrator
//! framework-agnostic.

use orchestrator::{MissionHistoryDto, MissionHistoryStatus};
use serde::Serialize;
use specta::Type;
use tauri::State;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Type)]
#[serde(rename_all = "snake_case")]
pub enum MissionHistoryStatusDto {
    Audited,
    Merged,
    Discarded,
    Aborted,
}

impl From<MissionHistoryStatus> for MissionHistoryStatusDto {
    fn from(status: MissionHistoryStatus) -> Self {
        match status {
            MissionHistoryStatus::Audited => Self::Audited,
            MissionHistoryStatus::Merged => Self::Merged,
            MissionHistoryStatus::Discarded => Self::Discarded,
            MissionHistoryStatus::Aborted => Self::Aborted,
        }
    }
}

/// One row in the MissionHistory view. Outcome state is authoritative for
/// whether the frontend may offer a rollback.
#[derive(Debug, Clone, Serialize, Type)]
pub struct MissionHistoryRow {
    pub mission_id: String,
    pub tier: String,
    pub audit_overall: f64,
    pub created_at: String,
    pub reverted: bool,
    pub status: MissionHistoryStatusDto,
    pub target_ref: Option<String>,
    pub repo_root: Option<String>,
    pub artifacts_cleaned: bool,
}

impl From<MissionHistoryDto> for MissionHistoryRow {
    fn from(d: MissionHistoryDto) -> Self {
        MissionHistoryRow {
            mission_id: d.mission_id,
            tier: d.tier,
            audit_overall: d.audit_overall,
            created_at: d.created_at,
            reverted: d.reverted,
            status: d.status.into(),
            target_ref: d.target_ref,
            repo_root: d.repo_root,
            artifacts_cleaned: d.artifacts_cleaned,
        }
    }
}

#[tauri::command]
#[specta::specta]
pub async fn list_recent_missions(
    limit: u32,
    runtime: State<'_, crate::RuntimeHandle>,
) -> Result<Vec<MissionHistoryRow>, String> {
    // Clamp the limit to a sane ceiling so a malicious / accidental
    // huge limit doesn't materialise an enormous result set in IPC.
    // The frontend asks for 20; the host enforces a hard cap of 100.
    let clamped = limit.min(100);
    let rows = runtime
        .ready()?
        .repository
        .list_recent_missions(clamped)
        .await
        .map_err(|e| e.to_string())?;
    Ok(rows.into_iter().map(MissionHistoryRow::from).collect())
}

#[cfg(test)]
mod tests {
    use super::*;
    use orchestrator::audit::persist::insert_audit;
    use orchestrator::audit::report::AuditReport;
    use orchestrator::Repository;

    #[tokio::test]
    async fn list_recent_missions_returns_typed_rows() {
        let repo = Repository::open_in_memory().await.unwrap();
        let report = AuditReport {
            overall: 0.75,
            ..Default::default()
        };
        insert_audit(repo.pool_for_test(), "mission-A", None, "standard", &report)
            .await
            .unwrap();
        repo.record_mission_outcome(
            "mission-A",
            "/repo/a",
            "main",
            orchestrator::MissionOutcomeState::Merged,
            "2026-07-21T12:00:00Z",
        )
        .await
        .unwrap();
        let rows: Vec<MissionHistoryRow> = repo
            .list_recent_missions(20)
            .await
            .unwrap()
            .into_iter()
            .map(MissionHistoryRow::from)
            .collect();
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].mission_id, "mission-A");
        assert!((rows[0].audit_overall - 0.75).abs() < 1e-6);
        assert!(!rows[0].reverted);
        assert_eq!(rows[0].status, MissionHistoryStatusDto::Merged);
        assert_eq!(rows[0].target_ref.as_deref(), Some("main"));
        assert_eq!(rows[0].repo_root.as_deref(), Some("/repo/a"));
        assert!(!rows[0].artifacts_cleaned);
    }
}
