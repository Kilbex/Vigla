//! Persistent record of cross-worker handoff notes.
//!
//! S8 introduces explicit, structured notes that a worker can
//! leave for a downstream task. The DAG scheduler (S7) reads all
//! upstream handoffs when assembling a downstream brief, and the
//! memory kernel optionally promotes them for cross-mission
//! recall. Persistence is durable so a process restart doesn't
//! lose the trail.

use crate::memory::error::MemoryError;
use crate::memory::ids;
use crate::memory::MemoryKernel;
use crate::mission_runtime::WorkerRole;
use serde::{Deserialize, Serialize};

/// Persisted shape of a handoff. Mirrors
/// `MissionEventKind::HandoffNote` payload plus a mission id.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct HandoffNote {
    pub mission_id: String,
    pub from_worker: String,
    pub to_role: WorkerRole,
    pub note: String,
}

/// Insert one handoff row and return the new `handoff_id`.
pub async fn persist_handoff(
    kernel: &MemoryKernel,
    note: &HandoffNote,
) -> Result<String, MemoryError> {
    let id = ids::new_memory_event_id();
    let now = crate::ids::rfc3339_now();
    let role_str = role_as_str(note.to_role);
    sqlx::query(
        "INSERT INTO memory_handoffs \
         (handoff_id, mission_id, from_worker, to_role, note, created_at) \
         VALUES (?, ?, ?, ?, ?, ?)",
    )
    .bind(&id)
    .bind(&note.mission_id)
    .bind(&note.from_worker)
    .bind(role_str)
    .bind(&note.note)
    .bind(&now)
    .execute(&kernel.pool)
    .await?;
    Ok(id)
}

/// List handoffs for one mission, in insertion order.
pub async fn list_handoffs_for_mission(
    kernel: &MemoryKernel,
    mission_id: &str,
) -> Result<Vec<HandoffNote>, MemoryError> {
    let rows: Vec<(String, String, String, String)> = sqlx::query_as(
        "SELECT mission_id, from_worker, to_role, note \
         FROM memory_handoffs WHERE mission_id = ? ORDER BY created_at ASC",
    )
    .bind(mission_id)
    .fetch_all(&kernel.pool)
    .await?;
    Ok(rows
        .into_iter()
        .map(|(m, f, r, n)| HandoffNote {
            mission_id: m,
            from_worker: f,
            to_role: str_as_role(&r),
            note: n,
        })
        .collect())
}

fn role_as_str(role: WorkerRole) -> &'static str {
    match role {
        WorkerRole::Supervisor => "supervisor",
        WorkerRole::Employee => "employee",
    }
}

fn str_as_role(s: &str) -> WorkerRole {
    match s {
        "supervisor" => WorkerRole::Supervisor,
        _ => WorkerRole::Employee,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::memory::MemoryKernel;
    use crate::mission_runtime::WorkerRole;
    use sqlx::sqlite::{SqliteConnectOptions, SqlitePoolOptions};
    use std::str::FromStr;
    use tempfile::TempDir;

    async fn fresh_kernel() -> (MemoryKernel, TempDir) {
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
        let root = TempDir::new().unwrap();
        let kernel = MemoryKernel::open(pool, root.path().to_path_buf())
            .await
            .unwrap();
        (kernel, root)
    }

    #[tokio::test]
    async fn persist_then_list_round_trips() {
        let (kernel, _root) = fresh_kernel().await;
        let note = HandoffNote {
            mission_id: "mid-1".into(),
            from_worker: "mock-1".into(),
            to_role: WorkerRole::Employee,
            note: "left tests passing; please add coverage for X".into(),
        };
        let id = persist_handoff(&kernel, &note).await.unwrap();
        assert!(!id.is_empty());

        let listed = list_handoffs_for_mission(&kernel, "mid-1").await.unwrap();
        assert_eq!(listed.len(), 1);
        assert_eq!(listed[0].from_worker, "mock-1");
        assert_eq!(listed[0].note, note.note);
    }

    #[tokio::test]
    async fn list_returns_empty_when_no_handoffs() {
        let (kernel, _root) = fresh_kernel().await;
        let listed = list_handoffs_for_mission(&kernel, "mid-empty")
            .await
            .unwrap();
        assert!(listed.is_empty());
    }

    #[tokio::test]
    async fn handoffs_listed_in_insertion_order() {
        let (kernel, _root) = fresh_kernel().await;
        for i in 0..3 {
            let note = HandoffNote {
                mission_id: "mid-1".into(),
                from_worker: format!("w-{i}"),
                to_role: WorkerRole::Employee,
                note: format!("note {i}"),
            };
            persist_handoff(&kernel, &note).await.unwrap();
            // Ensure created_at strings differ so ORDER BY created_at
            // produces a stable order. rfc3339_now() has millisecond
            // resolution; sleep a tick to avoid ties.
            tokio::time::sleep(std::time::Duration::from_millis(5)).await;
        }
        let listed = list_handoffs_for_mission(&kernel, "mid-1").await.unwrap();
        assert_eq!(listed.len(), 3);
        assert_eq!(listed[0].from_worker, "w-0");
        assert_eq!(listed[1].from_worker, "w-1");
        assert_eq!(listed[2].from_worker, "w-2");
    }
}
