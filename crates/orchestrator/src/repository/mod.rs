//! SQLite-backed persistence for canonical worker events.
//!
//! The single source of truth for canonical events. The repository
//! stores envelope fields as columns and the event payload as opaque
//! JSON, so unknown fields and even unknown event types survive
//! replay.
//!
//! The supervision pipeline owns writes; replay, history, compaction, and
//! mission-revert queries expose the read models needed by the host.

use crate::error::RepositoryError;
use event_schema::{
    Event, EventKind, Log, LogLevel, LogStream, TaskInfo, Vendor, WorkerInfo, WorkerState,
};
use sqlx::sqlite::{SqliteConnectOptions, SqlitePoolOptions, SqliteRow};
use sqlx::{Row, SqlitePool};
use std::path::Path;
use std::str::FromStr;
use std::time::Duration;

mod compaction;
mod disposition_journal;
mod history;
mod mission_cleanup;
mod mission_outcomes;
mod revert_log;

pub use disposition_journal::{DispositionAction, DispositionIntentDto};
pub use history::{MissionHistoryDto, MissionHistoryStatus};
pub use mission_outcomes::{MissionOutcomeDto, MissionOutcomeState};

/// Maximum simultaneous connections held by the file-backed pool. SQLite
/// serialises writes anyway; the pool size only matters for read
/// concurrency. 5 covers the orchestrator + UI replay queries with room.
const FILE_POOL_MAX_CONNECTIONS: u32 = 5;

/// Upper bound on how long a caller waits for a pool slot before
/// surfacing a `SQLITE_BUSY`-style error. Without this, every Tauri
/// IPC command (insert_event / mark_worker_ended / list_recent_workers /
/// replay_worker_events_page / record_mission_revert / …) could block
/// indefinitely when the retention sweeper and a UI replay query
/// contend on the writer lock, manifesting as buttons that never
/// respond. Pair with [`SQLITE_BUSY_TIMEOUT`] which lets SQLite itself
/// retry briefly inside the kernel before giving up.
const POOL_ACQUIRE_TIMEOUT: Duration = Duration::from_secs(5);

/// Per-connection SQLite busy timeout. The kernel default is 0 ms — a
/// second writer returns `SQLITE_BUSY` immediately, which sqlx then
/// surfaces as a pool-acquire wait. 3 s lets normal short-write
/// contention resolve without bubbling out to the caller.
const SQLITE_BUSY_TIMEOUT: Duration = Duration::from_secs(3);

/// Default per-worker live-tier cap. Events beyond this are moved to
/// `events_archive` by the retention triggers. Override via the
/// `VIGLA_EVENTS_PER_WORKER_CAP` environment variable (read once
/// at `Repository::open`).
const EVENTS_PER_WORKER_LIVE_CAP_DEFAULT: u64 = 50_000;

/// Server-side ceiling on `replay_for_worker_page` / `replay_for_task_page`
/// page size. The Tauri command surface is the most likely source of
/// a runaway request; clamping here keeps one round-trip bounded
/// regardless of caller behaviour.
const MAX_REPLAY_PAGE: u32 = 10_000;

fn live_cap_from_env() -> u64 {
    std::env::var("VIGLA_EVENTS_PER_WORKER_CAP")
        .ok()
        .and_then(|s| s.parse::<u64>().ok())
        .filter(|&v| v > 0)
        .unwrap_or(EVENTS_PER_WORKER_LIVE_CAP_DEFAULT)
}

/// Default retention sweeper tick interval. Override via
/// `VIGLA_RETENTION_TICK_SECS` (positive integer). Read once at
/// `RetentionGuard::spawn`.
const RETENTION_TICK_DEFAULT_SECS: u64 = 60;

pub(crate) fn retention_tick_from_env() -> std::time::Duration {
    let secs = std::env::var("VIGLA_RETENTION_TICK_SECS")
        .ok()
        .and_then(|s| s.parse::<u64>().ok())
        .filter(|&v| v > 0)
        .unwrap_or(RETENTION_TICK_DEFAULT_SECS);
    std::time::Duration::from_secs(secs)
}

/// Outcome of [`Repository::insert_event`] / [`Repository::insert_event_raw`].
/// The event-log contract mandates that consumers detect seq
/// regressions and mark them as warnings — the previous "INSERT OR
/// IGNORE" path silently swallowed them. Callers can now distinguish
/// the two cases and (for example) skip emitting a duplicate to the UI
/// or surface a synthetic warning event.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum InsertOutcome {
    /// New row added.
    Inserted,
    /// `(worker_id, seq)` already exists — schema §6 anomaly. The
    /// duplicate is logged via stderr inside the repository; the
    /// caller decides what to do downstream.
    DuplicateSkipped,
}

/// Step 25 — resume metadata for a worker. Returned by
/// [`Repository::get_resume_metadata`] and used by the supervisor to
/// resume a session or retry with the last prompt.
#[derive(Debug, Clone)]
pub struct ResumeMetadata {
    pub vendor: Vendor,
    pub cwd: String,
    pub session_id: Option<String>,
    pub last_prompt: Option<String>,
    pub model: Option<String>,
}

/// Handle to the persistent event store. Cheap to clone — the
/// underlying [`SqlitePool`] is `Arc`-wrapped internally.
#[derive(Debug, Clone)]
pub struct Repository {
    pool: SqlitePool,
    live_cap: u64,
}

impl Repository {
    /// Open (or create) the SQLite database at `path` and apply
    /// pending migrations. Parent directories are created on demand.
    pub async fn open(path: &Path) -> Result<Self, RepositoryError> {
        if let Some(parent) = path.parent() {
            if !parent.as_os_str().is_empty() {
                std::fs::create_dir_all(parent)?;
            }
        }

        let options = SqliteConnectOptions::new()
            .filename(path)
            .create_if_missing(true)
            .journal_mode(sqlx::sqlite::SqliteJournalMode::Wal)
            .synchronous(sqlx::sqlite::SqliteSynchronous::Normal)
            .busy_timeout(SQLITE_BUSY_TIMEOUT);

        let pool = SqlitePoolOptions::new()
            .max_connections(FILE_POOL_MAX_CONNECTIONS)
            .acquire_timeout(POOL_ACQUIRE_TIMEOUT)
            .connect_with(options)
            .await?;

        sqlx::migrate!("./migrations").run(&pool).await?;

        Ok(Self {
            pool,
            live_cap: live_cap_from_env(),
        })
    }

    /// Clone the underlying connection pool. Cheap — `SqlitePool` is
    /// `Arc`-backed. Lets the host build the persistent
    /// `VendorQuotaTracker` on the *same* database (sharing the migrated
    /// `vendor_quota_state` table) without opening a second pool.
    pub fn pool(&self) -> SqlitePool {
        self.pool.clone()
    }

    /// Persist one worker- or mission-level quality audit at the timestamp of
    /// its source mission event. Keeping this behind `Repository` prevents
    /// hosts from reaching into the SQL pool or duplicating conflict policy.
    pub async fn record_audit_at(
        &self,
        mission_id: &str,
        worker_id: Option<&str>,
        tier: &str,
        report: &crate::audit::AuditReport,
        created_at: &str,
    ) -> Result<(), RepositoryError> {
        crate::audit::persist::insert_audit_at(
            &self.pool, mission_id, worker_id, tier, report, created_at,
        )
        .await?;
        Ok(())
    }

    /// Record the mission's terminal user disposition. Replaying the same
    /// terminal event is idempotent; a conflicting terminal event is rejected.
    pub async fn record_mission_outcome(
        &self,
        mission_id: &str,
        repo_root: &str,
        target_ref: &str,
        state: MissionOutcomeState,
        updated_at: &str,
    ) -> Result<(), RepositoryError> {
        mission_outcomes::record_mission_outcome_impl(
            &self.pool, mission_id, repo_root, target_ref, state, updated_at,
        )
        .await
    }

    /// Return the durable terminal disposition for one mission, if recorded.
    pub async fn mission_outcome(
        &self,
        mission_id: &str,
    ) -> Result<Option<MissionOutcomeDto>, RepositoryError> {
        mission_outcomes::mission_outcome_impl(&self.pool, mission_id).await
    }

    /// Record that an aborted mission's retained Git artifacts were removed.
    /// The repository enforces both the aborted state and exact repository
    /// identity; replaying the same completion is idempotent.
    pub async fn record_mission_cleanup(
        &self,
        mission_id: &str,
        repo_root: &str,
        cleaned_at: &str,
    ) -> Result<(), RepositoryError> {
        mission_cleanup::record_cleanup_impl(&self.pool, mission_id, repo_root, cleaned_at).await
    }

    /// Whether explicit cleanup has completed for an aborted mission.
    pub async fn mission_artifacts_cleaned(
        &self,
        mission_id: &str,
    ) -> Result<bool, RepositoryError> {
        mission_cleanup::mission_was_cleaned_impl(&self.pool, mission_id).await
    }

    /// Persist a terminal disposition before any Git mutation. Repeating the
    /// exact intent is idempotent; changing its repository, target, or action
    /// is rejected.
    pub async fn record_disposition_intent(
        &self,
        mission_id: &str,
        repo_root: &str,
        target_ref: &str,
        action: DispositionAction,
        created_at: &str,
    ) -> Result<(), RepositoryError> {
        disposition_journal::record_intent_impl(
            &self.pool, mission_id, repo_root, target_ref, action, created_at,
        )
        .await
    }

    /// Return unresolved disposition intents for startup reconciliation.
    pub async fn list_disposition_intents(
        &self,
    ) -> Result<Vec<DispositionIntentDto>, RepositoryError> {
        disposition_journal::list_intents_impl(&self.pool).await
    }

    /// Clear an intent after its outcome and cleanup are durable.
    pub async fn clear_disposition_intent(&self, mission_id: &str) -> Result<(), RepositoryError> {
        disposition_journal::clear_intent_impl(&self.pool, mission_id).await
    }

    /// Open an in-memory database. Useful for fast tests; not suitable
    /// for production. SQLx allocates a unique named shared-cache URI
    /// per call, so multiple connections in the same pool all see the
    /// same in-memory database. We keep `min_connections(1)` so the DB
    /// isn't torn down when idle, and allow up to 4 concurrent
    /// connections (enough for tests that clone the repo into a
    /// background task, like `RetentionGuard`).
    pub async fn open_in_memory() -> Result<Self, RepositoryError> {
        let options =
            SqliteConnectOptions::from_str("sqlite::memory:")?.busy_timeout(SQLITE_BUSY_TIMEOUT);

        let pool = SqlitePoolOptions::new()
            .min_connections(1)
            .max_connections(4)
            .acquire_timeout(POOL_ACQUIRE_TIMEOUT)
            .idle_timeout(None)
            .max_lifetime(None)
            .connect_with(options)
            .await?;

        sqlx::migrate!("./migrations").run(&pool).await?;

        Ok(Self {
            pool,
            live_cap: live_cap_from_env(),
        })
    }

    // A4 (Tier-2G): the `pool()` accessor used to be `pub` so the
    // host crate could share the connection set with the global
    // `MemoryKernel`. A2 retired that path — each repo now opens its
    // own `<repo>/.vigla/memory/memory.sqlite`. With zero
    // production callers the accessor is gone; reintroducing it
    // requires the new caller to live INSIDE `orchestrator/src/` and
    // explicitly re-add a `pub(crate) fn pool(...)`. The
    // `no_sql_outside_orchestrator` integration test enforces the
    // crate-boundary half of the discipline.

    /// Persist a worker identity record. Errors if the id already
    /// exists.
    pub async fn insert_worker(&self, worker: &WorkerInfo) -> Result<(), RepositoryError> {
        let vendor_str = serde_json::to_value(worker.vendor)?
            .as_str()
            .ok_or_else(|| {
                RepositoryError::RowCorrupt("vendor did not serialize as string".into())
            })?
            .to_owned();

        sqlx::query(
            "INSERT INTO workers \
             (id, name, vendor, cli_binary, cli_version, cwd, model, spawned_at, ended_at) \
             VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?)",
        )
        .bind(&worker.id)
        .bind(&worker.name)
        .bind(&vendor_str)
        .bind(&worker.cli_binary)
        .bind(worker.cli_version.as_deref())
        .bind(&worker.cwd)
        .bind(worker.model.as_deref())
        .bind(&worker.spawned_at)
        .bind(worker.ended_at.as_deref())
        .execute(&self.pool)
        .await?;

        Ok(())
    }

    /// Persist a task identity record.
    pub async fn insert_task(&self, task: &TaskInfo) -> Result<(), RepositoryError> {
        let depends_on_json = serde_json::to_string(&task.depends_on)?;

        sqlx::query(
            "INSERT INTO tasks (id, parent_id, title, depends_on_json, created_at) \
             VALUES (?, ?, ?, ?, ?)",
        )
        .bind(&task.id)
        .bind(task.parent_id.as_deref())
        .bind(&task.title)
        .bind(&depends_on_json)
        .bind(&task.created_at)
        .execute(&self.pool)
        .await?;

        Ok(())
    }

    /// Insert a worker row and a task row in a single transaction. The
    /// supervisor's spawn paths create both records up-front; if the
    /// task insert fails, the worker insert rolls back so callers never
    /// see an orphan worker row in `list_recent_workers`.
    pub async fn insert_worker_and_task(
        &self,
        worker: &WorkerInfo,
        task: &TaskInfo,
    ) -> Result<(), RepositoryError> {
        let vendor_str = serde_json::to_value(worker.vendor)?
            .as_str()
            .ok_or_else(|| {
                RepositoryError::RowCorrupt("vendor did not serialize as string".into())
            })?
            .to_owned();
        let depends_on_json = serde_json::to_string(&task.depends_on)?;

        let mut tx = self.pool.begin().await?;

        sqlx::query(
            "INSERT INTO workers \
             (id, name, vendor, cli_binary, cli_version, cwd, model, spawned_at, ended_at) \
             VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?)",
        )
        .bind(&worker.id)
        .bind(&worker.name)
        .bind(&vendor_str)
        .bind(&worker.cli_binary)
        .bind(worker.cli_version.as_deref())
        .bind(&worker.cwd)
        .bind(worker.model.as_deref())
        .bind(&worker.spawned_at)
        .bind(worker.ended_at.as_deref())
        .execute(&mut *tx)
        .await?;

        sqlx::query(
            "INSERT INTO tasks (id, parent_id, title, depends_on_json, created_at) \
             VALUES (?, ?, ?, ?, ?)",
        )
        .bind(&task.id)
        .bind(task.parent_id.as_deref())
        .bind(&task.title)
        .bind(&depends_on_json)
        .bind(&task.created_at)
        .execute(&mut *tx)
        .await?;

        tx.commit().await?;
        Ok(())
    }

    /// Roll back identity rows created for a child process that never became
    /// runnable. Callers invoke this only before event supervision starts.
    /// The `NOT EXISTS` guards make the operation fail closed if a future
    /// caller accidentally tries to erase a worker with durable events.
    pub async fn rollback_worker_spawn(
        &self,
        worker_id: &str,
        task_id: Option<&str>,
        remove_task: bool,
    ) -> Result<(), RepositoryError> {
        let mut tx = self.pool.begin().await?;
        sqlx::query(
            "DELETE FROM workers
             WHERE id = ?1
               AND NOT EXISTS (SELECT 1 FROM events WHERE worker_id = ?1)",
        )
        .bind(worker_id)
        .execute(&mut *tx)
        .await?;
        if remove_task {
            if let Some(task_id) = task_id {
                sqlx::query(
                    "DELETE FROM tasks
                     WHERE id = ?1
                       AND NOT EXISTS (SELECT 1 FROM events WHERE task_id = ?1)",
                )
                .bind(task_id)
                .execute(&mut *tx)
                .await?;
            }
        }
        tx.commit().await?;
        Ok(())
    }

    /// Persist a single canonical event. Returns `DuplicateSkipped`
    /// when a row with the same `(worker_id, seq)` already exists —
    /// schema §6 calls those seq regressions and requires they be
    /// marked as warnings (we log to stderr; callers can also choose
    /// not to re-emit the duplicate to downstream sinks).
    pub async fn insert_event(&self, event: &Event) -> Result<InsertOutcome, RepositoryError> {
        let kind_value = serde_json::to_value(&event.kind)?;
        let type_str = kind_value
            .get("type")
            .and_then(|v| v.as_str())
            .ok_or_else(|| RepositoryError::RowCorrupt("event kind missing 'type' field".into()))?
            .to_owned();
        let payload_value = kind_value
            .get("payload")
            .cloned()
            .unwrap_or(serde_json::Value::Null);
        let payload_json = serde_json::to_string(&payload_value)?;

        let outcome = self
            .insert_event_raw(
                &event.worker_id,
                event.task_id.as_deref(),
                event.seq,
                &event.ts,
                &type_str,
                &payload_json,
                &event.schema_version,
            )
            .await?;
        if matches!(outcome, InsertOutcome::Inserted) {
            if let EventKind::Cost(cost) = &event.kind {
                if let Some(model) = cost.model.as_deref().filter(|m| !m.trim().is_empty()) {
                    if let Err(e) = self.set_worker_model(&event.worker_id, Some(model)).await {
                        tracing::warn!(
                            worker_id = %event.worker_id,
                            model = %model,
                            error = %e,
                            "failed to persist observed worker model"
                        );
                    }
                }
            }
        }
        Ok(outcome)
    }

    /// Persist the most recently observed/requested model for a worker.
    ///
    /// `None` clears the field. A missing worker is a no-op because
    /// mission-scoped canonical events may arrive before mission-worker
    /// rows exist in the standalone worker table.
    pub async fn set_worker_model(
        &self,
        worker_id: &str,
        model: Option<&str>,
    ) -> Result<(), RepositoryError> {
        sqlx::query("UPDATE workers SET model = ? WHERE id = ?")
            .bind(model)
            .bind(worker_id)
            .execute(&self.pool)
            .await?;
        Ok(())
    }

    /// Persist an event from pre-decomposed envelope fields and an
    /// opaque payload JSON string. Used by Step 5 supervision to
    /// honour the event-log contract ("persist every received
    /// event verbatim, including unknown event types and unknown
    /// fields") even when typed parsing fails.
    pub async fn insert_event_raw(
        &self,
        worker_id: &str,
        task_id: Option<&str>,
        seq: u64,
        ts: &str,
        event_type: &str,
        payload_json: &str,
        schema_version: &str,
    ) -> Result<InsertOutcome, RepositoryError> {
        let result = sqlx::query(
            "INSERT INTO events \
             (worker_id, task_id, seq, ts, type, payload_json, schema_version) \
             VALUES (?, ?, ?, ?, ?, ?, ?)",
        )
        .bind(worker_id)
        .bind(task_id)
        .bind(seq as i64)
        .bind(ts)
        .bind(event_type)
        .bind(payload_json)
        .bind(schema_version)
        .execute(&self.pool)
        .await;

        match result {
            Ok(_) => Ok(InsertOutcome::Inserted),
            Err(sqlx::Error::Database(db_err)) if db_err.is_unique_violation() => {
                tracing::error!(
                    "orchestrator: seq regression — worker {worker_id} \
                     seq {seq} already persisted (event-log contract)"
                );
                Ok(InsertOutcome::DuplicateSkipped)
            }
            Err(e) => Err(RepositoryError::Sqlx(e)),
        }
    }

    /// Mirror the worker's most recently-known lifecycle state into
    /// the `workers.last_state` column. Cheap; no-op if the worker id
    /// doesn't exist.
    pub async fn update_worker_state(
        &self,
        worker_id: &str,
        state: WorkerState,
    ) -> Result<(), RepositoryError> {
        let state_str = serde_json::to_value(state)?
            .as_str()
            .ok_or_else(|| {
                RepositoryError::RowCorrupt("worker state did not serialize as string".into())
            })?
            .to_owned();

        sqlx::query(
            "UPDATE workers SET last_state = ?1
             WHERE id = ?2
               AND (?1 IN ('done', 'failed') OR last_state NOT IN ('done', 'failed'))",
        )
        .bind(&state_str)
        .bind(worker_id)
        .execute(&self.pool)
        .await?;

        Ok(())
    }

    /// Expose the configured live-tier cap so [`RetentionGuard`] can
    /// read the same value `mark_worker_ended` uses for inline trim.
    pub(crate) fn live_cap(&self) -> u64 {
        self.live_cap
    }

    /// Mark a worker as ended (sets `ended_at`). Called by the
    /// supervisor when the child process exits. Also runs an inline
    /// archive trim so a finished worker's live-tier footprint collapses
    /// to at most `live_cap` rows.
    pub async fn mark_worker_ended(
        &self,
        worker_id: &str,
        ended_at: &str,
    ) -> Result<(), RepositoryError> {
        sqlx::query("UPDATE workers SET ended_at = ? WHERE id = ?")
            .bind(ended_at)
            .bind(worker_id)
            .execute(&self.pool)
            .await?;

        // Inline trim. A worker that just stopped emitting can be
        // shrunk immediately; we don't wait for the next RetentionGuard
        // tick. Errors are logged but do not fail the call — the worker
        // is already marked ended, and the periodic sweeper will retry.
        if let Err(e) = self
            .archive_excess_for_worker(worker_id, self.live_cap)
            .await
        {
            tracing::error!("orchestrator: archive trim failed for {worker_id} on mark_ended: {e}");
        }
        Ok(())
    }

    /// Step 25 — set the session_id for a worker (called once
    /// after the adapter emits it via `take_session_id`).
    pub async fn set_session_id(
        &self,
        worker_id: &str,
        session_id: &str,
    ) -> Result<(), RepositoryError> {
        sqlx::query("UPDATE workers SET session_id = ? WHERE id = ?")
            .bind(session_id)
            .bind(worker_id)
            .execute(&self.pool)
            .await?;

        Ok(())
    }

    /// Step 25 — set the last_prompt for a worker (called each
    /// time a worker is spawned or resumed, to capture the most
    /// recent prompt for potential retry).
    pub async fn set_last_prompt(
        &self,
        worker_id: &str,
        prompt: &str,
    ) -> Result<(), RepositoryError> {
        sqlx::query("UPDATE workers SET last_prompt = ? WHERE id = ?")
            .bind(prompt)
            .bind(worker_id)
            .execute(&self.pool)
            .await?;

        Ok(())
    }

    /// Step 25 — retrieve resume metadata (session_id, last_prompt,
    /// vendor, cwd) for a worker. Returns an error if the worker
    /// doesn't exist.
    pub async fn get_resume_metadata(
        &self,
        worker_id: &str,
    ) -> Result<ResumeMetadata, RepositoryError> {
        let row = sqlx::query(
            "SELECT vendor, cwd, session_id, last_prompt, model \
             FROM workers WHERE id = ?",
        )
        .bind(worker_id)
        .fetch_optional(&self.pool)
        .await?;

        let row = row.ok_or_else(|| {
            RepositoryError::RowCorrupt(format!("worker {} not found", worker_id))
        })?;

        let vendor_str: String = row.try_get("vendor")?;
        let vendor: Vendor = serde_json::from_value(serde_json::Value::String(vendor_str.clone()))
            .map_err(|_| RepositoryError::RowCorrupt(format!("unknown vendor {vendor_str:?}")))?;

        Ok(ResumeMetadata {
            vendor,
            cwd: row.try_get("cwd")?,
            session_id: row.try_get("session_id")?,
            last_prompt: row.try_get("last_prompt")?,
            model: row.try_get("model")?,
        })
    }

    /// Batch 2 — get full WorkerInfo for a specific worker by ID.
    pub async fn get_worker_info_by_id(
        &self,
        worker_id: &str,
    ) -> Result<WorkerInfo, RepositoryError> {
        let row = sqlx::query(
            "SELECT id, name, vendor, cli_binary, cli_version, cwd, model, \
             spawned_at, ended_at FROM workers WHERE id = ?",
        )
        .bind(worker_id)
        .fetch_optional(&self.pool)
        .await?;

        let row = row.ok_or_else(|| {
            RepositoryError::RowCorrupt(format!("worker {} not found", worker_id))
        })?;

        row_to_worker_info(&row)
    }

    /// Step 14 — list recent workers, most recent first.
    /// `limit` caps the number of records returned.
    ///
    /// Rows that no longer decode under the current schema (e.g. a
    /// vendor that was removed in a major schema bump — `aider` in
    /// 2.0) are skipped with a stderr warning rather than failing the
    /// whole query. The user's history view stays usable across schema
    /// evolutions; orphaned rows remain in the DB for forensic
    /// inspection but never surface to the UI.
    pub async fn list_recent_workers(
        &self,
        limit: u32,
    ) -> Result<Vec<WorkerInfo>, RepositoryError> {
        let rows = sqlx::query(
            "SELECT id, name, vendor, cli_binary, cli_version, cwd, model, \
                    spawned_at, ended_at \
             FROM workers ORDER BY spawned_at DESC, id DESC LIMIT ?",
        )
        .bind(limit as i64)
        .fetch_all(&self.pool)
        .await?;

        let mut out = Vec::with_capacity(rows.len());
        for row in &rows {
            match row_to_worker_info(row) {
                Ok(info) => out.push(info),
                Err(e) => {
                    let id: String = row.try_get("id").unwrap_or_else(|_| "<unknown>".into());
                    tracing::error!(
                        "[repository] list_recent_workers: skipping unreadable row id={id}: {e}"
                    );
                }
            }
        }
        Ok(out)
    }

    /// Largest `seq` already persisted for `worker_id`, or `None` when
    /// the worker has no events yet. Used by `spawn_claude_resume` to
    /// seed the resumed adapter past the original run's high-water
    /// mark — otherwise the (worker_id, seq) PRIMARY KEY collides on
    /// every event and `insert_event_raw` silently drops the resumed
    /// stream.
    pub async fn max_seq_for_worker(
        &self,
        worker_id: &str,
    ) -> Result<Option<u64>, RepositoryError> {
        // SQLite's MAX over zero rows is NULL but the row still comes
        // back from `fetch_one`. Decode straight into `Option<i64>` so
        // NULL becomes `None`; decoding into `i64` would happily round
        // NULL to `0` and the caller couldn't distinguish "no events"
        // from "first event already at seq 0".
        let row = sqlx::query("SELECT MAX(seq) AS max_seq FROM events WHERE worker_id = ?")
            .bind(worker_id)
            .fetch_one(&self.pool)
            .await?;
        let max: Option<i64> = row.try_get("max_seq")?;
        // seq is unsigned per schema §1; clamp the negative space away
        // defensively even though the column is populated from u64.
        Ok(max.map(|v| v.max(0) as u64))
    }

    /// Move all but the most-recent `keep_recent` events for `worker_id`
    /// from `events` into `events_archive`. Returns the number of rows
    /// archived.
    ///
    /// Safe under concurrent `insert_event_raw` calls: SQLite WAL
    /// serialises writes and the trim is bounded to
    /// `seq <= max_seq - keep_recent`, so any event arriving mid-trim
    /// sits outside the trim window. Two concurrent trim calls on the
    /// same `worker_id` are not expected and may collide on
    /// `SQLITE_BUSY`; the production triggers (`mark_worker_ended`
    /// inline + the periodic `RetentionGuard`) do not fan out on the
    /// same worker.
    ///
    /// Hot path note: this is NEVER called from `insert_event_raw`.
    /// Inserts must not block on a trim transaction. Trigger points are
    /// `mark_worker_ended` (inline) and `RetentionGuard` (periodic).
    pub async fn archive_excess_for_worker(
        &self,
        worker_id: &str,
        keep_recent: u64,
    ) -> Result<u64, RepositoryError> {
        self.archive_excess_one(worker_id, keep_recent).await
    }

    /// Trim one worker inside its own `BEGIN IMMEDIATE` write transaction.
    ///
    /// IMMEDIATE takes the WAL write lock up front. The trim reads
    /// `MAX(seq)` before it writes; under a plain *deferred* `BEGIN` that
    /// read takes a snapshot and the later INSERT must upgrade it to a
    /// write — if a foreground `insert_event_raw` committed in between,
    /// SQLite refuses the upgrade with `SQLITE_BUSY_SNAPSHOT` *immediately*
    /// (the busy handler is bypassed for snapshot conflicts, so the 3 s
    /// `busy_timeout` never applies) and the sweep loses the row race.
    /// Grabbing the write lock first makes contention wait on `busy_timeout`
    /// and retry instead — which is what the "safe under concurrent
    /// insert_event_raw" contract above requires.
    async fn archive_excess_one(
        &self,
        worker_id: &str,
        keep_recent: u64,
    ) -> Result<u64, RepositoryError> {
        let mut conn = self.pool.acquire().await?;
        sqlx::query("BEGIN IMMEDIATE").execute(&mut *conn).await?;
        match Self::archive_excess_in_tx(&mut conn, worker_id, keep_recent).await {
            Ok(moved) => {
                sqlx::query("COMMIT").execute(&mut *conn).await?;
                Ok(moved)
            }
            Err(e) => {
                // Roll back so the pooled connection is never returned
                // mid-transaction. `archive_excess_in_tx` has no panic
                // paths (every fallible call uses `?`), so this arm covers
                // every non-success outcome.
                let _ = sqlx::query("ROLLBACK").execute(&mut *conn).await;
                Err(e)
            }
        }
    }

    /// Inner helper shared by [`archive_excess_for_worker`] and
    /// [`archive_excess_for_all`] — runs the trim on a connection whose
    /// caller has already opened a `BEGIN IMMEDIATE` transaction.
    async fn archive_excess_in_tx(
        conn: &mut sqlx::SqliteConnection,
        worker_id: &str,
        keep_recent: u64,
    ) -> Result<u64, RepositoryError> {
        // Highest seq currently in the live tier.
        let max_seq_opt: Option<i64> =
            sqlx::query_scalar("SELECT MAX(seq) FROM events WHERE worker_id = ?")
                .bind(worker_id)
                .fetch_one(&mut *conn)
                .await?;
        let Some(max_seq) = max_seq_opt.map(|v| v.max(0) as u64) else {
            // No events for this worker.
            return Ok(0);
        };
        if max_seq < keep_recent {
            return Ok(0);
        }
        let threshold = (max_seq - keep_recent) as i64;
        let archived_at = crate::ids::rfc3339_now();

        let moved = sqlx::query(
            "INSERT INTO events_archive \
             (worker_id, task_id, seq, ts, type, payload_json, schema_version, archived_at) \
             SELECT worker_id, task_id, seq, ts, type, payload_json, schema_version, ? \
             FROM events WHERE worker_id = ? AND seq <= ?",
        )
        .bind(&archived_at)
        .bind(worker_id)
        .bind(threshold)
        .execute(&mut *conn)
        .await?
        .rows_affected();

        sqlx::query("DELETE FROM events WHERE worker_id = ? AND seq <= ?")
            .bind(worker_id)
            .bind(threshold)
            .execute(&mut *conn)
            .await?;

        Ok(moved)
    }

    /// Sweep every worker that holds more than `keep_recent` events in
    /// the live tier. Returns archived row counts keyed by worker id;
    /// workers that needed no work are absent from the map.
    ///
    /// Used by [`RetentionGuard`] for periodic background trim.
    ///
    /// Each worker is trimmed in its OWN short transaction. A single outer
    /// transaction across all workers (a prior optimization) held the WAL
    /// writer for the entire sweep, which could starve foreground
    /// `insert_event_raw` calls past `SQLITE_BUSY_TIMEOUT` on a busy system.
    /// Per-worker commits keep each lock hold short and let foreground
    /// writes interleave. The worker list is a point-in-time snapshot, but
    /// each trim re-reads the live max `seq` inside its own tx, so a worker
    /// that dropped below the cap between the scan and the trim simply
    /// archives nothing (F-10).
    pub async fn archive_excess_for_all(
        &self,
        keep_recent: u64,
    ) -> Result<std::collections::HashMap<String, u64>, RepositoryError> {
        // Single read pass: which workers exceed the cap?
        let rows = sqlx::query(
            "SELECT worker_id, COUNT(*) AS n FROM events \
             GROUP BY worker_id HAVING n > ?",
        )
        .bind(keep_recent as i64)
        .fetch_all(&self.pool)
        .await?;

        if rows.is_empty() {
            return Ok(std::collections::HashMap::new());
        }

        let mut out = std::collections::HashMap::new();
        for row in &rows {
            let worker_id: String = row.try_get("worker_id")?;
            // Per-worker BEGIN IMMEDIATE transaction: short write-lock hold
            // + a fresh in-tx count read (the outer `rows` list is a stale
            // snapshot). IMMEDIATE avoids the non-retryable read→write
            // SQLITE_BUSY_SNAPSHOT under concurrent foreground inserts.
            let moved = self.archive_excess_one(&worker_id, keep_recent).await?;
            if moved > 0 {
                out.insert(worker_id, moved);
            }
        }
        Ok(out)
    }

    /// Cursor-paged worker replay. `after_seq = None` starts from the
    /// smallest `seq`; subsequent calls pass `events.last().seq` to
    /// advance. An empty result means exhausted.
    ///
    /// `limit` is clamped to [`MAX_REPLAY_PAGE`] server-side.
    pub async fn replay_for_worker_page(
        &self,
        worker_id: &str,
        after_seq: Option<u64>,
        limit: u32,
    ) -> Result<Vec<Event>, RepositoryError> {
        let limit = limit.min(MAX_REPLAY_PAGE) as i64;
        // Bind after_seq twice so the SQL `(?2 IS NULL OR seq > ?2)`
        // guard works without Rust-side branching. NULL → all rows.
        let after_bind = after_seq.map(|v| v as i64);

        let rows = sqlx::query(
            "SELECT worker_id, task_id, seq, ts, type, payload_json, schema_version \
             FROM events \
             WHERE worker_id = ?1 \
               AND (?2 IS NULL OR seq > ?2) \
             ORDER BY seq ASC \
             LIMIT ?3",
        )
        .bind(worker_id)
        .bind(after_bind)
        .bind(limit)
        .fetch_all(&self.pool)
        .await?;

        rows.iter().map(row_to_event).collect()
    }

    /// Replay every event for a worker, in `seq` order. Thin wrapper
    /// over [`replay_for_worker_page`] that pages internally and
    /// concatenates. Memory footprint during the load is bounded to one
    /// page (`MAX_REPLAY_PAGE`) at a time — unlike the prior implementation
    /// which materialised the entire result set up front.
    ///
    /// New callers should prefer the page method directly so they can
    /// process events as they arrive. This wrapper exists for in-tree
    /// test callers that don't need streaming.
    pub async fn replay_for_worker(&self, worker_id: &str) -> Result<Vec<Event>, RepositoryError> {
        let mut all = Vec::new();
        let mut cursor: Option<u64> = None;
        loop {
            let page = self
                .replay_for_worker_page(worker_id, cursor, MAX_REPLAY_PAGE)
                .await?;
            if page.is_empty() {
                return Ok(all);
            }
            cursor = page.last().map(|e| e.seq);
            all.extend(page);
            if cursor.is_none() {
                // Defensive: page.last() returned None despite a non-empty
                // page — impossible, but bail rather than spin.
                return Ok(all);
            }
        }
    }

    /// Cursor-paged task replay. Ordering is `(ts, worker_id, seq)`. A task
    /// can be touched by more than one worker, and `seq` is per-worker (the
    /// events PK is `(worker_id, seq)`), so two workers can legitimately
    /// emit events sharing an identical `(ts, seq)`. `worker_id` is
    /// therefore part of both the ordering and the cursor — without it, one
    /// of a colliding pair is silently dropped at a page boundary (F-11).
    /// The cursor is the `(ts, worker_id, seq)` of the last returned row:
    /// all `None` on the first page, all `Some` on subsequent pages.
    ///
    /// `limit` is clamped to [`MAX_REPLAY_PAGE`] server-side.
    pub async fn replay_for_task_page(
        &self,
        task_id: &str,
        after_seq: Option<u64>,
        after_ts: Option<&str>,
        after_worker_id: Option<&str>,
        limit: u32,
    ) -> Result<Vec<Event>, RepositoryError> {
        let limit = limit.min(MAX_REPLAY_PAGE) as i64;
        let after_seq_bind = after_seq.map(|v| v as i64);

        let rows = sqlx::query(
            "SELECT worker_id, task_id, seq, ts, type, payload_json, schema_version \
             FROM events \
             WHERE task_id = ?1 \
               AND (?2 IS NULL \
                    OR ts > ?2 \
                    OR (ts = ?2 AND worker_id > ?4) \
                    OR (ts = ?2 AND worker_id = ?4 AND seq > ?3)) \
             ORDER BY ts ASC, worker_id ASC, seq ASC \
             LIMIT ?5",
        )
        .bind(task_id)
        .bind(after_ts)
        .bind(after_seq_bind)
        .bind(after_worker_id)
        .bind(limit)
        .fetch_all(&self.pool)
        .await?;

        rows.iter().map(row_to_event).collect()
    }

    /// Replay every event for a task, in `(ts, seq)` order. Thin
    /// wrapper over [`replay_for_task_page`]; see [`replay_for_worker`]
    /// for the rationale.
    pub async fn replay_for_task(&self, task_id: &str) -> Result<Vec<Event>, RepositoryError> {
        let mut all = Vec::new();
        let mut after_seq: Option<u64> = None;
        let mut after_ts: Option<String> = None;
        let mut after_worker_id: Option<String> = None;
        loop {
            let page = self
                .replay_for_task_page(
                    task_id,
                    after_seq,
                    after_ts.as_deref(),
                    after_worker_id.as_deref(),
                    MAX_REPLAY_PAGE,
                )
                .await?;
            if page.is_empty() {
                return Ok(all);
            }
            let last = page.last().unwrap();
            after_seq = Some(last.seq);
            after_ts = Some(last.ts.clone());
            after_worker_id = Some(last.worker_id.clone());
            all.extend(page);
        }
    }
}

impl Repository {
    /// Expose the connection pool for integration tests that need ad-hoc
    /// SQL assertions (e.g. raw column reads, PRAGMA queries).
    ///
    /// **Test-only.** Production code must use the typed methods on this
    /// struct — direct pool access bypasses error mapping and logging.
    ///
    /// Note for Task 5 implementers: this accessor was landed early
    /// (Task 2) to support the archive idempotence test. Do NOT
    /// re-add it — it already exists here.
    #[doc(hidden)]
    pub fn pool_for_test(&self) -> &SqlitePool {
        &self.pool
    }

    /// S4: typed wrapper around the free `mission_was_reverted` helper.
    /// The free function is reused so the SQL stays in one place; this
    /// accessor lets the host (which can't reach the private pool)
    /// drive idempotency for the `revert_mission` Tauri command.
    pub async fn mission_was_reverted(&self, mission_id: &str) -> Result<bool, RepositoryError> {
        revert_log::mission_was_reverted(&self.pool, mission_id)
            .await
            .map_err(RepositoryError::from)
    }

    /// S4: typed wrapper around `insert_mission_revert`. Same rationale
    /// as [`Self::mission_was_reverted`] — host calls flow through the
    /// typed surface so the database boundary stays small.
    pub async fn record_mission_revert(
        &self,
        mission_id: &str,
        reverted_at: &str,
        restored_sha: &str,
        pre_merge_tag: &str,
    ) -> Result<(), RepositoryError> {
        revert_log::insert_mission_revert(
            &self.pool,
            mission_id,
            reverted_at,
            restored_sha,
            pre_merge_tag,
        )
        .await
        .map_err(RepositoryError::from)
    }

    /// Read the compaction checkpoint for exactly one canonical repository.
    pub async fn last_compaction_run(
        &self,
        repo_root: &str,
    ) -> Result<Option<String>, RepositoryError> {
        compaction::get_last_compaction_run(&self.pool, repo_root)
            .await
            .map_err(RepositoryError::from)
    }

    /// Record a successful compaction pass for one canonical repository.
    pub async fn record_compaction_run(
        &self,
        repo_root: &str,
        last_run_at: &str,
        last_pruned_count: u32,
    ) -> Result<(), RepositoryError> {
        compaction::upsert_compaction_state(&self.pool, repo_root, last_run_at, last_pruned_count)
            .await
            .map_err(RepositoryError::from)
    }

    /// S10: cross-mission history view. Returns the last `limit` audited
    /// missions joined with their terminal disposition and revert status.
    pub async fn list_recent_missions(
        &self,
        limit: u32,
    ) -> Result<Vec<MissionHistoryDto>, RepositoryError> {
        history::list_recent_missions_impl(&self.pool, limit).await
    }
}

fn row_to_worker_info(row: &SqliteRow) -> Result<WorkerInfo, RepositoryError> {
    let id: String = row.try_get("id")?;
    let name: String = row.try_get("name")?;
    let vendor_str: String = row.try_get("vendor")?;
    let cli_binary: String = row.try_get("cli_binary")?;
    let cli_version: Option<String> = row.try_get("cli_version")?;
    let cwd: String = row.try_get("cwd")?;
    let model: Option<String> = row.try_get("model")?;
    let spawned_at: String = row.try_get("spawned_at")?;
    let ended_at: Option<String> = row.try_get("ended_at")?;

    let vendor: event_schema::Vendor =
        serde_json::from_value(serde_json::Value::String(vendor_str.clone()))
            .map_err(|_| RepositoryError::RowCorrupt(format!("unknown vendor {vendor_str:?}")))?;

    Ok(WorkerInfo {
        id,
        name,
        vendor,
        cli_binary,
        cli_version,
        cwd,
        model,
        spawned_at,
        ended_at,
    })
}

fn row_to_event(row: &SqliteRow) -> Result<Event, RepositoryError> {
    let worker_id: String = row.try_get("worker_id")?;
    let task_id: Option<String> = row.try_get("task_id")?;
    let seq: i64 = row.try_get("seq")?;
    let ts: String = row.try_get("ts")?;
    let type_str: String = row.try_get("type")?;
    let payload_json: String = row.try_get("payload_json")?;
    let schema_version: String = row.try_get("schema_version")?;

    let payload_value: serde_json::Value = serde_json::from_str(&payload_json)?;
    let envelope = serde_json::json!({
        "schema_version": schema_version,
        "worker_id": worker_id,
        "task_id": task_id,
        "seq": seq,
        "ts": ts,
        "type": type_str,
        "payload": payload_value,
    });

    // Schema §6: tolerate unknown event types — never crash on
    // replay. insert_event_raw accepts any type string, so a future
    // producer that emits a new event variant lands in the events
    // table; without this fallback, replay_for_worker errors out
    // entirely as soon as one such row exists.
    if let Ok(event) = serde_json::from_value::<Event>(envelope) {
        return Ok(event);
    }
    let raw_payload = serde_json::to_string(&payload_value)
        .unwrap_or_else(|_| String::from("<unparseable payload>"));
    Ok(Event {
        schema_version,
        worker_id,
        task_id,
        seq: u64::try_from(seq).unwrap_or(0),
        ts,
        kind: EventKind::Log(Log {
            level: LogLevel::Info,
            stream: LogStream::Stdout,
            line: raw_payload,
            tag: Some(format!("unknown:{type_str}")),
        }),
    })
}

#[cfg(test)]
pub(crate) async fn fresh_pool() -> SqlitePool {
    use tempfile::tempdir;
    let dir = tempdir().unwrap();
    let db_path = dir.path().join("repo.sqlite");
    let url = format!("sqlite:{}?mode=rwc", db_path.display());
    let pool = SqlitePool::connect(&url).await.unwrap();
    sqlx::migrate!("./migrations").run(&pool).await.unwrap();
    std::mem::forget(dir);
    pool
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn list_recent_workers_orders_ties_deterministically_by_id() {
        let repo = Repository::open_in_memory().await.unwrap();
        // Three workers sharing one spawn millisecond, inserted in an
        // order that deliberately differs from their id order. Without a
        // tiebreak, `ORDER BY spawned_at DESC` leaves the tie order
        // unspecified — the history list jumps between queries.
        for id in ["w-aaa", "w-ccc", "w-bbb"] {
            repo.insert_worker(&WorkerInfo {
                id: id.into(),
                name: id.into(),
                vendor: event_schema::Vendor::Mock,
                cli_binary: "mock".into(),
                cli_version: None,
                cwd: "/tmp".into(),
                model: None,
                spawned_at: "2026-05-29T12:00:00.000Z".into(),
                ended_at: None,
            })
            .await
            .unwrap();
        }
        let ids: Vec<String> = repo
            .list_recent_workers(10)
            .await
            .unwrap()
            .into_iter()
            .map(|w| w.id)
            .collect();
        // Ties broken by id DESC → a stable, deterministic ordering.
        assert_eq!(ids, ["w-ccc", "w-bbb", "w-aaa"]);
    }
}
