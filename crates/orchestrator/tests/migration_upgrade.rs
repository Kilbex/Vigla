//! Migration upgrade-path test.
//!
//! Every existing persistence test spins the schema up from empty.
//! That misses the catastrophic case: a non-additive migration that
//! drops/renames a column or adds a `NOT NULL` constraint without a
//! default ships, CI passes, and every existing user's DB is wrecked
//! on first launch after upgrade.
//!
//! These tests simulate the upgrade by:
//!   1. Opening a fresh file-backed SQLite database.
//!   2. Applying migrations 1..=N manually (raw SQL + `_sqlx_migrations`
//!      bookkeeping that matches the sqlx 0.8 schema).
//!   3. Seeding representative rows into tables that exist at vN.
//!   4. Closing the pool, reopening through `Repository::open` (which
//!      runs the full migrator — sqlx skips 1..=N by checksum and
//!      applies every migration after N).
//!   5. Asserting the seeded rows survive and that the post-upgrade
//!      typed API still reads them.
//!
//! The covered boundaries are the persistence transitions most likely to
//! damage existing data or silently change uniqueness and foreign-key rules:
//!   * 0005 → 0006: `events` keeps its row count, `events_archive`
//!     materialises empty.
//!   * 0006 → 0007: `audit_reports` is writable through `audit::persist`.
//!   * 0008 → 0009: `vendor_quota_state` loads through
//!     `VendorQuotaTracker::with_pool` without error.
//!   * 0011 → 0012: existing memory notes gain a nullable title.
//!   * 0012 → 0013: note embeddings cascade when their note is deleted.
//!   * 0013 → 0014: duplicate mission audits are deduplicated and blocked.
//!   * 0014 → 0015: existing audit history survives and durable mission
//!     outcomes become writable through the typed repository API.
//!   * 0016 → 0020: legacy outcomes remain readable but have no inferred
//!     repository authority; disposition journals, repository-scoped
//!     compaction checkpoints, and aborted-artifact cleanup markers are
//!     writable.

use orchestrator::recovery::quota::VendorQuotaTracker;
use orchestrator::{DispositionAction, MissionOutcomeState, Repository};
use sqlx::migrate::Migrator;
use sqlx::sqlite::{SqliteConnectOptions, SqlitePoolOptions};
use sqlx::{Executor, SqlitePool};
use tempfile::TempDir;

static MIGRATOR: Migrator = sqlx::migrate!("./migrations");

/// Open a fresh file-backed pool and apply migrations 1..=target_version
/// (inclusive). Records each applied migration in `_sqlx_migrations`
/// with the sqlx 0.8 schema so a subsequent `sqlx::migrate!` call on
/// the same file detects them as already-applied (by version +
/// checksum match) and only runs the remainder.
async fn apply_up_to(path: &std::path::Path, target_version: i64) -> SqlitePool {
    let pool = SqlitePoolOptions::new()
        .max_connections(1)
        .connect_with(
            SqliteConnectOptions::new()
                .filename(path)
                .create_if_missing(true),
        )
        .await
        .expect("open fresh file pool");

    // Mirror sqlx 0.8's _sqlx_migrations schema. The columns and types
    // here must stay in sync with sqlx-core's SqliteMigrator — if a
    // future sqlx bump changes the table shape, this test surfaces the
    // mismatch as a column-count error and the helper must be updated.
    pool.execute(
        "CREATE TABLE IF NOT EXISTS _sqlx_migrations (
            version BIGINT PRIMARY KEY,
            description TEXT NOT NULL,
            installed_on TIMESTAMP NOT NULL DEFAULT CURRENT_TIMESTAMP,
            success BOOLEAN NOT NULL,
            checksum BLOB NOT NULL,
            execution_time BIGINT NOT NULL
        );",
    )
    .await
    .expect("create _sqlx_migrations");

    for migration in MIGRATOR.iter() {
        if migration.version > target_version {
            break;
        }
        // Migration SQL is recorded as a single statement blob; sqlite
        // accepts multi-statement input through `execute(&str)`.
        pool.execute(migration.sql.as_ref())
            .await
            .unwrap_or_else(|e| panic!("apply v{}: {e}", migration.version));

        sqlx::query(
            "INSERT INTO _sqlx_migrations
                 (version, description, success, checksum, execution_time)
             VALUES (?, ?, ?, ?, ?)",
        )
        .bind(migration.version)
        .bind(migration.description.as_ref())
        .bind(true)
        .bind(migration.checksum.as_ref())
        .bind(0_i64)
        .execute(&pool)
        .await
        .unwrap_or_else(|e| panic!("record v{}: {e}", migration.version));
    }

    pool
}

#[tokio::test]
async fn migration_0005_to_0006_preserves_events() {
    let dir = TempDir::new().unwrap();
    let db_path = dir.path().join("upgrade.sqlite");
    let pool = apply_up_to(&db_path, 5).await;

    // Seed against the v0005 schema. At v5: workers has (id, name,
    // vendor, cli_binary, cli_version, cwd, model, spawned_at,
    // ended_at, last_state) plus the resume_metadata columns
    // (session_id, last_prompt). `mission_id` arrives in 0006 — must
    // not be referenced here.
    sqlx::query(
        "INSERT INTO workers
            (id, name, vendor, cli_binary, cwd, spawned_at, last_state)
         VALUES (?, ?, ?, ?, ?, ?, ?)",
    )
    .bind("wkr-c5")
    .bind("c5-worker")
    .bind("claude")
    .bind("claude")
    .bind("/tmp/c5")
    .bind("2026-01-01T00:00:00Z")
    .bind("idle")
    .execute(&pool)
    .await
    .unwrap();

    for seq in 0i64..10 {
        sqlx::query(
            "INSERT INTO events
                (worker_id, seq, ts, type, payload_json, schema_version)
             VALUES (?, ?, ?, ?, ?, ?)",
        )
        .bind("wkr-c5")
        .bind(seq)
        .bind("2026-01-01T00:00:00Z")
        .bind("worker.progress")
        .bind("{}")
        .bind("1")
        .execute(&pool)
        .await
        .unwrap();
    }

    // Close the v5 pool so `Repository::open` can re-attach in WAL
    // mode without contention.
    pool.close().await;

    // Re-open through the production path; the remaining migrations
    // (0006..=0010) run here. If migration 0006 silently moved or
    // dropped rows, the assertions below fire.
    let repo = Repository::open(&db_path).await.expect("upgrade open");

    let workers = repo.list_recent_workers(100).await.unwrap();
    assert!(
        workers.iter().any(|w| w.id == "wkr-c5"),
        "v5 worker row must survive the 0006 upgrade; got {workers:?}"
    );

    let live_count: (i64,) = sqlx::query_as("SELECT COUNT(*) FROM events WHERE worker_id = ?")
        .bind("wkr-c5")
        .fetch_one(repo.pool_for_test())
        .await
        .unwrap();
    assert_eq!(
        live_count.0, 10,
        "no events should be archived by the 0006 migration itself"
    );

    let archive_count: (i64,) = sqlx::query_as("SELECT COUNT(*) FROM events_archive")
        .fetch_one(repo.pool_for_test())
        .await
        .unwrap();
    assert_eq!(
        archive_count.0, 0,
        "events_archive must materialise empty on upgrade"
    );
}

#[tokio::test]
async fn migration_0006_to_0007_audit_history_writable() {
    use orchestrator::audit::persist as audit;
    use orchestrator::audit::report::AuditReport;

    let dir = TempDir::new().unwrap();
    let db_path = dir.path().join("upgrade.sqlite");
    let pool = apply_up_to(&db_path, 6).await;
    pool.close().await;

    let repo = Repository::open(&db_path).await.expect("upgrade open");

    let report = AuditReport {
        overall: 0.81,
        ..Default::default()
    };
    audit::insert_audit(
        repo.pool_for_test(),
        "msn-c5-upgrade",
        Some("wkr-c5"),
        "smoke",
        &report,
    )
    .await
    .expect("insert_audit against upgraded schema");

    let rows = audit::list_audits_for_mission(repo.pool_for_test(), "msn-c5-upgrade")
        .await
        .expect("list_audits_for_mission");
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].tier, "smoke");
    assert!((rows[0].overall - 0.81).abs() < 1e-6);
}

#[tokio::test]
async fn migration_0008_to_0009_quota_state_loadable() {
    let dir = TempDir::new().unwrap();
    let db_path = dir.path().join("upgrade.sqlite");
    let pool = apply_up_to(&db_path, 8).await;
    pool.close().await;

    let repo = Repository::open(&db_path).await.expect("upgrade open");

    let tracker = VendorQuotaTracker::with_pool(repo.pool_for_test().clone())
        .await
        .expect("VendorQuotaTracker::with_pool on upgraded schema");

    assert!(tracker.get(event_schema::Vendor::Claude).await.is_none());
}

#[tokio::test]
async fn migration_0012_adds_title_and_preserves_memory_notes() {
    // F-21: 0012 is a non-additive-looking ALTER (ADD COLUMN). Verify a
    // pre-0012 memory_notes row survives the upgrade and the new nullable
    // `title` reads as NULL for it.
    let dir = TempDir::new().unwrap();
    let db_path = dir.path().join("upgrade.sqlite");
    let pool = apply_up_to(&db_path, 11).await; // pre-`title` schema

    sqlx::query(
        "INSERT INTO memory_notes
            (id, kind, scope_kind, scope_value, body_path, body_hash,
             created_event_id, created_at)
         VALUES (?, 'fact', 'repo', NULL, 'notes/n1.md', 'deadbeef',
                 'evt-1', '2026-01-01T00:00:00.000Z')",
    )
    .bind("note-c5")
    .execute(&pool)
    .await
    .unwrap();
    pool.close().await;

    // Reopen runs 0012 (ALTER ADD COLUMN title) + 0013 + 0014.
    let repo = Repository::open(&db_path).await.expect("upgrade open");
    let row: (String, Option<String>) =
        sqlx::query_as("SELECT id, title FROM memory_notes WHERE id = ?")
            .bind("note-c5")
            .fetch_one(repo.pool_for_test())
            .await
            .expect("v11 note must survive the 0012 ALTER");
    assert_eq!(row.0, "note-c5");
    assert!(
        row.1.is_none(),
        "title must default to NULL for pre-0012 rows"
    );
}

#[tokio::test]
async fn migration_0013_embedding_cascades_on_note_delete() {
    // F-21: 0013 adds memory_note_embeddings with a FK ON DELETE CASCADE.
    // The cascade only fires if PRAGMA foreign_keys is ON — sqlx enables it
    // by default, and this test locks that assumption in.
    let dir = TempDir::new().unwrap();
    let db_path = dir.path().join("upgrade.sqlite");
    let pool = apply_up_to(&db_path, 12).await;
    pool.close().await;
    let repo = Repository::open(&db_path).await.expect("upgrade open"); // runs 0013, 0014
    let p = repo.pool_for_test();

    sqlx::query(
        "INSERT INTO memory_notes
            (id, kind, scope_kind, body_path, body_hash, created_event_id, created_at)
         VALUES ('note-fk', 'fact', 'repo', 'notes/fk.md', 'abc', 'evt-fk',
                 '2026-01-01T00:00:00.000Z')",
    )
    .execute(p)
    .await
    .unwrap();
    sqlx::query(
        "INSERT INTO memory_note_embeddings (note_id, vector, model_version, computed_at)
         VALUES ('note-fk', X'00000000', 'minilm-l6-v2', '2026-01-01T00:00:00.000Z')",
    )
    .execute(p)
    .await
    .unwrap();

    sqlx::query("DELETE FROM memory_notes WHERE id = 'note-fk'")
        .execute(p)
        .await
        .unwrap();
    let count: (i64,) =
        sqlx::query_as("SELECT COUNT(*) FROM memory_note_embeddings WHERE note_id = 'note-fk'")
            .fetch_one(p)
            .await
            .unwrap();
    assert_eq!(
        count.0, 0,
        "embedding must cascade-delete with its note (FK + foreign_keys=ON)"
    );
}

#[tokio::test]
async fn migration_0014_dedupes_and_blocks_duplicate_mission_level_audits() {
    use orchestrator::audit::persist as audit;
    use orchestrator::audit::report::AuditReport;

    // F-9 / F-21: pre-0014 the nullable-PK lets duplicate mission-level audit
    // rows accumulate. 0014 dedupes them and a partial unique index + ON
    // CONFLICT DO NOTHING prevents new ones.
    let dir = TempDir::new().unwrap();
    let db_path = dir.path().join("upgrade.sqlite");
    let pool = apply_up_to(&db_path, 13).await; // audit_reports exists, no partial index

    for _ in 0..2 {
        sqlx::query(
            "INSERT INTO audit_reports
                (mission_id, worker_id, tier, overall, payload_json, created_at)
             VALUES ('msn-dup', NULL, 'smoke', 0.5, '{}', '2026-01-01T00:00:00.000Z')",
        )
        .execute(&pool)
        .await
        .unwrap();
    }
    let before: (i64,) =
        sqlx::query_as("SELECT COUNT(*) FROM audit_reports WHERE mission_id = 'msn-dup'")
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(
        before.0, 2,
        "pre-0014 schema allows duplicate mission-level rows (NULLs distinct in PK)"
    );
    pool.close().await;

    // Reopen runs 0014: dedupe existing + partial unique index.
    let repo = Repository::open(&db_path).await.expect("upgrade open");
    let after: (i64,) =
        sqlx::query_as("SELECT COUNT(*) FROM audit_reports WHERE mission_id = 'msn-dup'")
            .fetch_one(repo.pool_for_test())
            .await
            .unwrap();
    assert_eq!(
        after.0, 1,
        "0014 must dedupe existing mission-level duplicates"
    );

    // A further duplicate mission-level insert is now a silent no-op (ON
    // CONFLICT DO NOTHING against the partial unique index), not a new row or
    // an error.
    let report = AuditReport {
        overall: 0.5,
        ..Default::default()
    };
    audit::insert_audit_at(
        repo.pool_for_test(),
        "msn-dup",
        None,
        "smoke",
        &report,
        "2026-01-01T00:00:00.000Z",
    )
    .await
    .expect("duplicate mission-level insert must be a silent no-op");
    let still_one: (i64,) =
        sqlx::query_as("SELECT COUNT(*) FROM audit_reports WHERE mission_id = 'msn-dup'")
            .fetch_one(repo.pool_for_test())
            .await
            .unwrap();
    assert_eq!(
        still_one.0, 1,
        "ON CONFLICT DO NOTHING must prevent a new duplicate"
    );

    // A worker-level row at the same timestamp is still allowed — the partial
    // index only constrains worker_id IS NULL rows.
    audit::insert_audit_at(
        repo.pool_for_test(),
        "msn-dup",
        Some("wkr-1"),
        "smoke",
        &report,
        "2026-01-01T00:00:00.000Z",
    )
    .await
    .expect("worker-level audit must still insert");
    let with_worker: (i64,) =
        sqlx::query_as("SELECT COUNT(*) FROM audit_reports WHERE mission_id = 'msn-dup'")
            .fetch_one(repo.pool_for_test())
            .await
            .unwrap();
    assert_eq!(with_worker.0, 2);
}

#[tokio::test]
async fn migration_0015_preserves_audits_and_adds_typed_mission_outcomes() {
    let dir = TempDir::new().unwrap();
    let db_path = dir.path().join("upgrade.sqlite");
    let pool = apply_up_to(&db_path, 14).await;

    sqlx::query(
        "INSERT INTO audit_reports
            (mission_id, worker_id, tier, overall, payload_json, created_at)
         VALUES ('msn-outcome', NULL, 'smoke', 0.93, '{}',
                 '2026-07-21T12:00:00.000Z')",
    )
    .execute(&pool)
    .await
    .unwrap();
    pool.close().await;

    let repo = Repository::open(&db_path).await.expect("upgrade open");
    let audit_count: (i64,) =
        sqlx::query_as("SELECT COUNT(*) FROM audit_reports WHERE mission_id = 'msn-outcome'")
            .fetch_one(repo.pool_for_test())
            .await
            .unwrap();
    assert_eq!(
        audit_count.0, 1,
        "0015 must not alter existing audit history"
    );

    repo.record_mission_outcome(
        "msn-outcome",
        "/repo/upgrade",
        "main",
        MissionOutcomeState::Merged,
        "2026-07-21T12:01:00.000Z",
    )
    .await
    .expect("typed mission outcome insert against upgraded schema");
    let outcome = repo
        .mission_outcome("msn-outcome")
        .await
        .expect("typed mission outcome lookup")
        .expect("inserted outcome");
    assert_eq!(outcome.mission_id, "msn-outcome");
    assert_eq!(outcome.repo_root.as_deref(), Some("/repo/upgrade"));
    assert_eq!(outcome.target_ref, "main");
    assert_eq!(outcome.state, MissionOutcomeState::Merged);
    assert_eq!(outcome.updated_at, "2026-07-21T12:01:00.000Z");
}

#[tokio::test]
async fn migrations_0017_through_0020_preserve_outcomes_and_scope_cleanup() {
    let dir = TempDir::new().unwrap();
    let db_path = dir.path().join("upgrade.sqlite");
    let pool = apply_up_to(&db_path, 16).await;
    sqlx::query(
        "INSERT INTO mission_outcomes (mission_id, target_ref, state, updated_at)
         VALUES ('legacy-mission', 'main', 'merged', '2026-07-21T12:00:00Z')",
    )
    .execute(&pool)
    .await
    .unwrap();
    pool.close().await;

    let repo = Repository::open(&db_path).await.expect("upgrade open");
    let legacy = repo
        .mission_outcome("legacy-mission")
        .await
        .unwrap()
        .unwrap();
    assert_eq!(legacy.state, MissionOutcomeState::Merged);
    assert_eq!(
        legacy.repo_root, None,
        "migration must not fabricate rollback authority for a legacy row"
    );

    repo.record_disposition_intent(
        "pending-mission",
        "/repo/canonical",
        "release/v1",
        DispositionAction::Discard,
        "2026-07-21T12:01:00Z",
    )
    .await
    .expect("journal is writable after upgrade");
    let intents = repo.list_disposition_intents().await.unwrap();
    assert_eq!(intents.len(), 1);
    assert_eq!(intents[0].repo_root, "/repo/canonical");
    assert_eq!(intents[0].action, DispositionAction::Discard);

    repo.record_compaction_run("/repo/a", "2026-07-21T13:00:00Z", 3)
        .await
        .unwrap();
    assert_eq!(
        repo.last_compaction_run("/repo/a")
            .await
            .unwrap()
            .as_deref(),
        Some("2026-07-21T13:00:00Z")
    );
    assert_eq!(repo.last_compaction_run("/repo/b").await.unwrap(), None);

    repo.record_mission_outcome(
        "aborted-mission",
        "/repo/canonical",
        "main",
        MissionOutcomeState::Aborted,
        "2026-07-21T14:00:00Z",
    )
    .await
    .unwrap();
    repo.record_mission_cleanup("aborted-mission", "/repo/canonical", "2026-07-21T14:01:00Z")
        .await
        .expect("artifact cleanup marker is writable after upgrade");
    assert!(repo
        .mission_artifacts_cleaned("aborted-mission")
        .await
        .unwrap());
}
