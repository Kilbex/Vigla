//! Cross-mission history: `MissionHistoryDto` and the query that backs
//! `Repository::list_recent_missions`. Extracted from `mod.rs` (Task 4).

use crate::error::RepositoryError;

#[derive(sqlx::FromRow)]
struct MissionHistoryQueryRow {
    mission_id: String,
    tier: String,
    overall: f64,
    created_at: String,
    reverted: i64,
    status: String,
    target_ref: Option<String>,
    repo_root: Option<String>,
    artifacts_cleaned: i64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MissionHistoryStatus {
    /// An audit predates durable disposition tracking, or the mission has not
    /// reached a terminal disposition yet.
    Audited,
    Merged,
    Discarded,
    Aborted,
}

impl MissionHistoryStatus {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Audited => "audited",
            Self::Merged => "merged",
            Self::Discarded => "discarded",
            Self::Aborted => "aborted",
        }
    }

    fn parse(value: &str) -> Result<Self, RepositoryError> {
        match value {
            "audited" => Ok(Self::Audited),
            "merged" => Ok(Self::Merged),
            "discarded" => Ok(Self::Discarded),
            "aborted" => Ok(Self::Aborted),
            other => Err(RepositoryError::RowCorrupt(format!(
                "unknown mission history status {other:?}"
            ))),
        }
    }
}

/// Row returned by `list_recent_missions_impl`. The orchestrator
/// owns the SQL; the host crate wraps this DTO into a specta-typed
/// `MissionHistoryRow` for the frontend.
#[derive(Debug, Clone, PartialEq)]
pub struct MissionHistoryDto {
    pub mission_id: String,
    pub tier: String,
    pub audit_overall: f64,
    pub created_at: String,
    /// True if `mission_revert_log` has any row for this mission.
    pub reverted: bool,
    /// Durable terminal disposition. `Audited` is the fail-closed fallback for
    /// legacy/in-progress rows without a terminal outcome.
    pub status: MissionHistoryStatus,
    /// Branch selected when the mission started. Present for rows with a
    /// durable terminal outcome and used to derive the exact rollback anchor.
    pub target_ref: Option<String>,
    /// Canonical repository root captured when the mission started. Legacy
    /// rows may be `None`; callers must fail closed rather than substitute a
    /// process working directory.
    pub repo_root: Option<String>,
    /// True after the user explicitly removes an aborted mission's retained
    /// Vigla worktrees, branches, and intermediate tags.
    pub artifacts_cleaned: bool,
}

/// Returns the last `limit` audited missions, joined against
/// `mission_outcomes` and `mission_revert_log`. Prefers the mission-level audit
/// (`worker_id IS NULL`) per mission via a ROW_NUMBER() CTE; falls back to the
/// latest worker-level audit. Ordered DESC by `created_at`.
pub(crate) async fn list_recent_missions_impl(
    pool: &sqlx::Pool<sqlx::Sqlite>,
    limit: u32,
) -> Result<Vec<MissionHistoryDto>, RepositoryError> {
    let rows: Vec<MissionHistoryQueryRow> = sqlx::query_as(
        r#"
        WITH ranked_audits AS (
            SELECT mission_id, tier, overall, created_at,
                   ROW_NUMBER() OVER (
                       PARTITION BY mission_id
                       ORDER BY (worker_id IS NULL) DESC, created_at DESC
                   ) AS rank
            FROM audit_reports
        ),
        latest_audits AS (
            SELECT mission_id, tier, overall, created_at
            FROM ranked_audits WHERE rank = 1
        )
        SELECT la.mission_id, la.tier, la.overall, la.created_at,
               CASE WHEN EXISTS (SELECT 1 FROM mission_revert_log r
                                 WHERE r.mission_id = la.mission_id)
                    THEN 1 ELSE 0 END AS reverted,
               COALESCE(mo.state, 'audited') AS status,
               mo.target_ref,
               mo.repo_root,
               CASE WHEN EXISTS (SELECT 1 FROM mission_artifact_cleanup c
                                 WHERE c.mission_id = la.mission_id)
                    THEN 1 ELSE 0 END AS artifacts_cleaned
        FROM latest_audits la
        LEFT JOIN mission_outcomes mo ON mo.mission_id = la.mission_id
        -- `created_at` is an RFC3339 string and is NOT unique across
        -- missions (audits written in the same millisecond collide), so
        -- add the unique mission_id as a deterministic tiebreak. Without
        -- it, the LIMIT boundary and row order are non-deterministic and
        -- the History list can reshuffle between reopens.
        ORDER BY la.created_at DESC, la.mission_id DESC
        LIMIT ?
        "#,
    )
    .bind(limit as i64)
    .fetch_all(pool)
    .await?;

    rows.into_iter()
        .map(|row| {
            Ok(MissionHistoryDto {
                mission_id: row.mission_id,
                tier: row.tier,
                audit_overall: row.overall,
                created_at: row.created_at,
                reverted: row.reverted == 1,
                status: MissionHistoryStatus::parse(&row.status)?,
                target_ref: row.target_ref,
                repo_root: row.repo_root,
                artifacts_cleaned: row.artifacts_cleaned == 1,
            })
        })
        .collect::<Result<Vec<_>, RepositoryError>>()
}

#[cfg(test)]
mod tests {
    use super::{list_recent_missions_impl, MissionHistoryStatus};
    use crate::audit::persist::{insert_audit, insert_audit_at};
    use crate::audit::report::AuditReport;
    use crate::ids::rfc3339_now;
    use crate::repository::fresh_pool;
    use crate::repository::mission_outcomes::{record_mission_outcome_impl, MissionOutcomeState};

    #[tokio::test]
    async fn list_recent_missions_returns_empty_for_fresh_db() {
        let pool = fresh_pool().await;
        let rows = list_recent_missions_impl(&pool, 20).await.unwrap();
        assert_eq!(rows.len(), 0);
    }

    #[tokio::test]
    async fn list_recent_missions_returns_audited_missions_ordered() {
        let pool = fresh_pool().await;
        // T6 — deterministic timestamps instead of sleep-based monotonicity.
        // Lexicographic order on full RFC3339 strings matches chronological,
        // so 1-minute gaps give us a stable DESC ordering.
        for n in 1..=3u32 {
            let mid = format!("mission-{n}");
            let report = AuditReport {
                overall: 0.8 + (n as f64) * 0.01,
                ..Default::default()
            };
            let ts = format!("2026-01-01T00:0{n}:00Z");
            insert_audit_at(&pool, &mid, None, "standard", &report, &ts)
                .await
                .unwrap();
        }
        let rows = list_recent_missions_impl(&pool, 20).await.unwrap();
        assert_eq!(rows.len(), 3);
        // Ordered DESC by created_at — most-recent first.
        assert_eq!(rows[0].mission_id, "mission-3");
        assert!((rows[0].audit_overall - 0.83).abs() < 1e-6);
        for row in &rows {
            assert!(!row.reverted);
            assert_eq!(row.status, MissionHistoryStatus::Audited);
            assert_eq!(row.target_ref, None);
            assert_eq!(row.repo_root, None);
            assert!(!row.artifacts_cleaned);
        }
    }

    #[tokio::test]
    async fn list_recent_missions_marks_reverted_missions() {
        let pool = fresh_pool().await;
        let mid = "mission-revert";
        insert_audit(&pool, mid, None, "deep", &AuditReport::default())
            .await
            .unwrap();
        record_mission_outcome_impl(
            &pool,
            mid,
            "/repo/revert",
            "main",
            MissionOutcomeState::Merged,
            "2026-05-22T09:59:00Z",
        )
        .await
        .unwrap();
        crate::repository::revert_log::insert_mission_revert(
            &pool,
            mid,
            &rfc3339_now(),
            "deadbeef",
            "vigla/pre-merge/x",
        )
        .await
        .unwrap();
        let rows = list_recent_missions_impl(&pool, 20).await.unwrap();
        assert_eq!(rows.len(), 1);
        assert!(rows[0].reverted);
        assert_eq!(rows[0].status, MissionHistoryStatus::Merged);
        assert_eq!(rows[0].target_ref.as_deref(), Some("main"));
        assert_eq!(rows[0].repo_root.as_deref(), Some("/repo/revert"));
        assert!(!rows[0].artifacts_cleaned);
    }

    #[tokio::test]
    async fn list_recent_missions_preserves_each_terminal_disposition() {
        let pool = fresh_pool().await;
        let states = [
            ("mission-merged", MissionOutcomeState::Merged),
            ("mission-discarded", MissionOutcomeState::Discarded),
            ("mission-aborted", MissionOutcomeState::Aborted),
        ];
        for (index, (mission_id, state)) in states.into_iter().enumerate() {
            insert_audit_at(
                &pool,
                mission_id,
                None,
                "standard",
                &AuditReport::default(),
                &format!("2026-07-21T12:0{index}:00Z"),
            )
            .await
            .unwrap();
            record_mission_outcome_impl(
                &pool,
                mission_id,
                "/repo/history",
                "release/v1",
                state,
                &format!("2026-07-21T12:0{index}:01Z"),
            )
            .await
            .unwrap();
        }

        let rows = list_recent_missions_impl(&pool, 10).await.unwrap();
        let statuses: Vec<_> = rows.iter().map(|row| row.status).collect();
        assert_eq!(
            statuses,
            vec![
                MissionHistoryStatus::Aborted,
                MissionHistoryStatus::Discarded,
                MissionHistoryStatus::Merged,
            ]
        );
        assert!(rows
            .iter()
            .all(|row| row.target_ref.as_deref() == Some("release/v1")));
        assert!(rows
            .iter()
            .all(|row| row.repo_root.as_deref() == Some("/repo/history")));
        assert!(rows.iter().all(|row| !row.artifacts_cleaned));
    }

    #[tokio::test]
    async fn list_recent_missions_marks_aborted_artifact_cleanup() {
        let pool = fresh_pool().await;
        insert_audit(
            &pool,
            "mission-aborted",
            None,
            "standard",
            &AuditReport::default(),
        )
        .await
        .unwrap();
        record_mission_outcome_impl(
            &pool,
            "mission-aborted",
            "/repo/history",
            "main",
            MissionOutcomeState::Aborted,
            "2026-07-21T12:00:00Z",
        )
        .await
        .unwrap();
        crate::repository::mission_cleanup::record_cleanup_impl(
            &pool,
            "mission-aborted",
            "/repo/history",
            "2026-07-21T12:01:00Z",
        )
        .await
        .unwrap();

        let rows = list_recent_missions_impl(&pool, 20).await.unwrap();
        assert_eq!(rows.len(), 1);
        assert!(rows[0].artifacts_cleaned);
    }

    #[tokio::test]
    async fn list_recent_missions_respects_limit() {
        let pool = fresh_pool().await;
        for n in 1..=25u32 {
            let mid = format!("mission-{n:02}");
            insert_audit(&pool, &mid, None, "smoke", &AuditReport::default())
                .await
                .unwrap();
        }
        assert_eq!(
            list_recent_missions_impl(&pool, 20).await.unwrap().len(),
            20
        );
    }

    #[tokio::test]
    async fn list_recent_missions_prefers_mission_level_audit() {
        let pool = fresh_pool().await;
        let mid = "mission-multi";
        let report1 = AuditReport {
            overall: 0.5,
            ..Default::default()
        };
        // T6 — deterministic ordering. Worker audit earlier; mission-level
        // audit later — `(worker_id IS NULL) DESC` should pick the
        // mission-level audit regardless.
        insert_audit_at(
            &pool,
            mid,
            Some("worker-1"),
            "smoke",
            &report1,
            "2026-01-01T00:00:00Z",
        )
        .await
        .unwrap();
        let report2 = AuditReport {
            overall: 0.9,
            ..Default::default()
        };
        insert_audit_at(&pool, mid, None, "deep", &report2, "2026-01-01T00:01:00Z")
            .await
            .unwrap();

        let rows = list_recent_missions_impl(&pool, 20).await.unwrap();
        assert_eq!(rows.len(), 1);
        // The mission-level (worker_id IS NULL) audit wins.
        assert!((rows[0].audit_overall - 0.9).abs() < 1e-6);
        assert_eq!(rows[0].tier, "deep");
    }

    /// T6 — when only worker-level audits exist, the `(worker_id IS NULL)
    /// DESC, created_at DESC` sort falls back to the latest worker-level
    /// audit. Previously untested.
    #[tokio::test]
    async fn list_recent_missions_picks_latest_worker_audit_when_no_mission_audit() {
        let pool = fresh_pool().await;
        let smoke = AuditReport {
            overall: 0.6,
            ..Default::default()
        };
        let standard = AuditReport {
            overall: 0.8,
            ..Default::default()
        };
        let deep = AuditReport {
            overall: 0.9,
            ..Default::default()
        };
        insert_audit_at(
            &pool,
            "M1",
            Some("w1"),
            "smoke",
            &smoke,
            "2026-01-01T00:00:00Z",
        )
        .await
        .unwrap();
        insert_audit_at(
            &pool,
            "M1",
            Some("w2"),
            "standard",
            &standard,
            "2026-01-01T00:01:00Z",
        )
        .await
        .unwrap();
        insert_audit_at(
            &pool,
            "M1",
            Some("w3"),
            "deep",
            &deep,
            "2026-01-01T00:02:00Z",
        )
        .await
        .unwrap();

        let rows = list_recent_missions_impl(&pool, 10).await.unwrap();
        assert_eq!(rows.len(), 1);
        // Without a mission-level audit, the latest worker audit wins.
        assert_eq!(rows[0].tier, "deep");
        assert!((rows[0].audit_overall - 0.9).abs() < 1e-9);
    }

    /// T6 — mission-level audit beats a later worker-level audit. Asserts
    /// the `(worker_id IS NULL) DESC` clause takes priority over
    /// `created_at DESC`.
    #[tokio::test]
    async fn list_recent_missions_prefers_mission_audit_even_if_older() {
        let pool = fresh_pool().await;
        let mission_level = AuditReport {
            overall: 0.7,
            ..Default::default()
        };
        let worker_level = AuditReport {
            overall: 0.95,
            ..Default::default()
        };
        insert_audit_at(
            &pool,
            "M1",
            None,
            "standard",
            &mission_level,
            "2026-01-01T00:00:00Z",
        )
        .await
        .unwrap();
        insert_audit_at(
            &pool,
            "M1",
            Some("w1"),
            "deep",
            &worker_level,
            "2026-01-01T01:00:00Z",
        )
        .await
        .unwrap();

        let rows = list_recent_missions_impl(&pool, 10).await.unwrap();
        assert_eq!(rows.len(), 1);
        // Mission-level (worker_id IS NULL) wins regardless of created_at.
        assert_eq!(rows[0].tier, "standard");
        assert!((rows[0].audit_overall - 0.7).abs() < 1e-9);
    }
}
