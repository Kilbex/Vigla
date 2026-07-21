//! SQLite persistence for AuditReports. Keyed by (mission, worker,
//! created_at) so multiple audits per mission can be stored
//! (one per worker submission + one mission-level final audit).

use crate::audit::report::AuditReport;
use crate::ids::rfc3339_now;
use sqlx::SqlitePool;

pub struct AuditRow {
    pub mission_id: String,
    pub worker_id: Option<String>,
    pub tier: String,
    pub overall: f64,
    pub payload_json: String,
    pub created_at: String,
}

pub async fn insert_audit(
    pool: &SqlitePool,
    mission_id: &str,
    worker_id: Option<&str>,
    tier: &str,
    report: &AuditReport,
) -> Result<(), sqlx::Error> {
    let created_at = rfc3339_now();
    insert_audit_at(pool, mission_id, worker_id, tier, report, &created_at).await
}

/// Variant of [`insert_audit`] that takes an explicit `created_at` timestamp.
/// Mission-event consumers use the source event's timestamp so retries remain
/// idempotent; tests use it for deterministic ordering without sleeps.
#[doc(hidden)]
pub async fn insert_audit_at(
    pool: &SqlitePool,
    mission_id: &str,
    worker_id: Option<&str>,
    tier: &str,
    report: &AuditReport,
    created_at: &str,
) -> Result<(), sqlx::Error> {
    let payload = serde_json::to_string(report).map_err(|e| sqlx::Error::Decode(Box::new(e)))?;
    // ON CONFLICT DO NOTHING: the PK protects worker-level rows, and migration
    // 0014's partial unique index protects mission-level rows (worker_id IS
    // NULL). A same-key re-insert is a silent no-op rather than a duplicate row
    // (the old behavior, since SQLite NULLs are distinct in the PK) or a hard
    // error. DO NOTHING (vs OR IGNORE) only suppresses conflicts, not NOT NULL
    // violations (F-9).
    sqlx::query(
        "INSERT INTO audit_reports (mission_id, worker_id, tier, overall, payload_json, created_at)
         VALUES (?, ?, ?, ?, ?, ?)
         ON CONFLICT DO NOTHING",
    )
    .bind(mission_id)
    .bind(worker_id)
    .bind(tier)
    .bind(report.overall)
    .bind(payload)
    .bind(created_at)
    .execute(pool)
    .await?;
    Ok(())
}

pub async fn list_audits_for_mission(
    pool: &SqlitePool,
    mission_id: &str,
) -> Result<Vec<AuditRow>, sqlx::Error> {
    let rows = sqlx::query_as::<_, (String, Option<String>, String, f64, String, String)>(
        "SELECT mission_id, worker_id, tier, overall, payload_json, created_at
         FROM audit_reports WHERE mission_id = ? ORDER BY created_at ASC",
    )
    .bind(mission_id)
    .fetch_all(pool)
    .await?;
    Ok(rows
        .into_iter()
        .map(|(m, w, t, o, p, c)| AuditRow {
            mission_id: m,
            worker_id: w,
            tier: t,
            overall: o,
            payload_json: p,
            created_at: c,
        })
        .collect())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::audit::report::AuditReport;
    use sqlx::SqlitePool;
    use tempfile::tempdir;

    async fn fresh_pool() -> SqlitePool {
        let dir = tempdir().unwrap();
        let db_path = dir.path().join("audit.sqlite");
        let url = format!("sqlite:{}?mode=rwc", db_path.display());
        let pool = SqlitePool::connect(&url).await.unwrap();
        sqlx::migrate!("./migrations").run(&pool).await.unwrap();
        std::mem::forget(dir); // keep file alive for the duration of the test
        pool
    }

    #[tokio::test]
    async fn insert_and_query_round_trip() {
        let pool = fresh_pool().await;
        let report = AuditReport {
            overall: 0.83,
            ..Default::default()
        };
        insert_audit(&pool, "mission-1", Some("worker-1"), "smoke", &report)
            .await
            .unwrap();
        let rows = list_audits_for_mission(&pool, "mission-1").await.unwrap();
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].tier, "smoke");
        assert!((rows[0].overall - 0.83).abs() < 1e-6);
    }
}
