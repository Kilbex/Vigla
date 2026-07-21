//! Durable completion marker for explicit aborted-mission artifact cleanup.
//!
//! Aborting a mission intentionally retains its Vigla-owned worktrees,
//! branches, and intermediate tags so a developer can inspect the failed run.
//! The cleanup action is separate and idempotent: Git is cleaned first, then
//! this table records completion so History can stop offering the action.

use crate::error::RepositoryError;
use sqlx::{Row, SqlitePool};

pub(crate) async fn record_cleanup_impl(
    pool: &SqlitePool,
    mission_id: &str,
    repo_root: &str,
    cleaned_at: &str,
) -> Result<(), RepositoryError> {
    // Authorize at the persistence boundary too: only an aborted outcome with
    // the exact recorded repository identity may acquire a cleanup marker.
    let result = sqlx::query(
        "INSERT INTO mission_artifact_cleanup (mission_id, repo_root, cleaned_at)
         SELECT mission_id, repo_root, ?3
         FROM mission_outcomes
         WHERE mission_id = ?1 AND state = 'aborted' AND repo_root = ?2
         ON CONFLICT(mission_id) DO NOTHING",
    )
    .bind(mission_id)
    .bind(repo_root)
    .bind(cleaned_at)
    .execute(pool)
    .await?;

    if result.rows_affected() == 1 {
        return Ok(());
    }

    let existing =
        sqlx::query("SELECT repo_root FROM mission_artifact_cleanup WHERE mission_id = ?1")
            .bind(mission_id)
            .fetch_optional(pool)
            .await?;
    if let Some(row) = existing {
        let existing_root: String = row.try_get("repo_root")?;
        if existing_root == repo_root {
            return Ok(());
        }
        return Err(RepositoryError::RowCorrupt(format!(
            "mission {mission_id:?} cleanup was recorded for {existing_root:?}; refusing {repo_root:?}"
        )));
    }

    Err(RepositoryError::RowCorrupt(format!(
        "mission {mission_id:?} has no aborted outcome for repository {repo_root:?}"
    )))
}

pub(crate) async fn mission_was_cleaned_impl(
    pool: &SqlitePool,
    mission_id: &str,
) -> Result<bool, RepositoryError> {
    let found: i64 = sqlx::query_scalar(
        "SELECT EXISTS(SELECT 1 FROM mission_artifact_cleanup WHERE mission_id = ?1)",
    )
    .bind(mission_id)
    .fetch_one(pool)
    .await?;
    Ok(found == 1)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::repository::fresh_pool;
    use crate::repository::mission_outcomes::{record_mission_outcome_impl, MissionOutcomeState};

    #[tokio::test]
    async fn aborted_cleanup_round_trips_and_is_idempotent() {
        let pool = fresh_pool().await;
        record_mission_outcome_impl(
            &pool,
            "mission-aborted",
            "/repo/one",
            "main",
            MissionOutcomeState::Aborted,
            "2026-07-21T12:00:00Z",
        )
        .await
        .unwrap();

        record_cleanup_impl(
            &pool,
            "mission-aborted",
            "/repo/one",
            "2026-07-21T12:01:00Z",
        )
        .await
        .unwrap();
        record_cleanup_impl(
            &pool,
            "mission-aborted",
            "/repo/one",
            "2026-07-21T12:02:00Z",
        )
        .await
        .unwrap();

        assert!(mission_was_cleaned_impl(&pool, "mission-aborted")
            .await
            .unwrap());
        let count: i64 = sqlx::query_scalar(
            "SELECT COUNT(*) FROM mission_artifact_cleanup WHERE mission_id = ?1",
        )
        .bind("mission-aborted")
        .fetch_one(&pool)
        .await
        .unwrap();
        assert_eq!(count, 1);
    }

    #[tokio::test]
    async fn cleanup_refuses_missing_non_aborted_or_mismatched_outcomes() {
        let pool = fresh_pool().await;
        let missing = record_cleanup_impl(&pool, "missing", "/repo/one", "now")
            .await
            .unwrap_err();
        assert!(missing.to_string().contains("no aborted outcome"));

        record_mission_outcome_impl(
            &pool,
            "mission-merged",
            "/repo/one",
            "main",
            MissionOutcomeState::Merged,
            "2026-07-21T12:00:00Z",
        )
        .await
        .unwrap();
        let merged = record_cleanup_impl(&pool, "mission-merged", "/repo/one", "now")
            .await
            .unwrap_err();
        assert!(merged.to_string().contains("no aborted outcome"));

        record_mission_outcome_impl(
            &pool,
            "mission-aborted",
            "/repo/one",
            "main",
            MissionOutcomeState::Aborted,
            "2026-07-21T12:00:00Z",
        )
        .await
        .unwrap();
        let mismatch = record_cleanup_impl(&pool, "mission-aborted", "/repo/two", "now")
            .await
            .unwrap_err();
        assert!(mismatch.to_string().contains("no aborted outcome"));
        assert!(!mission_was_cleaned_impl(&pool, "mission-aborted")
            .await
            .unwrap());
    }
}
