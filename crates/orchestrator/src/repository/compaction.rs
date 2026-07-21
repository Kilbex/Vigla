//! Repository-scoped compaction checkpoints (migration 0019).

/// Get the last compaction run time, or None if never run.
pub(crate) async fn get_last_compaction_run(
    pool: &sqlx::Pool<sqlx::Sqlite>,
    repo_root: &str,
) -> Result<Option<String>, sqlx::Error> {
    let row: Option<(String,)> = sqlx::query_as(
        "SELECT last_run_at FROM snapshot_compaction_state_by_repo WHERE repo_root = ?1",
    )
    .bind(repo_root)
    .fetch_optional(pool)
    .await?;
    Ok(row.map(|(s,)| s))
}

/// Upsert the compaction state row.
pub(crate) async fn upsert_compaction_state(
    pool: &sqlx::Pool<sqlx::Sqlite>,
    repo_root: &str,
    last_run_at: &str,
    last_pruned_count: u32,
) -> Result<(), sqlx::Error> {
    sqlx::query(
        "INSERT INTO snapshot_compaction_state_by_repo
            (repo_root, last_run_at, last_pruned_count)
         VALUES (?1, ?2, ?3)
         ON CONFLICT(repo_root) DO UPDATE
            SET last_run_at = excluded.last_run_at,
                last_pruned_count = excluded.last_pruned_count",
    )
    .bind(repo_root)
    .bind(last_run_at)
    .bind(last_pruned_count as i64)
    .execute(pool)
    .await?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::{get_last_compaction_run, upsert_compaction_state};
    use crate::repository::fresh_pool;

    #[tokio::test]
    async fn compaction_state_upserts() {
        let pool = fresh_pool().await;
        assert!(get_last_compaction_run(&pool, "/repo/a")
            .await
            .unwrap()
            .is_none());
        upsert_compaction_state(&pool, "/repo/a", "2026-05-22T03:00:00Z", 3)
            .await
            .unwrap();
        assert_eq!(
            get_last_compaction_run(&pool, "/repo/a")
                .await
                .unwrap()
                .as_deref(),
            Some("2026-05-22T03:00:00Z")
        );
        assert!(get_last_compaction_run(&pool, "/repo/b")
            .await
            .unwrap()
            .is_none());
        upsert_compaction_state(&pool, "/repo/a", "2026-05-23T03:00:00Z", 5)
            .await
            .unwrap();
        assert_eq!(
            get_last_compaction_run(&pool, "/repo/a")
                .await
                .unwrap()
                .as_deref(),
            Some("2026-05-23T03:00:00Z")
        );
    }
}
