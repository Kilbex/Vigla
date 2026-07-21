//! Bundle composer (V3 §4.7).
//!
//! Two paths:
//! * `compose_manual` — explicit ordered list of note IDs, used by
//!   the supervisor-driven manual-pin UI flow + every test that
//!   needs deterministic input. Renders in caller order, drops the
//!   tail on budget overflow.
//! * `compose_retrieval` (V1.3, orchestrated by
//!   [`crate::memory::MemoryKernel::compose_retrieval`]) — caller
//!   hands in a [`RetrievalBrief`]; the kernel runs the hybrid
//!   scorer + MMR re-ranker to pick note IDs, then delegates to
//!   the same `compose_manual` rendering pipeline. The
//!   `RetrievalBrief` type lives here so callers can build it from
//!   pure data without importing the kernel.
//!
//! Determinism guarantees (verified by tests):
//!
//!   * Same `note_ids` (same order) + same notes in the store ⇒ same
//!     `rendered_block` bytes ⇒ same `bundle_hash`.
//!   * Slots are emitted in the order requested. Budget overflow drops
//!     the tail; the trace records the drop with reason.
//!   * Token budget is read from the adapter, not from a global —
//!     vendor-specific budgets are honoured without composer changes.
//!   * V1.3 contract: same `RetrievalBrief` + same corpus ⇒ same
//!     `bundle_hash`. Provided by MMR's `(score DESC, note_id ASC)`
//!     tie-break.
//!
//! The composer does not write to the worker's worktree. It produces
//! a [`ComposedBundle`] and archives a copy under
//! `<missions_root>/<mission_id>/bundles/<worker_id>/<turn>.md`. The
//! kernel facade is what actually writes the anchor block into the
//! worktree (see `memory/kernel.rs`).

use std::path::{Path, PathBuf};

use serde::Serialize;
use sqlx::SqlitePool;
use tokio::fs;
use tokio::io::AsyncWriteExt;

use event_schema::memory::{BundleSlot, MemoryBundleComposed, MEMORY_SCHEMA_VERSION};
use event_schema::Vendor;

use super::adapter::{estimate_tokens, MemoryAdapter, RenderedSlot};
use super::error::MemoryError;
use super::hierarchy::NoteState;
use super::ids;
use super::store::{hash_hex, MemoryStore};

/// Caller-supplied context per composition.
#[derive(Debug, Clone)]
pub struct BundleBrief {
    pub mission_id: String,
    pub worker_id: String,
    pub turn: u32,
    pub vendor: Vendor,
}

/// V1.3 (Phase 3) — caller-supplied context for the
/// retrieval-driven composer path
/// ([`crate::memory::MemoryKernel::compose_retrieval`]). Extends
/// [`BundleBrief`] with the task-context fields used to construct
/// the retrieval query.
///
/// # Query construction
///
/// The kernel concatenates the task-context fields into a single
/// string (`task_title + " " + task_description + " " +
/// mission_objective + " " + upstream_handoffs.join(" ")`) and
/// hands it to the hybrid scorer + MMR re-ranker. Empty fields are
/// skipped — a minimal `RetrievalBrief` is just `task_title +
/// mission_objective`, which still produces a usable query.
#[derive(Debug, Clone)]
pub struct RetrievalBrief {
    pub mission_id: String,
    pub worker_id: String,
    pub turn: u32,
    pub vendor: Vendor,
    pub task_title: String,
    pub task_description: Option<String>,
    pub mission_objective: String,
    pub upstream_handoffs: Vec<String>,
}

impl RetrievalBrief {
    /// Project to a [`BundleBrief`] for the downstream
    /// `compose_manual` call — the rendering pipeline only needs
    /// the mission/worker/turn/vendor envelope.
    pub fn bundle_brief(&self) -> BundleBrief {
        BundleBrief {
            mission_id: self.mission_id.clone(),
            worker_id: self.worker_id.clone(),
            turn: self.turn,
            vendor: self.vendor,
        }
    }

    /// Build the retrieval query string. Skips empty fields so
    /// callers with a minimal brief still get a meaningful query.
    pub fn query_text(&self) -> String {
        let mut parts: Vec<&str> = Vec::with_capacity(4 + self.upstream_handoffs.len());
        if !self.task_title.is_empty() {
            parts.push(self.task_title.as_str());
        }
        if let Some(d) = self.task_description.as_deref() {
            if !d.is_empty() {
                parts.push(d);
            }
        }
        if !self.mission_objective.is_empty() {
            parts.push(self.mission_objective.as_str());
        }
        for h in &self.upstream_handoffs {
            if !h.is_empty() {
                parts.push(h.as_str());
            }
        }
        parts.join(" ")
    }
}

/// Outcome of [`Composer::compose_manual`]. The composer hands this to
/// the kernel facade, which is responsible for writing the anchor
/// block into the worktree.
#[derive(Debug, Clone)]
pub struct ComposedBundle {
    pub bundle_id: String,
    pub mission_id: String,
    pub worker_id: String,
    pub turn: u32,
    pub vendor: Vendor,
    pub page_table: Vec<BundleSlot>,
    pub rendered_block: String,
    pub block_hash: String,
    pub archived_path: PathBuf,
    pub composed_event_id: String,
    pub trace_json: String,
}

/// Composer entry-point.
#[derive(Debug, Clone)]
pub struct Composer {
    pool: SqlitePool,
    store: MemoryStore,
    missions_root: PathBuf,
}

impl Composer {
    pub fn new(pool: SqlitePool, store: MemoryStore, missions_root: PathBuf) -> Self {
        Self {
            pool,
            store,
            missions_root,
        }
    }

    /// Compose a bundle from an explicit ordered list of note IDs.
    /// Missing or non-readable notes are skipped (trace records why).
    /// Budget overflows truncate the tail.
    pub async fn compose_manual(
        &self,
        brief: &BundleBrief,
        adapter: &dyn MemoryAdapter,
        note_ids: &[String],
    ) -> Result<ComposedBundle, MemoryError> {
        let mut decisions: Vec<TraceDecision> = Vec::with_capacity(note_ids.len());
        let mut chosen: Vec<RenderedSlot> = Vec::with_capacity(note_ids.len());
        let mut page_table: Vec<BundleSlot> = Vec::with_capacity(note_ids.len());
        let max_tokens = adapter.max_tokens();
        let mut tokens_used: u32 = 0;
        let mut slot_idx: u32 = 0;

        // A3 (Tier-2G): one batched SQL fetch + concurrent body reads
        // for the whole candidate set, then a pure in-memory loop to
        // apply policy + budget. Replaces an N+1 pattern that scaled
        // linearly with the candidate count.
        //
        // Notes whose body file or index row is missing are dropped
        // by `notes_by_ids` (logged to stderr); the loop below emits
        // a `not_found` trace decision for any input id absent from
        // the resolved map, preserving the user-observable behaviour
        // of the previous per-note path.
        let mut resolved = self.store.notes_by_ids(note_ids).await?;
        for id in note_ids {
            let note = match resolved.remove(id) {
                Some(n) => n,
                None => {
                    decisions.push(TraceDecision::dropped(id, "not_found"));
                    continue;
                }
            };
            if note.state == NoteState::Invalid || note.state == NoteState::Disputed {
                decisions.push(TraceDecision::dropped(id, "state_unfit"));
                continue;
            }
            let token_est = estimate_tokens(&note.body);
            if tokens_used.saturating_add(token_est) as usize > max_tokens {
                decisions.push(TraceDecision::dropped(id, "budget_exceeded"));
                // Stop scanning further: deterministic and preserves the
                // caller's ordering intent. Retrieval ranks candidates before
                // entering this same tail-drop path.
                break;
            }
            page_table.push(BundleSlot {
                slot: slot_idx,
                note_id: note.id.clone(),
                tokens: token_est,
            });
            chosen.push(RenderedSlot {
                slot: slot_idx,
                note,
                tokens: token_est,
            });
            decisions.push(TraceDecision::picked(id, slot_idx, token_est));
            tokens_used = tokens_used.saturating_add(token_est);
            slot_idx += 1;
        }

        let rendered_block = adapter.render_block_body(&chosen);
        // Hash the canonical anchor-body form: the bytes that will sit
        // *between* the anchor delimiters after `write_anchor_block`
        // frames them. `compose_file_contents` trims trailing
        // newlines, and `find_anchor_span` strips one framing newline
        // on each side at read time. Hashing the trimmed form keeps
        // composer-time hashes equal to drift-check-time hashes — the
        // determinism invariant the bundle archive depends on.
        let block_hash = hash_hex(rendered_block.trim_end_matches('\n').as_bytes());
        let bundle_id = ids::new_bundle_id();
        let composed_event_id = ids::new_memory_event_id();

        let trace = ComposerTrace {
            adapter_max_tokens: max_tokens,
            adapter_vendor: adapter.vendor(),
            tokens_used,
            decisions,
            embedding_model_hash: None,
        };
        let trace_json = serde_json::to_string(&trace)?;

        // Archive the rendered block. This is the byte-replay anchor:
        // future audits read this file plus the page_table row to
        // reconstruct exactly what the worker saw.
        let archived_path = self.archive_path(brief);
        if let Some(parent) = archived_path.parent() {
            fs::create_dir_all(parent).await?;
        }
        write_atomic(&archived_path, rendered_block.as_bytes()).await?;

        // Persist the bundle row + a MemoryBundleComposed event in one tx.
        let now = crate::ids::rfc3339_now();
        let page_table_json = serde_json::to_string(&page_table)?;
        let payload = MemoryBundleComposed {
            bundle_id: bundle_id.clone(),
            mission_id: brief.mission_id.clone(),
            worker_id: brief.worker_id.clone(),
            turn: brief.turn,
            vendor: adapter.vendor(),
            hash: block_hash.clone(),
            page_table: page_table.clone(),
            trace_json: trace_json.clone(),
            prefetched: Vec::new(),
        };
        let payload_json = serde_json::to_string(&payload)?;

        let mut tx = self.pool.begin().await?;
        sqlx::query(
            "INSERT INTO memory_bundles \
             (bundle_id, mission_id, worker_id, turn, vendor, hash, \
              page_table_json, trace_json, rendered_path, composed_event_id) \
             VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?)",
        )
        .bind(&bundle_id)
        .bind(&brief.mission_id)
        .bind(&brief.worker_id)
        .bind(brief.turn as i64)
        .bind(vendor_to_str(adapter.vendor()))
        .bind(&block_hash)
        .bind(&page_table_json)
        .bind(&trace_json)
        .bind(archived_path.to_string_lossy().as_ref())
        .bind(&composed_event_id)
        .execute(&mut *tx)
        .await?;

        sqlx::query(
            "INSERT INTO memory_events \
             (event_id, mission_id, worker_id, ts, type, payload_json, schema_version) \
             VALUES (?, ?, ?, ?, 'bundle_composed', ?, ?)",
        )
        .bind(&composed_event_id)
        .bind(&brief.mission_id)
        .bind(&brief.worker_id)
        .bind(&now)
        .bind(&payload_json)
        .bind(MEMORY_SCHEMA_VERSION)
        .execute(&mut *tx)
        .await?;
        tx.commit().await?;

        Ok(ComposedBundle {
            bundle_id,
            mission_id: brief.mission_id.clone(),
            worker_id: brief.worker_id.clone(),
            turn: brief.turn,
            vendor: adapter.vendor(),
            page_table,
            rendered_block,
            block_hash,
            archived_path,
            composed_event_id,
            trace_json,
        })
    }

    fn archive_path(&self, brief: &BundleBrief) -> PathBuf {
        self.missions_root
            .join(&brief.mission_id)
            .join("bundles")
            .join(&brief.worker_id)
            .join(format!("{turn}.md", turn = brief.turn))
    }
}

// ---------------------------------------------------------------------
// Trace types (V3 §7.3 — every composition decision is recorded)
// ---------------------------------------------------------------------

#[derive(Debug, Clone, Serialize)]
struct ComposerTrace {
    adapter_max_tokens: usize,
    adapter_vendor: Vendor,
    tokens_used: u32,
    decisions: Vec<TraceDecision>,
    embedding_model_hash: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
struct TraceDecision {
    note_id: String,
    outcome: &'static str, // "picked" | "dropped"
    slot: Option<u32>,
    tokens: Option<u32>,
    reason: Option<&'static str>,
}

impl TraceDecision {
    fn picked(note_id: &str, slot: u32, tokens: u32) -> Self {
        Self {
            note_id: note_id.into(),
            outcome: "picked",
            slot: Some(slot),
            tokens: Some(tokens),
            reason: None,
        }
    }
    fn dropped(note_id: &str, reason: &'static str) -> Self {
        Self {
            note_id: note_id.into(),
            outcome: "dropped",
            slot: None,
            tokens: None,
            reason: Some(reason),
        }
    }
}

fn vendor_to_str(v: Vendor) -> &'static str {
    match v {
        Vendor::Claude => "claude",
        Vendor::Codex => "codex",
        Vendor::Gemini => "gemini",
        Vendor::Antigravity => "antigravity",
        Vendor::Kiro => "kiro",
        Vendor::Copilot => "copilot",
        Vendor::Opencode => "opencode",
        Vendor::Mock => "mock",
    }
}

async fn write_atomic(dest: &Path, bytes: &[u8]) -> Result<(), MemoryError> {
    let parent = dest.parent().ok_or_else(|| {
        std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            "archive path has no parent",
        )
    })?;
    fs::create_dir_all(parent).await?;
    let tmp_name = format!(
        ".{}.tmp.{}",
        dest.file_name()
            .and_then(|s| s.to_str())
            .unwrap_or("bundle"),
        uuid::Uuid::now_v7().simple()
    );
    let tmp = parent.join(tmp_name);
    let mut f = fs::File::create(&tmp).await?;
    f.write_all(bytes).await?;
    f.flush().await?;
    f.sync_all().await?;
    drop(f);
    match fs::rename(&tmp, dest).await {
        Ok(()) => Ok(()),
        Err(e) => {
            let _ = fs::remove_file(&tmp).await;
            Err(MemoryError::Io(e))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::memory::adapters::ClaudeMemoryAdapter;
    use crate::memory::hierarchy::{NoteKind, Scope, ScopeKind, StandardNoteKind};
    use crate::memory::{MemoryStore, NewNote, NoteAuthor};
    use event_schema::memory::AuthorSource;
    use sqlx::sqlite::{SqliteConnectOptions, SqlitePoolOptions};
    use std::str::FromStr;
    use tempfile::TempDir;

    async fn fresh_composer() -> (Composer, MemoryStore, TempDir) {
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
        // Post-A1 flat layout: notes live directly under the memory
        // root. No `codex/` wrapper.
        let store = MemoryStore::open(pool.clone(), dir.path().to_path_buf())
            .await
            .unwrap();
        let composer = Composer::new(pool, store.clone(), dir.path().join("missions"));
        (composer, store, dir)
    }

    fn brief() -> BundleBrief {
        BundleBrief {
            mission_id: "add-logout-7a3f".into(),
            worker_id: "worker-a".into(),
            turn: 0,
            vendor: Vendor::Claude,
        }
    }

    async fn add_note(store: &MemoryStore, body: &str, kind: StandardNoteKind) -> String {
        store
            .note_add(
                NewNote {
                    kind: NoteKind::Standard(kind),
                    scope: Scope {
                        kind: ScopeKind::Repo,
                        value: None,
                    },
                    body: body.into(),
                },
                NoteAuthor::User {
                    source: AuthorSource::Cli,
                },
            )
            .await
            .unwrap()
    }

    #[tokio::test]
    async fn determinism_same_inputs_same_block_hash() {
        let (composer, store, _dir) = fresh_composer().await;
        let a = add_note(&store, "alpha body", StandardNoteKind::Fact).await;
        let b = add_note(&store, "beta body", StandardNoteKind::Hazard).await;
        let adapter = ClaudeMemoryAdapter;

        let one = composer
            .compose_manual(&brief(), &adapter, &[a.clone(), b.clone()])
            .await
            .unwrap();
        let two = composer
            .compose_manual(
                &BundleBrief { turn: 1, ..brief() },
                &adapter,
                &[a.clone(), b.clone()],
            )
            .await
            .unwrap();
        // Different bundle_id (fresh) but same rendered bytes ⇒ same hash.
        assert_ne!(one.bundle_id, two.bundle_id);
        assert_eq!(one.block_hash, two.block_hash);
        assert_eq!(one.rendered_block, two.rendered_block);
    }

    #[tokio::test]
    async fn order_affects_output() {
        let (composer, store, _dir) = fresh_composer().await;
        let a = add_note(&store, "alpha body", StandardNoteKind::Fact).await;
        let b = add_note(&store, "beta body", StandardNoteKind::Hazard).await;
        let adapter = ClaudeMemoryAdapter;

        let ab = composer
            .compose_manual(&brief(), &adapter, &[a.clone(), b.clone()])
            .await
            .unwrap();
        let ba = composer
            .compose_manual(
                &BundleBrief { turn: 1, ..brief() },
                &adapter,
                &[b.clone(), a.clone()],
            )
            .await
            .unwrap();
        assert_ne!(ab.block_hash, ba.block_hash);
    }

    #[tokio::test]
    async fn archived_file_is_written_and_byte_matches_rendered_block() {
        let (composer, store, _dir) = fresh_composer().await;
        let n = add_note(&store, "x", StandardNoteKind::Fact).await;
        let adapter = ClaudeMemoryAdapter;
        let bundle = composer
            .compose_manual(&brief(), &adapter, &[n])
            .await
            .unwrap();
        let on_disk = std::fs::read_to_string(&bundle.archived_path).unwrap();
        assert_eq!(on_disk, bundle.rendered_block);
        assert!(bundle
            .archived_path
            .to_string_lossy()
            .contains("bundles/worker-a/0.md"));
    }

    #[tokio::test]
    async fn missing_note_is_dropped_with_trace_reason() {
        let (composer, _codex, _dir) = fresh_composer().await;
        let adapter = ClaudeMemoryAdapter;
        let bundle = composer
            .compose_manual(&brief(), &adapter, &["does-not-exist".into()])
            .await
            .unwrap();
        assert!(bundle.page_table.is_empty());
        assert!(bundle.trace_json.contains("not_found"));
    }

    #[tokio::test]
    async fn budget_overflow_drops_tail() {
        let (composer, store, _dir) = fresh_composer().await;
        // Force overflow by building several large notes.
        let mut ids = Vec::new();
        for i in 0..5 {
            ids.push(
                add_note(
                    &store,
                    &format!("note {i} ").repeat(400),
                    StandardNoteKind::Fact,
                )
                .await,
            );
        }
        let adapter = ClaudeMemoryAdapter; // 1200 tok default
        let bundle = composer
            .compose_manual(&brief(), &adapter, &ids)
            .await
            .unwrap();
        assert!(bundle.page_table.len() < ids.len());
        assert!(bundle.trace_json.contains("budget_exceeded"));
    }

    #[tokio::test]
    async fn bundle_row_and_event_are_persisted() {
        let (composer, store, _dir) = fresh_composer().await;
        let n = add_note(&store, "x", StandardNoteKind::Fact).await;
        let adapter = ClaudeMemoryAdapter;
        let bundle = composer
            .compose_manual(&brief(), &adapter, &[n])
            .await
            .unwrap();

        let (bundle_count,): (i64,) =
            sqlx::query_as("SELECT COUNT(*) FROM memory_bundles WHERE bundle_id = ?")
                .bind(&bundle.bundle_id)
                .fetch_one(&composer.pool)
                .await
                .unwrap();
        assert_eq!(bundle_count, 1);

        let (event_count,): (i64,) = sqlx::query_as(
            "SELECT COUNT(*) FROM memory_events WHERE type = 'bundle_composed' AND event_id = ?",
        )
        .bind(&bundle.composed_event_id)
        .fetch_one(&composer.pool)
        .await
        .unwrap();
        assert_eq!(event_count, 1);
    }

    /// A3 regression: composer with a many-note candidate set must
    /// still succeed and produce a bundle. This doesn't directly
    /// count SQL round-trips (would need a query interceptor), but
    /// asserts the batch path works at scale — 20 candidates that
    /// previously required 20 sequential `note_show` calls now run
    /// in one batched fetch + concurrent body reads.
    #[tokio::test]
    async fn batch_composer_handles_many_candidates() {
        let (composer, store, _dir) = fresh_composer().await;
        let mut ids = Vec::with_capacity(20);
        for i in 0..20 {
            ids.push(add_note(&store, &format!("note-{i}"), StandardNoteKind::Fact).await);
        }
        let adapter = ClaudeMemoryAdapter;
        let bundle = composer
            .compose_manual(&brief(), &adapter, &ids)
            .await
            .unwrap();
        // Some notes drop via budget; that's fine. The structural
        // property is "every candidate got considered, no error,
        // ordered output".
        assert!(!bundle.page_table.is_empty());
        for (idx, slot) in bundle.page_table.iter().enumerate() {
            assert_eq!(slot.slot as usize, idx);
        }
    }
}
