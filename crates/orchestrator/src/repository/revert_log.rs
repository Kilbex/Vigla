//! mission_revert_log helpers (append-only revert audit, migration 0008).

/// Append a revert-log entry. Idempotent at the (mission_id,
/// reverted_at) primary key.
pub(crate) async fn insert_mission_revert(
    pool: &sqlx::Pool<sqlx::Sqlite>,
    mission_id: &str,
    reverted_at: &str,
    restored_sha: &str,
    pre_merge_tag: &str,
) -> Result<(), sqlx::Error> {
    sqlx::query(
        "INSERT OR IGNORE INTO mission_revert_log
            (mission_id, reverted_at, restored_sha, pre_merge_tag)
         VALUES (?1, ?2, ?3, ?4)",
    )
    .bind(mission_id)
    .bind(reverted_at)
    .bind(restored_sha)
    .bind(pre_merge_tag)
    .execute(pool)
    .await?;
    Ok(())
}

/// Return true if this mission has been reverted at least once.
/// Used by `revert_mission` for idempotency.
pub(crate) async fn mission_was_reverted(
    pool: &sqlx::Pool<sqlx::Sqlite>,
    mission_id: &str,
) -> Result<bool, sqlx::Error> {
    let row: Option<(i64,)> =
        sqlx::query_as("SELECT COUNT(*) FROM mission_revert_log WHERE mission_id = ?1")
            .bind(mission_id)
            .fetch_optional(pool)
            .await?;
    Ok(row.map(|(n,)| n > 0).unwrap_or(false))
}

#[cfg(test)]
mod tests {
    use super::{insert_mission_revert, mission_was_reverted};
    use crate::repository::fresh_pool;

    #[tokio::test]
    async fn mission_revert_log_inserts_and_dedupes() {
        let pool = fresh_pool().await;
        insert_mission_revert(
            &pool,
            "mid-1",
            "2026-05-22T10:00:00Z",
            "abc123",
            "pre-merge-mid-1-0",
        )
        .await
        .unwrap();
        assert!(mission_was_reverted(&pool, "mid-1").await.unwrap());
        assert!(!mission_was_reverted(&pool, "mid-2").await.unwrap());
        // Idempotent re-insert at same (mission_id, reverted_at):
        insert_mission_revert(
            &pool,
            "mid-1",
            "2026-05-22T10:00:00Z",
            "abc123",
            "pre-merge-mid-1-0",
        )
        .await
        .unwrap();
    }
}
