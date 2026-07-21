//! Phase 2 (V1.2) per-note embedding storage.
//!
//! Companion to [`super::embed`]: this module is where vectors leave
//! / enter the `memory_note_embeddings` SQLite table. It has no
//! opinion about *how* vectors are computed — that's `embed`'s job —
//! so it compiles cleanly whether or not the `embeddings` feature
//! is on. Hybrid scoring (Task 12) can load vectors from here at any
//! time; if the embedder is Disabled or the row is missing, the
//! caller simply falls back to BM25-only.
//!
//! ## BLOB format
//!
//! Raw little-endian `f32` bytes, no header. `serialize` and
//! `deserialize` are mutual inverses; deserialise validates the
//! byte-length is a multiple of 4 and that the resulting dim
//! matches [`super::embed::EMBEDDING_DIM`].

use sqlx::SqlitePool;

use super::embed::EMBEDDING_DIM;
use crate::memory::error::MemoryError;

/// Serialise an f32 vector as little-endian bytes for SQLite BLOB
/// storage. Width is implicit in the vector length — `vec.len() * 4`
/// bytes out.
pub fn serialize(vec: &[f32]) -> Vec<u8> {
    let mut out = Vec::with_capacity(vec.len() * 4);
    for x in vec {
        out.extend_from_slice(&x.to_le_bytes());
    }
    out
}

/// Inverse of [`serialize`]. Returns an error if the byte length
/// isn't a multiple of 4 or if the decoded dimension doesn't match
/// [`EMBEDDING_DIM`] — both indicate row corruption from a model
/// swap that bypassed the [`purge_other_versions`] cleanup.
pub fn deserialize(bytes: &[u8]) -> Result<Vec<f32>, MemoryError> {
    if !bytes.len().is_multiple_of(4) {
        return Err(MemoryError::RowCorrupt(format!(
            "embedding BLOB length {} is not a multiple of 4",
            bytes.len()
        )));
    }
    let dim = bytes.len() / 4;
    if dim != EMBEDDING_DIM {
        return Err(MemoryError::RowCorrupt(format!(
            "embedding BLOB dim {} does not match MODEL_VERSION dim {}",
            dim, EMBEDDING_DIM
        )));
    }
    let mut out = Vec::with_capacity(dim);
    let mut buf = [0u8; 4];
    for chunk in bytes.chunks_exact(4) {
        buf.copy_from_slice(chunk);
        out.push(f32::from_le_bytes(buf));
    }
    Ok(out)
}

/// Upsert a note's embedding under the given `model_version`. Uses
/// `ON CONFLICT(note_id) DO UPDATE` so a re-embed (e.g. after
/// `MODEL_VERSION` bump) replaces the stale vector in one round-trip.
pub async fn store_embedding(
    pool: &SqlitePool,
    note_id: &str,
    vector: &[f32],
    model_version: &str,
) -> Result<(), MemoryError> {
    let blob = serialize(vector);
    let now = crate::ids::rfc3339_now();
    sqlx::query(
        "INSERT INTO memory_note_embeddings \
           (note_id, vector, model_version, computed_at) \
         VALUES (?, ?, ?, ?) \
         ON CONFLICT(note_id) DO UPDATE SET \
           vector = excluded.vector, \
           model_version = excluded.model_version, \
           computed_at = excluded.computed_at",
    )
    .bind(note_id)
    .bind(&blob)
    .bind(model_version)
    .bind(&now)
    .execute(pool)
    .await?;
    Ok(())
}

/// Fetch a note's embedding under the given `model_version`, or
/// `None` if no row exists or the stored row is under a different
/// model. The version filter is critical: a Task 12 hybrid scorer
/// must never blend a query vector from MiniLM with a note vector
/// from `bge-small-en`.
pub async fn get_embedding(
    pool: &SqlitePool,
    note_id: &str,
    model_version: &str,
) -> Result<Option<Vec<f32>>, MemoryError> {
    let row: Option<(Vec<u8>,)> = sqlx::query_as(
        "SELECT vector FROM memory_note_embeddings \
         WHERE note_id = ? AND model_version = ?",
    )
    .bind(note_id)
    .bind(model_version)
    .fetch_optional(pool)
    .await?;
    match row {
        Some((bytes,)) => Ok(Some(deserialize(&bytes)?)),
        None => Ok(None),
    }
}

/// List every promoted note that lacks a current-version embedding.
/// Output is the input the backfill task feeds to the embedder. The
/// `LEFT JOIN ... IS NULL` shape lets SQLite use the
/// `idx_memory_note_embeddings_model` index for the existence probe.
pub async fn list_promoted_without_embedding(
    pool: &SqlitePool,
    model_version: &str,
) -> Result<Vec<String>, MemoryError> {
    let rows: Vec<(String,)> = sqlx::query_as(
        "SELECT n.id \
         FROM memory_notes n \
         LEFT JOIN memory_note_embeddings e \
           ON e.note_id = n.id AND e.model_version = ? \
         WHERE n.state = 'promoted' AND e.note_id IS NULL \
         ORDER BY n.id ASC",
    )
    .bind(model_version)
    .fetch_all(pool)
    .await?;
    Ok(rows.into_iter().map(|(id,)| id).collect())
}

/// Delete every embedding row whose `model_version` differs from
/// `current`. Run once on `MemoryKernel::open` so a bumped
/// `MODEL_VERSION` const triggers a clean backfill instead of
/// silently mixing dimensions.
///
/// Returns the number of rows deleted (for the `tracing::info!`
/// banner the kernel emits when this is non-zero).
pub async fn purge_other_versions(pool: &SqlitePool, current: &str) -> Result<u64, MemoryError> {
    let res = sqlx::query("DELETE FROM memory_note_embeddings WHERE model_version <> ?")
        .bind(current)
        .execute(pool)
        .await?;
    Ok(res.rows_affected())
}

#[cfg(test)]
mod tests {
    use super::*;
    use sqlx::sqlite::SqliteConnectOptions;
    use sqlx::SqlitePool;
    use std::str::FromStr;

    async fn fresh_pool() -> SqlitePool {
        let opts = SqliteConnectOptions::from_str("sqlite::memory:")
            .unwrap()
            .create_if_missing(true);
        let pool = SqlitePool::connect_with(opts).await.unwrap();
        sqlx::migrate!("./migrations").run(&pool).await.unwrap();
        pool
    }

    fn dummy_vec() -> Vec<f32> {
        let mut v = vec![0.0_f32; EMBEDDING_DIM];
        for (i, x) in v.iter_mut().enumerate() {
            *x = (i as f32) / (EMBEDDING_DIM as f32);
        }
        v
    }

    #[test]
    fn serialize_deserialize_round_trip() {
        let v = dummy_vec();
        let bytes = serialize(&v);
        assert_eq!(bytes.len(), EMBEDDING_DIM * 4);
        let back = deserialize(&bytes).unwrap();
        assert_eq!(back, v);
    }

    #[test]
    fn deserialize_rejects_non_multiple_of_four() {
        let bytes = vec![0u8; 7];
        assert!(deserialize(&bytes).is_err());
    }

    #[test]
    fn deserialize_rejects_wrong_dimension() {
        let bytes = vec![0u8; 4 * (EMBEDDING_DIM + 1)];
        let err = deserialize(&bytes).unwrap_err().to_string();
        assert!(err.contains("does not match"), "got: {err}");
    }

    #[tokio::test]
    async fn store_then_get_returns_same_vector() {
        let pool = fresh_pool().await;
        sqlx::query(
            "INSERT INTO memory_notes \
             (id, kind, scope_kind, scope_value, state, body_path, body_hash, created_event_id, created_at) \
             VALUES ('n1', 'fact', 'repo', NULL, 'promoted', 'p', 'h', 'e', '2026-05-22T00:00:00Z')",
        )
        .execute(&pool)
        .await
        .unwrap();
        let v = dummy_vec();
        store_embedding(&pool, "n1", &v, "model-A").await.unwrap();
        let got = get_embedding(&pool, "n1", "model-A").await.unwrap();
        assert_eq!(got, Some(v));
    }

    #[tokio::test]
    async fn get_returns_none_for_different_model_version() {
        let pool = fresh_pool().await;
        sqlx::query(
            "INSERT INTO memory_notes \
             (id, kind, scope_kind, scope_value, state, body_path, body_hash, created_event_id, created_at) \
             VALUES ('n1', 'fact', 'repo', NULL, 'promoted', 'p', 'h', 'e', '2026-05-22T00:00:00Z')",
        )
        .execute(&pool)
        .await
        .unwrap();
        store_embedding(&pool, "n1", &dummy_vec(), "model-A")
            .await
            .unwrap();
        let got = get_embedding(&pool, "n1", "model-B").await.unwrap();
        assert!(
            got.is_none(),
            "version filter must reject cross-model reads"
        );
    }

    #[tokio::test]
    async fn list_promoted_without_embedding_filters_correctly() {
        let pool = fresh_pool().await;
        for (id, state) in &[
            ("p1", "promoted"),
            ("p2", "promoted"),
            ("p3", "promoted"),
            ("o1", "owned"),
        ] {
            sqlx::query(
                "INSERT INTO memory_notes \
                 (id, kind, scope_kind, scope_value, state, body_path, body_hash, created_event_id, created_at) \
                 VALUES (?, 'fact', 'repo', NULL, ?, 'p', 'h', 'e', '2026-05-22T00:00:00Z')",
            )
            .bind(id)
            .bind(state)
            .execute(&pool)
            .await
            .unwrap();
        }
        // p1 has current-version, p2 has old-version, p3 has none.
        store_embedding(&pool, "p1", &dummy_vec(), "current")
            .await
            .unwrap();
        store_embedding(&pool, "p2", &dummy_vec(), "old")
            .await
            .unwrap();
        let pending = list_promoted_without_embedding(&pool, "current")
            .await
            .unwrap();
        assert_eq!(pending, vec!["p2".to_string(), "p3".to_string()]);
    }

    #[tokio::test]
    async fn purge_other_versions_keeps_current_only() {
        let pool = fresh_pool().await;
        for id in &["a", "b", "c"] {
            sqlx::query(
                "INSERT INTO memory_notes \
                 (id, kind, scope_kind, scope_value, state, body_path, body_hash, created_event_id, created_at) \
                 VALUES (?, 'fact', 'repo', NULL, 'promoted', 'p', 'h', 'e', '2026-05-22T00:00:00Z')",
            )
            .bind(id)
            .execute(&pool)
            .await
            .unwrap();
        }
        store_embedding(&pool, "a", &dummy_vec(), "current")
            .await
            .unwrap();
        store_embedding(&pool, "b", &dummy_vec(), "old-1")
            .await
            .unwrap();
        store_embedding(&pool, "c", &dummy_vec(), "old-2")
            .await
            .unwrap();
        let removed = purge_other_versions(&pool, "current").await.unwrap();
        assert_eq!(removed, 2);
        let remaining: Vec<(String,)> =
            sqlx::query_as("SELECT note_id FROM memory_note_embeddings ORDER BY note_id")
                .fetch_all(&pool)
                .await
                .unwrap();
        assert_eq!(remaining, vec![("a".to_string(),)]);
    }

    #[tokio::test]
    async fn cascade_delete_on_note_removes_embedding() {
        let pool = fresh_pool().await;
        sqlx::query("PRAGMA foreign_keys = ON")
            .execute(&pool)
            .await
            .unwrap();
        sqlx::query(
            "INSERT INTO memory_notes \
             (id, kind, scope_kind, scope_value, state, body_path, body_hash, created_event_id, created_at) \
             VALUES ('n1', 'fact', 'repo', NULL, 'promoted', 'p', 'h', 'e', '2026-05-22T00:00:00Z')",
        )
        .execute(&pool)
        .await
        .unwrap();
        store_embedding(&pool, "n1", &dummy_vec(), "m")
            .await
            .unwrap();
        sqlx::query("DELETE FROM memory_notes WHERE id = 'n1'")
            .execute(&pool)
            .await
            .unwrap();
        let got = get_embedding(&pool, "n1", "m").await.unwrap();
        assert!(got.is_none(), "FK cascade must drop the embedding row");
    }
}
