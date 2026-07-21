//! Durable terminal disposition for missions.
//!
//! Audit history and mission disposition are intentionally separate facts: a
//! mission can pass its audit and still be discarded. The first terminal
//! disposition wins; replaying the same event is idempotent, while a conflicting
//! terminal event is treated as repository corruption instead of silently
//! changing history.

use crate::error::RepositoryError;
use sqlx::{Row, SqlitePool};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MissionOutcomeState {
    Merged,
    Discarded,
    Aborted,
}

impl MissionOutcomeState {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Merged => "merged",
            Self::Discarded => "discarded",
            Self::Aborted => "aborted",
        }
    }

    pub(crate) fn parse(value: &str) -> Result<Self, RepositoryError> {
        match value {
            "merged" => Ok(Self::Merged),
            "discarded" => Ok(Self::Discarded),
            "aborted" => Ok(Self::Aborted),
            other => Err(RepositoryError::RowCorrupt(format!(
                "unknown mission outcome state {other:?}"
            ))),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MissionOutcomeDto {
    pub mission_id: String,
    pub repo_root: Option<String>,
    pub target_ref: String,
    pub state: MissionOutcomeState,
    pub updated_at: String,
}

pub(crate) async fn record_mission_outcome_impl(
    pool: &SqlitePool,
    mission_id: &str,
    repo_root: &str,
    target_ref: &str,
    state: MissionOutcomeState,
    updated_at: &str,
) -> Result<(), RepositoryError> {
    let result = sqlx::query(
        "INSERT INTO mission_outcomes (mission_id, repo_root, target_ref, state, updated_at)
         VALUES (?1, ?2, ?3, ?4, ?5)
         ON CONFLICT(mission_id) DO NOTHING",
    )
    .bind(mission_id)
    .bind(repo_root)
    .bind(target_ref)
    .bind(state.as_str())
    .bind(updated_at)
    .execute(pool)
    .await?;

    if result.rows_affected() == 1 {
        return Ok(());
    }

    let existing = mission_outcome_impl(pool, mission_id)
        .await?
        .ok_or_else(|| {
            RepositoryError::RowCorrupt(format!(
                "mission outcome conflict for {mission_id:?} but no row exists"
            ))
        })?;
    if existing.repo_root.as_deref() == Some(repo_root)
        && existing.target_ref == target_ref
        && existing.state == state
    {
        return Ok(());
    }

    Err(RepositoryError::RowCorrupt(format!(
        "mission {mission_id:?} already ended as {} on {:?}; refusing conflicting {} outcome on {:?}",
        existing.state.as_str(),
        existing.target_ref,
        state.as_str(),
        target_ref
    )))
}

pub(crate) async fn mission_outcome_impl(
    pool: &SqlitePool,
    mission_id: &str,
) -> Result<Option<MissionOutcomeDto>, RepositoryError> {
    let row = sqlx::query(
        "SELECT mission_id, repo_root, target_ref, state, updated_at
         FROM mission_outcomes WHERE mission_id = ?1",
    )
    .bind(mission_id)
    .fetch_optional(pool)
    .await?;

    row.map(|row| {
        let state: String = row.try_get("state")?;
        Ok(MissionOutcomeDto {
            mission_id: row.try_get("mission_id")?,
            repo_root: row.try_get("repo_root")?,
            target_ref: row.try_get("target_ref")?,
            state: MissionOutcomeState::parse(&state)?,
            updated_at: row.try_get("updated_at")?,
        })
    })
    .transpose()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::repository::fresh_pool;

    #[tokio::test]
    async fn outcome_round_trips_and_same_event_is_idempotent() {
        let pool = fresh_pool().await;
        record_mission_outcome_impl(
            &pool,
            "mission-1",
            "/repo/one",
            "main",
            MissionOutcomeState::Merged,
            "2026-07-21T12:00:00Z",
        )
        .await
        .unwrap();
        record_mission_outcome_impl(
            &pool,
            "mission-1",
            "/repo/one",
            "main",
            MissionOutcomeState::Merged,
            "2026-07-21T12:00:00Z",
        )
        .await
        .unwrap();

        let outcome = mission_outcome_impl(&pool, "mission-1")
            .await
            .unwrap()
            .unwrap();
        assert_eq!(outcome.mission_id, "mission-1");
        assert_eq!(outcome.repo_root.as_deref(), Some("/repo/one"));
        assert_eq!(outcome.target_ref, "main");
        assert_eq!(outcome.state, MissionOutcomeState::Merged);
        assert_eq!(outcome.updated_at, "2026-07-21T12:00:00Z");
    }

    #[tokio::test]
    async fn first_terminal_outcome_cannot_be_rewritten() {
        let pool = fresh_pool().await;
        record_mission_outcome_impl(
            &pool,
            "mission-1",
            "/repo/one",
            "main",
            MissionOutcomeState::Discarded,
            "2026-07-21T12:00:00Z",
        )
        .await
        .unwrap();

        let error = record_mission_outcome_impl(
            &pool,
            "mission-1",
            "/repo/two",
            "release",
            MissionOutcomeState::Merged,
            "2026-07-21T12:00:01Z",
        )
        .await
        .unwrap_err();
        assert!(error.to_string().contains("refusing conflicting"));

        let outcome = mission_outcome_impl(&pool, "mission-1")
            .await
            .unwrap()
            .unwrap();
        assert_eq!(outcome.target_ref, "main");
        assert_eq!(outcome.state, MissionOutcomeState::Discarded);
    }
}
