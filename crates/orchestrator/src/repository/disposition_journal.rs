//! Write-ahead journal for terminal mission dispositions.
//!
//! A merge or discard intent is persisted before Git is mutated. The row is
//! removed only after the authoritative outcome is durable and cleanup has
//! completed, allowing startup reconciliation to close a crash window without
//! guessing from the process working directory.

use crate::error::RepositoryError;
use sqlx::{Row, SqlitePool};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DispositionAction {
    Merge,
    Discard,
}

impl DispositionAction {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Merge => "merge",
            Self::Discard => "discard",
        }
    }

    fn parse(value: &str) -> Result<Self, RepositoryError> {
        match value {
            "merge" => Ok(Self::Merge),
            "discard" => Ok(Self::Discard),
            other => Err(RepositoryError::RowCorrupt(format!(
                "unknown mission disposition action {other:?}"
            ))),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DispositionIntentDto {
    pub mission_id: String,
    pub repo_root: String,
    pub target_ref: String,
    pub action: DispositionAction,
    pub created_at: String,
}

pub(crate) async fn record_intent_impl(
    pool: &SqlitePool,
    mission_id: &str,
    repo_root: &str,
    target_ref: &str,
    action: DispositionAction,
    created_at: &str,
) -> Result<(), RepositoryError> {
    let inserted = sqlx::query(
        "INSERT INTO mission_disposition_journal
            (mission_id, repo_root, target_ref, action, created_at)
         VALUES (?1, ?2, ?3, ?4, ?5)
         ON CONFLICT(mission_id) DO NOTHING",
    )
    .bind(mission_id)
    .bind(repo_root)
    .bind(target_ref)
    .bind(action.as_str())
    .bind(created_at)
    .execute(pool)
    .await?;
    if inserted.rows_affected() == 1 {
        return Ok(());
    }

    let existing = intent_impl(pool, mission_id).await?.ok_or_else(|| {
        RepositoryError::RowCorrupt(format!(
            "disposition intent conflict for {mission_id:?} but no row exists"
        ))
    })?;
    if existing.repo_root == repo_root
        && existing.target_ref == target_ref
        && existing.action == action
    {
        return Ok(());
    }
    Err(RepositoryError::RowCorrupt(format!(
        "mission {mission_id:?} already has a {} intent for {} at {}; refusing {}",
        existing.action.as_str(),
        existing.target_ref,
        existing.repo_root,
        action.as_str()
    )))
}

async fn intent_impl(
    pool: &SqlitePool,
    mission_id: &str,
) -> Result<Option<DispositionIntentDto>, RepositoryError> {
    let row = sqlx::query(
        "SELECT mission_id, repo_root, target_ref, action, created_at
         FROM mission_disposition_journal WHERE mission_id = ?1",
    )
    .bind(mission_id)
    .fetch_optional(pool)
    .await?;
    row.map(intent_from_row).transpose()
}

pub(crate) async fn list_intents_impl(
    pool: &SqlitePool,
) -> Result<Vec<DispositionIntentDto>, RepositoryError> {
    sqlx::query(
        "SELECT mission_id, repo_root, target_ref, action, created_at
         FROM mission_disposition_journal ORDER BY created_at, mission_id",
    )
    .fetch_all(pool)
    .await?
    .into_iter()
    .map(intent_from_row)
    .collect()
}

fn intent_from_row(row: sqlx::sqlite::SqliteRow) -> Result<DispositionIntentDto, RepositoryError> {
    let action: String = row.try_get("action")?;
    Ok(DispositionIntentDto {
        mission_id: row.try_get("mission_id")?,
        repo_root: row.try_get("repo_root")?,
        target_ref: row.try_get("target_ref")?,
        action: DispositionAction::parse(&action)?,
        created_at: row.try_get("created_at")?,
    })
}

pub(crate) async fn clear_intent_impl(
    pool: &SqlitePool,
    mission_id: &str,
) -> Result<(), RepositoryError> {
    sqlx::query("DELETE FROM mission_disposition_journal WHERE mission_id = ?1")
        .bind(mission_id)
        .execute(pool)
        .await?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::repository::fresh_pool;

    #[tokio::test]
    async fn intent_is_idempotent_but_cannot_change_action_or_repository() {
        let pool = fresh_pool().await;
        record_intent_impl(
            &pool,
            "mission-1",
            "/repo/a",
            "main",
            DispositionAction::Merge,
            "2026-07-21T12:00:00Z",
        )
        .await
        .unwrap();
        record_intent_impl(
            &pool,
            "mission-1",
            "/repo/a",
            "main",
            DispositionAction::Merge,
            "later timestamps are ignored for idempotent replay",
        )
        .await
        .unwrap();
        assert_eq!(list_intents_impl(&pool).await.unwrap().len(), 1);

        let error = record_intent_impl(
            &pool,
            "mission-1",
            "/repo/b",
            "main",
            DispositionAction::Discard,
            "2026-07-21T12:00:01Z",
        )
        .await
        .unwrap_err();
        assert!(error.to_string().contains("refusing discard"));

        clear_intent_impl(&pool, "mission-1").await.unwrap();
        assert!(list_intents_impl(&pool).await.unwrap().is_empty());
    }
}
