//! T3 long-term memory store (V3 §4.4).
//!
//! Owns the on-disk note files under `<memory_root>/notes/<id>.md` and
//! the queryable index in SQLite (`memory_notes`, `memory_links`,
//! `memory_provenance`). Workers never reach this surface directly;
//! `MemoryKernel` brokers all mutations.
//!
//! ## A1 rename (Tier-2F)
//!
//! Previously named `CodexStore` with files under
//! `<root>/codex/notes/<id>.md`. The "codex" word leaked into the
//! user-facing path layout; the rename flattens to
//! `<root>/notes/<id>.md` so the user-visible surface is just
//! `.vigla/memory/` (root) + `notes/` + `missions/`.
//! `MemoryKernel::open` migrates legacy layouts on first encounter.
//!
//! Atomic write protocol:
//!   1. Render the note (frontmatter + body) into a tmp file in the
//!      same directory as the target. Same-dir rename is atomic on
//!      POSIX/APFS.
//!   2. `sync_all` the file to flush data to disk.
//!   3. `rename` over the final path.
//!   4. Insert SQLite row + memory event in a single transaction.
//!
//! On any error after the tmp file is written, the tmp file is
//! removed. The SQL row only commits after the file is durable, so a
//! crash mid-write leaves no orphan index entries.
//!
//! ## Tier-1 refactor
//!
//! The previous P2 design exposed `note_add` (user path) and
//! `note_add_raw` (low-level, used by ratify) as independent methods,
//! each with their own tx. That broke atomicity for ratify: the note
//! row landed in one tx and the `MemoryRatified` event in another, so
//! a crash between them left the note row pointing at a non-existent
//! `created_event_id`. The refactor splits I/O from row insertion:
//!
//!   * [`MemoryStore::prepare_note`] — validate + atomically write the
//!     body file outside any tx. Returns a [`PreparedNote`].
//!   * [`MemoryStore::mint_note_in_tx`] — inside a caller-owned tx,
//!     insert the index row + provenance row pointing at a
//!     caller-supplied `created_event_id`.
//!
//! `note_add` chains the two with a `note_authored` event inside the
//! same tx. The ratification pipeline does the same with a
//! `MemoryRatified` event, achieving the atomicity the user called
//! out as missing.

use std::path::{Path, PathBuf};

use sqlx::{Sqlite, SqlitePool, Transaction};
use tokio::fs;
use tokio::io::AsyncWriteExt;

use event_schema::memory::WitnessKind;

use super::error::MemoryError;
use super::hierarchy::{
    ListFilter, MemoryNoteAuthored, Note, NoteAuthor, NoteKind, NoteState, NoteSummary, Scope,
    ScopeKind, MEMORY_SCHEMA_VERSION, NOTE_BODY_CAP_BYTES,
};
use super::ids;
use super::witnesses;

/// Subdirectory under the memory root that holds atomic note files.
const NOTES_SUBDIR: &str = "notes";

#[derive(Debug, Clone)]
pub struct MemoryStore {
    pool: SqlitePool,
    memory_root: PathBuf,
}

#[derive(Debug, Clone)]
pub struct NewNote {
    pub kind: NoteKind,
    pub scope: Scope,
    pub body: String,
}

/// Result of [`MemoryStore::prepare_note`]. The body file is durable
/// on disk by the time this returns; the caller chooses when to
/// commit the index row + provenance via [`MemoryStore::mint_note_in_tx`].
#[derive(Debug, Clone)]
pub struct PreparedNote {
    pub note_id: String,
    pub rel_body_path: String,
    pub body_hash: String,
    pub created_at: String,
    /// Title extracted from the body's first H1, captured here so
    /// `mint_note_in_tx` can persist it atomically with the row.
    /// `None` when the body has no H1.
    pub title: Option<String>,
}

/// Role recorded on `memory_provenance` for the genesis link.
#[derive(Debug, Clone, Copy)]
pub enum NoteOrigin {
    /// User authored — e.g. CLI pin or UI right-click. Genesis event
    /// is `note_authored`.
    Authored,
    /// Materialised from a ratified worker proposal. Genesis event is
    /// `ratified`.
    Ratified,
}

impl NoteOrigin {
    fn role(self) -> &'static str {
        match self {
            NoteOrigin::Authored => "authored",
            NoteOrigin::Ratified => "ratified",
        }
    }
}

impl MemoryStore {
    pub async fn open(pool: SqlitePool, memory_root: PathBuf) -> Result<Self, MemoryError> {
        fs::create_dir_all(memory_root.join(NOTES_SUBDIR)).await?;
        Ok(Self { pool, memory_root })
    }

    /// Validate inputs, generate a note id, and atomically write the
    /// body file. The returned [`PreparedNote`] is then passed to
    /// [`mint_note_in_tx`] inside the caller's transaction.
    ///
    /// On any error after the body file is written, the tmp file is
    /// cleaned up by `write_atomic`. On crash between this call and
    /// the tx commit, the file is orphaned (cheap to GC; no row
    /// references it).
    pub(crate) async fn prepare_note(&self, new: &NewNote) -> Result<PreparedNote, MemoryError> {
        validate_scope(&new.scope)?;
        validate_taxonomy(&self.pool, &new.kind, &new.scope.kind).await?;
        if new.body.len() > NOTE_BODY_CAP_BYTES {
            return Err(MemoryError::BodyTooLarge {
                actual: new.body.len(),
                cap: NOTE_BODY_CAP_BYTES,
            });
        }

        let note_id = ids::new_note_id();
        let created_at = crate::ids::rfc3339_now();
        let rel_body_path = format!("{NOTES_SUBDIR}/{note_id}.md");
        let abs_body_path = self.memory_root.join(&rel_body_path);
        let rendered = render_note_file(&note_id, new, &created_at);
        let body_hash = hash_hex(rendered.as_bytes());
        let title = extract_title(&new.body);
        write_atomic(&abs_body_path, rendered.as_bytes()).await?;

        Ok(PreparedNote {
            note_id,
            rel_body_path,
            body_hash,
            created_at,
            title,
        })
    }

    /// Inside a caller-owned tx, insert the index row + provenance row
    /// pointing at `created_event_id`. The caller is responsible for
    /// having already inserted the corresponding event row in the same
    /// tx — that's what makes ratification atomic.
    pub(crate) async fn mint_note_in_tx(
        &self,
        tx: &mut Transaction<'_, Sqlite>,
        new: &NewNote,
        prepared: &PreparedNote,
        created_event_id: &str,
        origin: NoteOrigin,
    ) -> Result<String, MemoryError> {
        sqlx::query(
            "INSERT INTO memory_notes \
             (id, kind, scope_kind, scope_value, body_path, body_hash, state, \
              created_event_id, created_at, title) \
             VALUES (?, ?, ?, ?, ?, ?, 'owned', ?, ?, ?)",
        )
        .bind(&prepared.note_id)
        .bind(new.kind.as_str())
        .bind(new.scope.kind.as_str())
        .bind(new.scope.value.as_deref())
        .bind(&prepared.rel_body_path)
        .bind(&prepared.body_hash)
        .bind(created_event_id)
        .bind(&prepared.created_at)
        .bind(prepared.title.as_deref())
        .execute(&mut **tx)
        .await?;
        sqlx::query("INSERT INTO memory_provenance (note_id, event_id, role) VALUES (?, ?, ?)")
            .bind(&prepared.note_id)
            .bind(created_event_id)
            .bind(origin.role())
            .execute(&mut **tx)
            .await?;
        Ok(prepared.note_id.clone())
    }

    /// User-authored note. Atomic across:
    ///   1. body file write (durable before tx)
    ///   2. note_authored event row
    ///   3. memory_notes index row
    ///   4. memory_provenance link
    ///
    ///   5. UserAuthored witness (source_event_id = note_authored event_id)
    ///
    /// All five land in ONE transaction, so a crash can't leave a
    /// committed note un-witnessed. The witness write is idempotent on the
    /// (note_id, source_event_id) UNIQUE constraint.
    pub async fn note_add(&self, new: NewNote, author: NoteAuthor) -> Result<String, MemoryError> {
        let prepared = self.prepare_note(&new).await?;
        let event_id = ids::new_memory_event_id();

        let NoteAuthor::User { source } = author;
        let payload = MemoryNoteAuthored {
            note_id: prepared.note_id.clone(),
            source,
            body: new.body.clone(),
        };
        let payload_json = serde_json::to_string(&payload)?;

        let mut tx = self.pool.begin().await?;
        sqlx::query(
            "INSERT INTO memory_events \
             (event_id, mission_id, worker_id, ts, type, payload_json, schema_version) \
             VALUES (?, NULL, NULL, ?, 'note_authored', ?, ?)",
        )
        .bind(&event_id)
        .bind(&prepared.created_at)
        .bind(&payload_json)
        .bind(MEMORY_SCHEMA_VERSION)
        .execute(&mut *tx)
        .await?;
        let note_id = self
            .mint_note_in_tx(&mut tx, &new, &prepared, &event_id, NoteOrigin::Authored)
            .await?;
        // Record the UserAuthored witness INSIDE the same transaction, so a
        // crash between the note commit and the witness write can't leave a
        // committed note with zero witnesses (which would never promote).
        // Mirrors the ratify path; idempotent on the
        // (note_id, source_event_id) UNIQUE constraint (F-2).
        witnesses::record_in_tx(&mut tx, &note_id, WitnessKind::UserAuthored, &event_id).await?;
        tx.commit().await?;

        Ok(note_id)
    }

    /// Fetch a single note. Reads the body file off disk; if the file
    /// is missing returns [`MemoryError::NoteNotFound`] (with a
    /// distinct message from "row missing").
    pub async fn note_show(&self, id: &str) -> Result<Note, MemoryError> {
        type NoteRow = (
            String,
            String,
            Option<String>,
            String,
            String,
            String,
            String,
            Option<String>,
            String,
            Option<String>,
        );
        let row: Option<NoteRow> = sqlx::query_as(
            "SELECT kind, scope_kind, scope_value, body_path, body_hash, state, \
                    created_event_id, last_verified_at, created_at, title \
             FROM memory_notes WHERE id = ?",
        )
        .bind(id)
        .fetch_optional(&self.pool)
        .await?;
        let (
            kind,
            scope_kind,
            scope_value,
            body_path,
            body_hash,
            state,
            created_event_id,
            last_verified_at,
            created_at,
            title,
        ) = row.ok_or_else(|| MemoryError::NoteNotFound(id.to_owned()))?;

        let abs_body = self.memory_root.join(&body_path);
        let body_bytes = fs::read(&abs_body).await.map_err(|e| match e.kind() {
            std::io::ErrorKind::NotFound => MemoryError::NoteNotFound(format!(
                "{id} (body file {} missing)",
                abs_body.display()
            )),
            _ => MemoryError::Io(e),
        })?;
        let body = String::from_utf8(body_bytes)
            .map_err(|e| MemoryError::RowCorrupt(format!("body utf8: {e}")))?;
        let body = strip_frontmatter(&body).to_owned();

        Ok(Note {
            id: id.to_owned(),
            kind: NoteKind::from_str(&kind),
            scope: Scope {
                kind: ScopeKind::from_str(&scope_kind)
                    .ok_or_else(|| MemoryError::RowCorrupt(format!("scope_kind {scope_kind}")))?,
                value: scope_value,
            },
            body,
            body_hash,
            state: NoteState::from_str(&state)
                .ok_or_else(|| MemoryError::RowCorrupt(format!("state {state}")))?,
            created_event_id,
            created_at,
            last_verified_at,
            title,
        })
    }

    /// A3 (Tier-2G): batch fetch. Resolve `ids` to their full
    /// [`Note`]s in one SQL query plus a concurrent fan-out for the
    /// body-file reads.
    ///
    /// **Contract.** Returns a `HashMap<id, Note>` containing one
    /// entry per id that exists in the index AND whose body file is
    /// readable. Missing ids — whether absent from the index or
    /// orphaned on disk — are silently omitted; the caller decides
    /// how to react (the composer emits a `dropped("not_found")`
    /// trace decision for each absent id).
    ///
    /// Chunks input lists longer than [`Self::MAX_BATCH`] so we
    /// never hit SQLite's parameter cap (`SQLITE_LIMIT_VARIABLE_NUMBER`,
    /// default 999). Per-chunk failures propagate; per-body-file
    /// failures log to stderr and are skipped.
    pub async fn notes_by_ids(
        &self,
        ids: &[String],
    ) -> Result<std::collections::HashMap<String, Note>, MemoryError> {
        if ids.is_empty() {
            return Ok(std::collections::HashMap::new());
        }
        let mut out = std::collections::HashMap::<String, Note>::with_capacity(ids.len());
        for chunk in ids.chunks(Self::MAX_BATCH) {
            self.notes_by_ids_chunk(chunk, &mut out).await?;
        }
        Ok(out)
    }

    /// Caller-friendly cap on a single `notes_by_ids` round-trip. The
    /// SQLite default for `SQLITE_LIMIT_VARIABLE_NUMBER` is 999, so
    /// 500 leaves significant headroom and is well above any realistic
    /// composer candidate set.
    pub const MAX_BATCH: usize = 500;

    async fn notes_by_ids_chunk(
        &self,
        ids: &[String],
        out: &mut std::collections::HashMap<String, Note>,
    ) -> Result<(), MemoryError> {
        debug_assert!(!ids.is_empty());
        debug_assert!(ids.len() <= Self::MAX_BATCH);

        // Build "?,?,?,..." placeholders. ids come from our own DB
        // so they're safe to bind individually, but we never
        // interpolate user input into the SQL string itself.
        let placeholders = std::iter::repeat_n("?", ids.len())
            .collect::<Vec<_>>()
            .join(",");
        let sql = format!(
            "SELECT id, kind, scope_kind, scope_value, body_path, body_hash, state, \
                    created_event_id, created_at, last_verified_at, title \
             FROM memory_notes WHERE id IN ({placeholders})"
        );
        type BatchRow = (
            String,         // id
            String,         // kind
            String,         // scope_kind
            Option<String>, // scope_value
            String,         // body_path
            String,         // body_hash
            String,         // state
            String,         // created_event_id
            String,         // created_at
            Option<String>, // last_verified_at
            Option<String>, // title
        );
        let mut q = sqlx::query_as::<_, BatchRow>(&sql);
        for id in ids {
            q = q.bind(id);
        }
        let rows: Vec<BatchRow> = q.fetch_all(&self.pool).await?;

        // Concurrent body-file reads. For N=30 small files this saves
        // tens of milliseconds on cold disks; even on warm disks the
        // I/O cost dominates the parallel-await overhead.
        // `r.4` is `body_path` (see the tuple alias above; positional
        // because sqlx-derived structs would need the `derive`
        // feature we don't enable elsewhere).
        let read_futures = rows.iter().map(|r| {
            let abs = self.memory_root.join(&r.4);
            async move { fs::read(&abs).await }
        });
        let body_results: Vec<std::io::Result<Vec<u8>>> =
            futures::future::join_all(read_futures).await;

        for (row, body_res) in rows.into_iter().zip(body_results) {
            let (
                id,
                kind,
                scope_kind,
                scope_value,
                body_path,
                body_hash,
                state,
                created_event_id,
                created_at,
                last_verified_at,
                title,
            ) = row;
            let body_bytes = match body_res {
                Ok(b) => b,
                Err(e) => {
                    // Orphaned index row — log and skip. Composer
                    // surfaces this as a `dropped("not_found")` trace
                    // decision via the missing-from-map check.
                    tracing::error!(
                        "vigla: memory body file missing for note {id} at {}: {e}",
                        self.memory_root.join(&body_path).display()
                    );
                    continue;
                }
            };
            let body = match String::from_utf8(body_bytes) {
                Ok(s) => strip_frontmatter(&s).to_owned(),
                Err(e) => {
                    tracing::warn!("vigla: memory body file for note {id} is not utf8: {e}");
                    continue;
                }
            };
            let scope_kind_typed = match ScopeKind::from_str(&scope_kind) {
                Some(s) => s,
                None => {
                    tracing::warn!("vigla: corrupt scope_kind {scope_kind} on note {id}; skipping");
                    continue;
                }
            };
            let state_typed = match NoteState::from_str(&state) {
                Some(s) => s,
                None => {
                    tracing::warn!("vigla: corrupt state {state} on note {id}; skipping");
                    continue;
                }
            };
            out.insert(
                id.clone(),
                Note {
                    id,
                    kind: NoteKind::from_str(&kind),
                    scope: Scope {
                        kind: scope_kind_typed,
                        value: scope_value,
                    },
                    body,
                    body_hash,
                    state: state_typed,
                    created_event_id,
                    created_at,
                    last_verified_at,
                    title,
                },
            );
        }
        Ok(())
    }

    /// List notes matching `filter`. Always sorted newest-first by
    /// `created_at` so the UI shows recent activity at the top.
    pub async fn note_list(&self, filter: ListFilter) -> Result<Vec<NoteSummary>, MemoryError> {
        let mut sql = String::from(
            "SELECT id, kind, scope_kind, scope_value, state, created_at \
             FROM memory_notes WHERE 1=1",
        );
        let mut binds: Vec<String> = Vec::new();
        if let Some(k) = &filter.kind {
            sql.push_str(" AND kind = ?");
            binds.push(k.as_str().to_owned());
        }
        if let Some(s) = filter.state {
            sql.push_str(" AND state = ?");
            binds.push(s.as_str().to_owned());
        }
        if let Some(sk) = filter.scope_kind {
            sql.push_str(" AND scope_kind = ?");
            binds.push(sk.as_str().to_owned());
        }
        if let Some(sv) = &filter.scope_value {
            sql.push_str(" AND scope_value = ?");
            binds.push(sv.clone());
        }
        sql.push_str(" ORDER BY created_at DESC");

        let mut q =
            sqlx::query_as::<_, (String, String, String, Option<String>, String, String)>(&sql);
        for b in &binds {
            q = q.bind(b);
        }
        let rows = q.fetch_all(&self.pool).await?;

        let mut out = Vec::with_capacity(rows.len());
        for (id, kind, scope_kind, scope_value, state, created_at) in rows {
            out.push(NoteSummary {
                id,
                kind: NoteKind::from_str(&kind),
                scope: Scope {
                    kind: ScopeKind::from_str(&scope_kind).ok_or_else(|| {
                        MemoryError::RowCorrupt(format!("scope_kind {scope_kind}"))
                    })?,
                    value: scope_value,
                },
                state: NoteState::from_str(&state)
                    .ok_or_else(|| MemoryError::RowCorrupt(format!("state {state}")))?,
                created_at,
            });
        }
        Ok(out)
    }

    /// Pool accessor used by tests to inspect raw event rows. Not
    /// part of the public surface — production callers go through
    /// `MemoryKernel`.
    #[cfg(test)]
    pub(crate) fn pool(&self) -> &SqlitePool {
        &self.pool
    }

    /// Memory root path. Tests use this to verify on-disk layout.
    pub fn root(&self) -> &Path {
        &self.memory_root
    }
}

// ---------------------------------------------------------------------
// Test/debug helper: synthesise an owned note without going through
// the user or ratification path. Marked `pub(crate)` and documented
// loudly because it skips event emission — production code must NOT
// use it.
// ---------------------------------------------------------------------

#[cfg(test)]
impl MemoryStore {
    /// Test-only seed helper. Mints a note in `state=owned` with a
    /// synthetic `created_event_id` that does NOT correspond to a
    /// `memory_events` row. Use only for seeding test fixtures that
    /// don't exercise the full ratify / pin paths.
    pub(crate) async fn _test_seed_owned_note(&self, new: NewNote) -> Result<String, MemoryError> {
        let prepared = self.prepare_note(&new).await?;
        let synthetic_event_id = ids::new_memory_event_id();
        let mut tx = self.pool.begin().await?;
        let id = self
            .mint_note_in_tx(
                &mut tx,
                &new,
                &prepared,
                &synthetic_event_id,
                NoteOrigin::Ratified,
            )
            .await?;
        tx.commit().await?;
        Ok(id)
    }
}

// ---------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------

fn validate_scope(scope: &Scope) -> Result<(), MemoryError> {
    if scope.kind == ScopeKind::Repo {
        return Ok(());
    }
    match scope.value.as_deref() {
        Some(v) if !v.is_empty() => Ok(()),
        _ => Err(MemoryError::MissingScopeValue(
            scope.kind.as_str().to_owned(),
        )),
    }
}

async fn validate_taxonomy(
    pool: &SqlitePool,
    kind: &NoteKind,
    scope_kind: &ScopeKind,
) -> Result<(), MemoryError> {
    let kind_exists: Option<(String,)> = sqlx::query_as(
        "SELECT name FROM memory_taxonomy WHERE category = 'kind' AND name = ? AND deprecated_at IS NULL",
    )
    .bind(kind.as_str())
    .fetch_optional(pool)
    .await?;
    if kind_exists.is_none() {
        return Err(MemoryError::UnknownTaxonomy {
            category: "kind".into(),
            name: kind.as_str().to_owned(),
        });
    }
    let scope_exists: Option<(String,)> = sqlx::query_as(
        "SELECT name FROM memory_taxonomy WHERE category = 'scope_kind' AND name = ? AND deprecated_at IS NULL",
    )
    .bind(scope_kind.as_str())
    .fetch_optional(pool)
    .await?;
    if scope_exists.is_none() {
        return Err(MemoryError::UnknownTaxonomy {
            category: "scope_kind".into(),
            name: scope_kind.as_str().to_owned(),
        });
    }
    Ok(())
}

fn render_note_file(note_id: &str, new: &NewNote, created_at: &str) -> String {
    let mut s = String::new();
    s.push_str("---\n");
    s.push_str(&format!("id: {note_id}\n"));
    s.push_str(&format!("kind: {}\n", new.kind.as_str()));
    s.push_str("scope:\n");
    s.push_str(&format!("  kind: {}\n", new.scope.kind.as_str()));
    if let Some(v) = &new.scope.value {
        s.push_str(&format!("  value: {v}\n"));
    }
    s.push_str(&format!("created_at: {created_at}\n"));
    s.push_str("schema_version: 1\n");
    s.push_str("---\n");
    s.push_str(new.body.trim_end());
    s.push('\n');
    s
}

fn strip_frontmatter(rendered: &str) -> &str {
    if !rendered.starts_with("---\n") {
        return rendered;
    }
    if let Some(rest) = rendered.strip_prefix("---\n") {
        if let Some(end) = rest.find("\n---\n") {
            return &rest[end + "\n---\n".len()..];
        }
    }
    rendered
}

/// Extract a curator title from the first Markdown H1 in `body`.
///
/// Conventions:
///
/// - Leading blank lines are skipped.
/// - Only `"# "` (single hash + space) counts; `"## "` and deeper are
///   subsections, not titles.
/// - Title text is trimmed and capped at [`TITLE_MAX_LEN`] characters
///   so a runaway one-line note can't blow the row up.
/// - If the first non-blank line isn't an H1, the note has no
///   surfaceable title and this returns `None` — V1.1 BM25 treats
///   `None` titles as "body-only signal" rather than as a parse error.
pub(crate) fn extract_title(body: &str) -> Option<String> {
    for line in body.lines() {
        let trimmed = line.trim_start();
        if trimmed.is_empty() {
            continue;
        }
        let rest = trimmed.strip_prefix("# ")?;
        let title = rest.trim();
        if title.is_empty() {
            return None;
        }
        let capped: String = title.chars().take(TITLE_MAX_LEN).collect();
        return Some(capped);
    }
    None
}

/// Cap on extracted-title length, in chars. Mirrors the design doc's
/// "title is the curator's distilled summary" framing; a 200-char cap
/// keeps the SQLite row small while comfortably fitting any real
/// human-authored H1.
pub(crate) const TITLE_MAX_LEN: usize = 200;

/// One-shot backfill: populate `memory_notes.title` for rows where it
/// is currently NULL by re-reading the body file and re-running
/// [`extract_title`]. Idempotent; safe to call on every kernel open.
///
/// Errors reading individual body files are logged at `warn` and the
/// row is left with `title = NULL` — backfill is best-effort, never
/// blocks kernel open.
pub(crate) async fn backfill_titles(
    pool: &SqlitePool,
    memory_root: &Path,
) -> Result<(), MemoryError> {
    let rows: Vec<(String, String)> =
        sqlx::query_as("SELECT id, body_path FROM memory_notes WHERE title IS NULL")
            .fetch_all(pool)
            .await?;
    if rows.is_empty() {
        return Ok(());
    }

    for (id, rel_path) in rows {
        let abs = memory_root.join(&rel_path);
        let bytes = match fs::read(&abs).await {
            Ok(b) => b,
            Err(e) => {
                tracing::warn!(
                    "vigla: backfill: body file missing for note {id} at {}: {e}",
                    abs.display()
                );
                continue;
            }
        };
        let rendered = match String::from_utf8(bytes) {
            Ok(s) => s,
            Err(e) => {
                tracing::warn!("vigla: backfill: body for note {id} not utf8: {e}");
                continue;
            }
        };
        let body = strip_frontmatter(&rendered);
        let Some(title) = extract_title(body) else {
            continue;
        };
        if let Err(e) =
            sqlx::query("UPDATE memory_notes SET title = ? WHERE id = ? AND title IS NULL")
                .bind(&title)
                .bind(&id)
                .execute(pool)
                .await
        {
            tracing::warn!("vigla: backfill: failed to update title for note {id}: {e}");
        }
    }
    Ok(())
}

pub(crate) fn hash_hex(bytes: &[u8]) -> String {
    let h = blake3::hash(bytes);
    let bs = h.as_bytes();
    let mut s = String::with_capacity(32);
    for b in &bs[..16] {
        s.push_str(&format!("{b:02x}"));
    }
    s
}

async fn write_atomic(dest: &Path, bytes: &[u8]) -> Result<(), MemoryError> {
    let parent = dest.parent().ok_or_else(|| {
        std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            "no parent dir for atomic write",
        )
    })?;
    fs::create_dir_all(parent).await?;

    let tmp_name = format!(
        ".{}.tmp.{}",
        dest.file_name().and_then(|s| s.to_str()).unwrap_or("note"),
        uuid::Uuid::now_v7().simple()
    );
    let tmp_path = parent.join(tmp_name);

    let mut f = fs::File::create(&tmp_path).await?;
    f.write_all(bytes).await?;
    f.flush().await?;
    f.sync_all().await?;
    drop(f);

    match fs::rename(&tmp_path, dest).await {
        Ok(()) => Ok(()),
        Err(e) => {
            let _ = fs::remove_file(&tmp_path).await;
            Err(MemoryError::Io(e))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::memory::hierarchy::{NoteKind, NoteState, Scope, ScopeKind, StandardNoteKind};
    use event_schema::memory::{AuthorSource, WitnessKind};
    use sqlx::sqlite::{SqliteConnectOptions, SqlitePoolOptions};
    use std::str::FromStr;
    use tempfile::TempDir;

    async fn fresh_store() -> (MemoryStore, TempDir) {
        let opts = SqliteConnectOptions::from_str("sqlite::memory:").unwrap();
        let pool = SqlitePoolOptions::new()
            .min_connections(1)
            .max_connections(1)
            .idle_timeout(None)
            .max_lifetime(None)
            .connect_with(opts)
            .await
            .unwrap();
        sqlx::migrate!("./migrations").run(&pool).await.unwrap();
        let dir = TempDir::new().unwrap();
        let store = MemoryStore::open(pool, dir.path().to_path_buf())
            .await
            .unwrap();
        (store, dir)
    }

    fn user_author() -> NoteAuthor {
        NoteAuthor::User {
            source: AuthorSource::Cli,
        }
    }

    #[tokio::test]
    async fn note_add_then_show_roundtrip() {
        let (store, _dir) = fresh_store().await;
        let id = store
            .note_add(
                NewNote {
                    kind: NoteKind::Standard(StandardNoteKind::Hazard),
                    scope: Scope {
                        kind: ScopeKind::Path,
                        value: Some("adapters/claude".into()),
                    },
                    body: "Resume tokens are host-bound; recapture per host.".into(),
                },
                user_author(),
            )
            .await
            .unwrap();
        let note = store.note_show(&id).await.unwrap();
        assert_eq!(note.kind.as_str(), "hazard");
        assert!(note.body.contains("Resume tokens"));
        assert_eq!(note.state, NoteState::Owned);
    }

    #[tokio::test]
    async fn note_add_records_user_authored_witness() {
        let (store, _dir) = fresh_store().await;
        let id = store
            .note_add(
                NewNote {
                    kind: NoteKind::Standard(StandardNoteKind::Fact),
                    scope: Scope {
                        kind: ScopeKind::Repo,
                        value: None,
                    },
                    body: "y".into(),
                },
                user_author(),
            )
            .await
            .unwrap();

        let ws = witnesses::for_note(store.pool(), &id).await.unwrap();
        assert_eq!(ws.len(), 1);
        assert_eq!(ws[0].kind, WitnessKind::UserAuthored);

        // Source event id is the note_authored event id — verify by
        // joining against memory_events.
        let (event_type,): (String,) =
            sqlx::query_as("SELECT type FROM memory_events WHERE event_id = ?")
                .bind(&ws[0].source_event_id)
                .fetch_one(store.pool())
                .await
                .unwrap();
        assert_eq!(event_type, "note_authored");
    }

    #[tokio::test]
    async fn note_add_created_event_id_resolves_to_real_event() {
        let (store, _dir) = fresh_store().await;
        let id = store
            .note_add(
                NewNote {
                    kind: NoteKind::Standard(StandardNoteKind::Fact),
                    scope: Scope {
                        kind: ScopeKind::Repo,
                        value: None,
                    },
                    body: "z".into(),
                },
                user_author(),
            )
            .await
            .unwrap();
        let n = store.note_show(&id).await.unwrap();
        let (count,): (i64,) =
            sqlx::query_as("SELECT COUNT(*) FROM memory_events WHERE event_id = ?")
                .bind(&n.created_event_id)
                .fetch_one(store.pool())
                .await
                .unwrap();
        assert_eq!(count, 1, "created_event_id must resolve to an event row");
    }

    #[tokio::test]
    async fn body_file_lives_on_disk_with_frontmatter() {
        let (store, _dir) = fresh_store().await;
        let id = store
            .note_add(
                NewNote {
                    kind: NoteKind::Standard(StandardNoteKind::Fact),
                    scope: Scope {
                        kind: ScopeKind::Repo,
                        value: None,
                    },
                    body: "Build with `cargo build --workspace`.".into(),
                },
                user_author(),
            )
            .await
            .unwrap();
        let path = store.root().join(format!("notes/{id}.md"));
        let contents = std::fs::read_to_string(&path).unwrap();
        assert!(contents.starts_with("---\n"));
        assert!(contents.contains("kind: fact\n"));
    }

    #[tokio::test]
    async fn list_filters_by_kind_and_state() {
        let (store, _dir) = fresh_store().await;
        store
            .note_add(
                NewNote {
                    kind: NoteKind::Standard(StandardNoteKind::Hazard),
                    scope: Scope {
                        kind: ScopeKind::Path,
                        value: Some("a".into()),
                    },
                    body: "h".into(),
                },
                user_author(),
            )
            .await
            .unwrap();
        store
            .note_add(
                NewNote {
                    kind: NoteKind::Standard(StandardNoteKind::Fact),
                    scope: Scope {
                        kind: ScopeKind::Repo,
                        value: None,
                    },
                    body: "f".into(),
                },
                user_author(),
            )
            .await
            .unwrap();

        let all = store.note_list(ListFilter::default()).await.unwrap();
        assert_eq!(all.len(), 2);
        let hazards = store
            .note_list(ListFilter {
                kind: Some(NoteKind::Standard(StandardNoteKind::Hazard)),
                ..Default::default()
            })
            .await
            .unwrap();
        assert_eq!(hazards.len(), 1);
    }

    #[tokio::test]
    async fn rejects_missing_scope_value() {
        let (store, _dir) = fresh_store().await;
        let err = store
            .note_add(
                NewNote {
                    kind: NoteKind::Standard(StandardNoteKind::Hazard),
                    scope: Scope {
                        kind: ScopeKind::Path,
                        value: None,
                    },
                    body: "x".into(),
                },
                user_author(),
            )
            .await
            .unwrap_err();
        assert!(matches!(err, MemoryError::MissingScopeValue(s) if s == "path"));
    }

    #[tokio::test]
    async fn rejects_oversized_body() {
        let (store, _dir) = fresh_store().await;
        let huge = "x".repeat(NOTE_BODY_CAP_BYTES + 1);
        let err = store
            .note_add(
                NewNote {
                    kind: NoteKind::Standard(StandardNoteKind::Fact),
                    scope: Scope {
                        kind: ScopeKind::Repo,
                        value: None,
                    },
                    body: huge,
                },
                user_author(),
            )
            .await
            .unwrap_err();
        assert!(matches!(err, MemoryError::BodyTooLarge { .. }));
    }

    #[tokio::test]
    async fn rejects_unknown_kind() {
        let (store, _dir) = fresh_store().await;
        let err = store
            .note_add(
                NewNote {
                    kind: NoteKind::Other("lesson".into()),
                    scope: Scope {
                        kind: ScopeKind::Repo,
                        value: None,
                    },
                    body: "x".into(),
                },
                user_author(),
            )
            .await
            .unwrap_err();
        assert!(
            matches!(err, MemoryError::UnknownTaxonomy { ref category, .. } if category == "kind")
        );
    }

    #[tokio::test]
    async fn note_authored_event_is_recorded() {
        let (store, _dir) = fresh_store().await;
        store
            .note_add(
                NewNote {
                    kind: NoteKind::Standard(StandardNoteKind::Fact),
                    scope: Scope {
                        kind: ScopeKind::Repo,
                        value: None,
                    },
                    body: "y".into(),
                },
                user_author(),
            )
            .await
            .unwrap();
        let (count,): (i64,) =
            sqlx::query_as("SELECT COUNT(*) FROM memory_events WHERE type = 'note_authored'")
                .fetch_one(store.pool())
                .await
                .unwrap();
        assert_eq!(count, 1);
    }

    #[tokio::test]
    async fn note_show_returns_not_found_on_missing_id() {
        let (store, _dir) = fresh_store().await;
        let err = store.note_show("does-not-exist").await.unwrap_err();
        assert!(matches!(err, MemoryError::NoteNotFound(_)));
    }

    #[tokio::test]
    async fn list_ordering_is_newest_first() {
        let (store, _dir) = fresh_store().await;
        let a = store
            .note_add(
                NewNote {
                    kind: NoteKind::Standard(StandardNoteKind::Fact),
                    scope: Scope {
                        kind: ScopeKind::Repo,
                        value: None,
                    },
                    body: "a".into(),
                },
                user_author(),
            )
            .await
            .unwrap();
        tokio::time::sleep(std::time::Duration::from_millis(2)).await;
        let b = store
            .note_add(
                NewNote {
                    kind: NoteKind::Standard(StandardNoteKind::Fact),
                    scope: Scope {
                        kind: ScopeKind::Repo,
                        value: None,
                    },
                    body: "b".into(),
                },
                user_author(),
            )
            .await
            .unwrap();
        let list = store.note_list(ListFilter::default()).await.unwrap();
        assert_eq!(list[0].id, b);
        assert_eq!(list[1].id, a);
    }

    // -----------------------------------------------------------------
    // A3 (Tier-2G) — notes_by_ids batch fetch
    // -----------------------------------------------------------------

    async fn seed_note(store: &MemoryStore, body: &str) -> String {
        store
            .note_add(
                NewNote {
                    kind: NoteKind::Standard(StandardNoteKind::Fact),
                    scope: Scope {
                        kind: ScopeKind::Repo,
                        value: None,
                    },
                    body: body.into(),
                },
                user_author(),
            )
            .await
            .unwrap()
    }

    #[tokio::test]
    async fn notes_by_ids_empty_input_returns_empty() {
        let (store, _dir) = fresh_store().await;
        let out = store.notes_by_ids(&[]).await.unwrap();
        assert!(out.is_empty());
    }

    #[tokio::test]
    async fn notes_by_ids_resolves_present_ids_with_full_body() {
        let (store, _dir) = fresh_store().await;
        let a = seed_note(&store, "alpha-body").await;
        let b = seed_note(&store, "beta-body").await;
        let out = store.notes_by_ids(&[a.clone(), b.clone()]).await.unwrap();
        assert_eq!(out.len(), 2);
        // Body files carry a trailing newline by render convention;
        // `note_show` and the batch path both surface that. Compare
        // the trimmed form to keep the test's intent clear.
        assert_eq!(out.get(&a).unwrap().body.trim(), "alpha-body");
        assert_eq!(out.get(&b).unwrap().body.trim(), "beta-body");
    }

    #[tokio::test]
    async fn notes_by_ids_silently_omits_unknown_ids() {
        let (store, _dir) = fresh_store().await;
        let a = seed_note(&store, "exists").await;
        let out = store
            .notes_by_ids(&[a.clone(), "no-such-id".into()])
            .await
            .unwrap();
        assert_eq!(out.len(), 1);
        assert!(out.contains_key(&a));
        assert!(!out.contains_key("no-such-id"));
    }

    #[tokio::test]
    async fn notes_by_ids_skips_orphan_index_rows() {
        let (store, _dir) = fresh_store().await;
        let a = seed_note(&store, "x").await;
        // Simulate disk corruption: delete the body file but leave
        // the index row. Batch fetch should log + skip, not error.
        let path = store.root().join(format!("notes/{a}.md"));
        std::fs::remove_file(&path).unwrap();
        let out = store.notes_by_ids(std::slice::from_ref(&a)).await.unwrap();
        assert!(out.is_empty());
    }

    #[tokio::test]
    async fn notes_by_ids_handles_chunks_when_input_exceeds_max_batch() {
        // We don't seed 500+ notes; instead we synthesise an id list
        // longer than MAX_BATCH containing one real id and the rest
        // unknown. The chunking path is exercised; unknown ids
        // silently omit.
        let (store, _dir) = fresh_store().await;
        let real = seed_note(&store, "y").await;
        let mut ids = vec![real.clone()];
        for i in 0..(MemoryStore::MAX_BATCH + 10) {
            ids.push(format!("ghost-{i}"));
        }
        let out = store.notes_by_ids(&ids).await.unwrap();
        // Only the real one resolves.
        assert_eq!(out.len(), 1);
        assert!(out.contains_key(&real));
    }

    #[tokio::test]
    async fn notes_by_ids_returns_consistent_data_for_repeated_ids() {
        // SQL `IN (?, ?)` with the same id twice returns one row.
        // `notes_by_ids` is HashMap-shaped, so dupes collapse to a
        // single entry. Confirm.
        let (store, _dir) = fresh_store().await;
        let a = seed_note(&store, "z").await;
        let out = store.notes_by_ids(&[a.clone(), a.clone()]).await.unwrap();
        assert_eq!(out.len(), 1);
        assert_eq!(out.get(&a).unwrap().body.trim(), "z");
    }

    // ---- Phase 0 (hybrid retrieval) — title extraction --------------

    #[test]
    fn extract_title_takes_first_h1_after_blank_lines() {
        let body = "\n\n# Auth tokens are host-bound\n\nSome body.";
        assert_eq!(
            super::extract_title(body),
            Some("Auth tokens are host-bound".to_string())
        );
    }

    #[test]
    fn extract_title_returns_none_when_first_nonblank_line_is_not_h1() {
        assert_eq!(super::extract_title("Just a paragraph\n# Later H1"), None);
        assert_eq!(super::extract_title("## H2 only"), None);
        assert_eq!(super::extract_title(""), None);
        assert_eq!(super::extract_title("# "), None);
    }

    #[test]
    fn extract_title_caps_at_title_max_len_chars() {
        let body = format!("# {}", "x".repeat(super::TITLE_MAX_LEN + 50));
        let t = super::extract_title(&body).unwrap();
        assert_eq!(t.chars().count(), super::TITLE_MAX_LEN);
    }

    #[tokio::test]
    async fn note_add_persists_title_extracted_from_body_h1() {
        let (store, _dir) = fresh_store().await;
        let id = store
            .note_add(
                NewNote {
                    kind: NoteKind::Standard(StandardNoteKind::Fact),
                    scope: Scope {
                        kind: ScopeKind::Repo,
                        value: None,
                    },
                    body: "# Use std::pin::Pin not core::pin::Pin\n\nReason: …".into(),
                },
                user_author(),
            )
            .await
            .unwrap();

        let note = store.note_show(&id).await.unwrap();
        assert_eq!(
            note.title.as_deref(),
            Some("Use std::pin::Pin not core::pin::Pin")
        );
    }

    #[tokio::test]
    async fn note_add_leaves_title_null_when_body_has_no_h1() {
        let (store, _dir) = fresh_store().await;
        let id = store
            .note_add(
                NewNote {
                    kind: NoteKind::Standard(StandardNoteKind::Fact),
                    scope: Scope {
                        kind: ScopeKind::Repo,
                        value: None,
                    },
                    body: "Plain prose, no heading.".into(),
                },
                user_author(),
            )
            .await
            .unwrap();
        let note = store.note_show(&id).await.unwrap();
        assert_eq!(note.title, None);
    }

    #[tokio::test]
    async fn backfill_titles_populates_legacy_rows_only() {
        // Seed a note normally (title gets populated), then NULL it
        // out behind the store's back to simulate a pre-migration
        // row. Seed a second note that genuinely has no H1; backfill
        // must leave that one NULL.
        let (store, _dir) = fresh_store().await;
        let with_h1 = store
            .note_add(
                NewNote {
                    kind: NoteKind::Standard(StandardNoteKind::Fact),
                    scope: Scope {
                        kind: ScopeKind::Repo,
                        value: None,
                    },
                    body: "# Procedure: cargo build\n\ncargo build --workspace".into(),
                },
                user_author(),
            )
            .await
            .unwrap();
        let no_h1 = store
            .note_add(
                NewNote {
                    kind: NoteKind::Standard(StandardNoteKind::Fact),
                    scope: Scope {
                        kind: ScopeKind::Repo,
                        value: None,
                    },
                    body: "no heading here".into(),
                },
                user_author(),
            )
            .await
            .unwrap();

        sqlx::query("UPDATE memory_notes SET title = NULL WHERE id = ?")
            .bind(&with_h1)
            .execute(store.pool())
            .await
            .unwrap();

        super::backfill_titles(store.pool(), &store.memory_root)
            .await
            .unwrap();

        let n1 = store.note_show(&with_h1).await.unwrap();
        let n2 = store.note_show(&no_h1).await.unwrap();
        assert_eq!(n1.title.as_deref(), Some("Procedure: cargo build"));
        assert_eq!(n2.title, None);

        // Idempotent — second call is a no-op (no NULL rows left).
        super::backfill_titles(store.pool(), &store.memory_root)
            .await
            .unwrap();
        let n1b = store.note_show(&with_h1).await.unwrap();
        assert_eq!(n1b.title.as_deref(), Some("Procedure: cargo build"));
    }
}
