//! Fail-soft "attach memory to a worker's worktree" helper (Tier-2A).
//!
//! This is the bridge between mission lifecycle and the Memory Kernel.
//! Callers from `mission_runtime` invoke it right after a worker
//! worktree is created and immediately before the worker would
//! otherwise start. The function:
//!
//!   1. Lists currently-promoted notes whose scope matches the mission's
//!      worker vendor or applies to the whole repo.
//!   2. Composes a bundle deterministically (V3 Tier-2B — no
//!      embeddings yet; ordering is by SQL recency).
//!   3. Renders the bundle into the worker's native memory file inside
//!      the worktree.
//!
//! **Fail-soft is mandatory.** Any error from listing, composing, or
//! rendering is swallowed and logged to stderr. Memory must never
//! block mission dispatch — a release gate. Callers receive `None`
//! when something went wrong, and the mission proceeds as if memory
//! were not installed.

use std::path::Path;

use event_schema::Vendor;

use super::adapters::{ClaudeMemoryAdapter, CodexMemoryAdapter, GeminiMemoryAdapter};
use super::composer::{BundleBrief, RetrievalBrief};
use super::hierarchy::{ListFilter, NoteState, ScopeKind};
use super::kernel::{MemoryKernel, RenderedBundle};
use super::retrieval::hybrid::DEFAULT_MMR_LAMBDA;
use crate::mission_event::{ComposeSource, MissionEventKind};

/// Resolve a worker-model string (from `MissionSpec.worker_model`) to
/// a vendor + a stack-allocated adapter. Unknown / "auto" maps to
/// Claude as the safe default (its `CLAUDE.md` is the most widely
/// understood native file across the broader ecosystem).
pub fn vendor_for_model(model: Option<&str>) -> Vendor {
    match model.unwrap_or("auto") {
        "claude" | "Claude" => Vendor::Claude,
        "codex" | "Codex" => Vendor::Codex,
        "gemini" | "Gemini" => Vendor::Gemini,
        _ => Vendor::Claude,
    }
}

/// Compose + render a memory bundle into the worker's worktree, with
/// no error propagation. Returns `Some(RenderedBundle)` on success,
/// `None` if anything went wrong (the error is logged to stderr).
pub async fn attach_to_worktree(
    kernel: &MemoryKernel,
    mission_id: &str,
    worker_id: &str,
    turn: u32,
    worker_vendor: Vendor,
    worktree: &Path,
) -> Option<RenderedBundle> {
    let note_ids = match select_promoted_notes(kernel, worker_vendor).await {
        Ok(ids) => ids,
        Err(e) => {
            tracing::warn!("vigla: memory attach skipped — promoted-note query failed: {e}");
            return None;
        }
    };

    if note_ids.is_empty() {
        // Nothing to render. Still write an empty anchor block so the
        // worker file shape is consistent and drift detection has a
        // ground truth on the next turn.
    }

    let brief = BundleBrief {
        mission_id: mission_id.to_owned(),
        worker_id: worker_id.to_owned(),
        turn,
        vendor: worker_vendor,
    };

    let result = match worker_vendor {
        Vendor::Claude
        | Vendor::Mock
        | Vendor::Opencode
        | Vendor::Antigravity
        | Vendor::Kiro
        | Vendor::Copilot => {
            kernel
                .render_for_worker(&brief, &ClaudeMemoryAdapter, &note_ids, worktree)
                .await
        }
        Vendor::Codex => {
            kernel
                .render_for_worker(&brief, &CodexMemoryAdapter, &note_ids, worktree)
                .await
        }
        Vendor::Gemini => {
            kernel
                .render_for_worker(&brief, &GeminiMemoryAdapter, &note_ids, worktree)
                .await
        }
    };

    match result {
        Ok(rendered) => Some(rendered),
        Err(e) => {
            tracing::warn!(
                "vigla: memory attach skipped — render failed for mission {mission_id}, worker {worker_id}: {e}"
            );
            None
        }
    }
}

/// V1.3 retrieval-driven attach. Sibling to [`attach_to_worktree`]
/// that takes a [`RetrievalBrief`] instead of selecting promoted
/// notes by SQL recency. The kernel runs BM25 → hybrid → MMR to
/// pick the note ids, then composes + renders + archives + writes
/// the anchor block the same way the manual path does.
///
/// Fail-soft contract is identical to [`attach_to_worktree`]:
/// any error (kernel build failure, SQL error, render error) is
/// logged to stderr and `None` is returned. Memory must never
/// block mission dispatch.
///
/// When `emit_fn` is supplied, a `ContextBundleComposed`
/// telemetry event fires after a successful render with
/// `source: Retrieval`. `mmr_lambda` is `Some(λ)` iff MMR
/// actually ran (i.e. at least one candidate had a stored
/// embedding); `None` when the retrieval path fell back to
/// BM25-only (no vectors yet, or `embeddings` feature off).
/// `candidate_count` is the post-MMR chosen-id list length,
/// **before** any composer-side budget truncation.
pub async fn attach_with_retrieval(
    kernel: &MemoryKernel,
    brief: &RetrievalBrief,
    worktree: &Path,
    emit_fn: Option<&(dyn Fn(MissionEventKind) + Send + Sync)>,
) -> Option<RenderedBundle> {
    let result = match brief.vendor {
        Vendor::Claude
        | Vendor::Mock
        | Vendor::Opencode
        | Vendor::Antigravity
        | Vendor::Kiro
        | Vendor::Copilot => {
            kernel
                .render_for_worker_with_retrieval(brief, &ClaudeMemoryAdapter, worktree)
                .await
        }
        Vendor::Codex => {
            kernel
                .render_for_worker_with_retrieval(brief, &CodexMemoryAdapter, worktree)
                .await
        }
        Vendor::Gemini => {
            kernel
                .render_for_worker_with_retrieval(brief, &GeminiMemoryAdapter, worktree)
                .await
        }
    };

    match result {
        Ok((rendered, telemetry)) => {
            if let Some(emit) = emit_fn {
                emit(MissionEventKind::ContextBundleComposed {
                    bundle_id: rendered.bundle_id.clone(),
                    source: ComposeSource::Retrieval,
                    candidate_count: telemetry.chosen_count,
                    mmr_lambda: telemetry.mmr_applied.then_some(DEFAULT_MMR_LAMBDA),
                });
            }
            Some(rendered)
        }
        Err(e) => {
            tracing::warn!(
                "vigla: memory retrieval attach skipped — render failed for mission {}, worker {}: {e}",
                brief.mission_id,
                brief.worker_id
            );
            None
        }
    }
}

/// Tier-2B simple automatic composer: list promoted notes whose scope
/// applies to this worker, ordered newest-first by `created_at`.
///
///   * `scope_kind = 'repo'` — applies to every worker in the repo.
///   * `scope_kind = 'vendor' AND scope_value = <worker vendor>` —
///     applies only to workers of this vendor.
///
/// `path` scopes are not selected automatically here — they need a
/// concept of "files this mission will touch" which P4 introduces.
async fn select_promoted_notes(
    kernel: &MemoryKernel,
    vendor: Vendor,
) -> Result<Vec<String>, super::error::MemoryError> {
    let repo_scoped = kernel
        .store
        .note_list(ListFilter {
            state: Some(NoteState::Promoted),
            scope_kind: Some(ScopeKind::Repo),
            ..Default::default()
        })
        .await?;

    let vendor_str = vendor_to_str(vendor);
    let vendor_scoped = kernel
        .store
        .note_list(ListFilter {
            state: Some(NoteState::Promoted),
            scope_kind: Some(ScopeKind::Vendor),
            scope_value: Some(vendor_str.to_owned()),
            ..Default::default()
        })
        .await?;

    let mut ids: Vec<String> = repo_scoped.into_iter().map(|s| s.id).collect();
    ids.extend(vendor_scoped.into_iter().map(|s| s.id));
    Ok(ids)
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

/// Per-worker token budget, in chars/4-style estimated tokens.
/// The composer already honours the adapter's `max_tokens` as a
/// hard ceiling; this budget is a softer per-worker override the
/// supervisor (or user envelope, future) can dial down.
///
/// Default for new callers is 8000 tokens — generous enough that
/// most missions never hit it, but tight enough that runaway
/// bundle growth doesn't crowd out the worker's actual task.
pub const DEFAULT_TOKEN_BUDGET: usize = 8_000;

/// Per-worker token budget enforcer.
///
/// V2 (S9): when the natural render exceeds `token_budget`, the
/// over-budget archive is deleted via
/// [`MemoryKernel::delete_bundle`] and re-rendered through the
/// composer with only the note-id prefix that fits the budget. Two
/// telemetry events fire on the mission stream:
///
///   * [`crate::mission_event::MissionEventKind::ContextBudgetExceeded`]
///     — observed overflow.
///   * [`crate::mission_event::MissionEventKind::ContextBudgetTruncated`]
///     — re-render succeeded; carries dropped note ids.
///
/// The delete-then-re-render path preserves the
/// `memory_bundles(mission_id, worker_id, turn)` UNIQUE invariant: only the
/// truncated bundle is archived. Replay sees the
/// `ContextBudgetExceeded` event on the mission stream as the
/// signal that truncation occurred — the over-budget bundle row
/// itself is discarded.
///
/// On re-render failure (rare — composer would have to transiently
/// fail after the delete), an empty fallback is logged to stderr
/// and `None` is returned; the worker's native file is left
/// without an anchor block, which is the same fail-soft contract
/// as [`attach_to_worktree`] on a render error.
pub async fn attach_to_worktree_with_budget(
    kernel: &MemoryKernel,
    mission_id: &str,
    worker_id: &str,
    turn: u32,
    worker_vendor: Vendor,
    worktree: &Path,
    token_budget: Option<usize>,
    emit_fn: Option<Box<dyn Fn(crate::mission_event::MissionEventKind) + Send + Sync>>,
) -> Option<RenderedBundle> {
    let rendered =
        attach_to_worktree(kernel, mission_id, worker_id, turn, worker_vendor, worktree).await?;

    // Helper: fire the Manual ContextBundleComposed telemetry.
    let emit_composed = |b: &RenderedBundle, emit: &(dyn Fn(MissionEventKind) + Send + Sync)| {
        emit(MissionEventKind::ContextBundleComposed {
            bundle_id: b.bundle_id.clone(),
            source: ComposeSource::Manual,
            candidate_count: b.page_table.len() as u32,
            mmr_lambda: None,
        });
    };

    let budget = match token_budget {
        Some(b) => b,
        None => {
            if let Some(emit) = emit_fn.as_ref() {
                emit_composed(&rendered, emit.as_ref());
            }
            return Some(rendered);
        }
    };

    let requested: u32 = rendered.page_table.iter().map(|s| s.tokens).sum();
    if (requested as usize) <= budget {
        if let Some(emit) = emit_fn.as_ref() {
            emit_composed(&rendered, emit.as_ref());
        }
        return Some(rendered);
    }

    // Build the keep / drop split from the over-budget page table.
    let mut keep_tokens: u32 = 0;
    let mut keep_note_ids: Vec<String> = Vec::new();
    let mut dropped_note_ids: Vec<String> = Vec::new();
    for slot in &rendered.page_table {
        if keep_tokens.saturating_add(slot.tokens) as usize <= budget {
            keep_tokens = keep_tokens.saturating_add(slot.tokens);
            keep_note_ids.push(slot.note_id.clone());
        } else {
            dropped_note_ids.push(slot.note_id.clone());
        }
    }
    let dropped_count: u32 = dropped_note_ids.len() as u32;

    // Step 1 telemetry: observed overflow (kept on the mission
    // event stream even though the over-budget archive is about
    // to be discarded — this is the replay signal that the
    // truncation pipeline ran).
    if let Some(emit) = emit_fn.as_ref() {
        emit(
            crate::mission_event::MissionEventKind::ContextBudgetExceeded {
                worker_id: worker_id.to_owned(),
                requested_tokens: requested,
                granted_tokens: keep_tokens,
                dropped_entries: dropped_count,
            },
        );
    } else {
        tracing::warn!(
            "vigla: memory attach overflow for worker {worker_id}: \
             requested={requested} granted={keep_tokens} dropped={dropped_count}"
        );
    }

    // Erase the over-budget archive so the (worker_id, turn)
    // UNIQUE index lets the truncated re-render INSERT succeed.
    // The ContextBudgetExceeded telemetry above is what replay
    // uses to know the overflow happened.
    if let Err(e) = kernel.delete_bundle(&rendered.bundle_id).await {
        tracing::error!(
            "vigla: memory attach truncate delete-bundle failed for worker {worker_id}: {e}"
        );
        // Fall back to the original (over-budget) bundle — the
        // worker still has something usable; the exceeded
        // telemetry already fired.
        return Some(rendered);
    }

    // Step 2: re-render with the truncated keep-list. compose_manual
    // INSERTs a fresh bundle row under the same (worker_id, turn)
    // pair, which is now free.
    let brief = BundleBrief {
        mission_id: mission_id.to_owned(),
        worker_id: worker_id.to_owned(),
        turn,
        vendor: worker_vendor,
    };

    let truncated = match worker_vendor {
        Vendor::Claude
        | Vendor::Mock
        | Vendor::Opencode
        | Vendor::Antigravity
        | Vendor::Kiro
        | Vendor::Copilot => {
            kernel
                .render_for_worker(&brief, &ClaudeMemoryAdapter, &keep_note_ids, worktree)
                .await
        }
        Vendor::Codex => {
            kernel
                .render_for_worker(&brief, &CodexMemoryAdapter, &keep_note_ids, worktree)
                .await
        }
        Vendor::Gemini => {
            kernel
                .render_for_worker(&brief, &GeminiMemoryAdapter, &keep_note_ids, worktree)
                .await
        }
    };

    let truncated = match truncated {
        Ok(b) => b,
        Err(e) => {
            tracing::warn!(
                "vigla: memory attach truncated re-render failed for worker {worker_id}: {e}"
            );
            // Re-render failed AFTER we deleted the original. No
            // bundle in the archive; the worker's anchor block
            // also got removed on the original render's
            // write_anchor_block path. This is the same fail-soft
            // contract as a primary render failure — return None
            // so the caller sees "memory attach failed" rather
            // than a half-baked bundle.
            return None;
        }
    };

    let rendered_bytes: u32 = truncated.page_table.iter().map(|s| s.tokens).sum();
    let original_bytes: u32 = requested;

    // Step 3 telemetry: re-render succeeded.
    if let Some(emit) = emit_fn.as_ref() {
        emit(
            crate::mission_event::MissionEventKind::ContextBudgetTruncated {
                worker_id: worker_id.to_owned(),
                original_bytes,
                rendered_bytes,
                dropped_note_ids,
            },
        );
        // The truncated bundle is the one the worker actually
        // reads — emit ContextBundleComposed for it (not for the
        // discarded over-budget bundle).
        emit_composed(&truncated, emit.as_ref());
    } else {
        tracing::warn!(
            "vigla: memory attach truncated for worker {worker_id}: \
             original_bytes={original_bytes} rendered_bytes={rendered_bytes}"
        );
    }

    Some(truncated)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::memory::hierarchy::{NoteKind, Scope, StandardNoteKind};
    use crate::memory::NewNote;
    use event_schema::memory::AuthorSource;
    use sqlx::sqlite::{SqliteConnectOptions, SqlitePoolOptions};
    use std::str::FromStr;
    use tempfile::TempDir;

    async fn fresh_kernel() -> (MemoryKernel, TempDir, TempDir) {
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
        let vigla_root = TempDir::new().unwrap();
        let worktree = TempDir::new().unwrap();
        let kernel = MemoryKernel::open(pool, vigla_root.path().to_path_buf())
            .await
            .unwrap();
        (kernel, vigla_root, worktree)
    }

    #[test]
    fn vendor_for_model_maps_string_names() {
        assert_eq!(vendor_for_model(Some("claude")), Vendor::Claude);
        assert_eq!(vendor_for_model(Some("codex")), Vendor::Codex);
        assert_eq!(vendor_for_model(Some("gemini")), Vendor::Gemini);
        assert_eq!(vendor_for_model(Some("auto")), Vendor::Claude);
        assert_eq!(vendor_for_model(Some("unknown")), Vendor::Claude);
        assert_eq!(vendor_for_model(None), Vendor::Claude);
    }

    #[tokio::test]
    async fn sequential_missions_can_attach_the_same_task_index() {
        let (kernel, _root, worktrees) = fresh_kernel().await;
        kernel
            .pin_note(crate::memory::PinInput {
                kind: NoteKind::Standard(StandardNoteKind::Fact),
                scope: Scope {
                    kind: ScopeKind::Repo,
                    value: None,
                },
                body: "shared repository context".into(),
                source: AuthorSource::Cli,
            })
            .await
            .unwrap();
        let first_worktree = worktrees.path().join("mission-1");
        let second_worktree = worktrees.path().join("mission-2");
        std::fs::create_dir_all(&first_worktree).unwrap();
        std::fs::create_dir_all(&second_worktree).unwrap();

        let first = attach_to_worktree(
            &kernel,
            "mission-1",
            "mock-1",
            0,
            Vendor::Claude,
            &first_worktree,
        )
        .await
        .expect("first mission bundle");
        let second = attach_to_worktree(
            &kernel,
            "mission-2",
            "mock-1",
            0,
            Vendor::Claude,
            &second_worktree,
        )
        .await
        .expect("second mission bundle");

        assert_ne!(first.bundle_id, second.bundle_id);
        assert!(std::fs::read_to_string(second.native_file_path)
            .unwrap()
            .contains("vigla:memory:begin v1"));
        let count: (i64,) = sqlx::query_as(
            "SELECT COUNT(*) FROM memory_bundles WHERE worker_id = 'mock-1' AND turn = 0",
        )
        .fetch_one(&kernel.pool)
        .await
        .unwrap();
        assert_eq!(count.0, 2);
    }

    // V1.3: attach_with_retrieval picks promoted notes via the
    // retrieval pipeline, renders into the worktree, and emits a
    // `ContextBundleComposed { source: Retrieval, mmr_lambda: Some }`
    // telemetry event.
    #[tokio::test]
    async fn attach_with_retrieval_renders_and_emits_retrieval_telemetry() {
        let (kernel, _root, worktree) = fresh_kernel().await;
        // Pin one repo-scoped note so the corpus is non-empty.
        kernel
            .pin_note(crate::memory::PinInput {
                kind: NoteKind::Standard(StandardNoteKind::Decision),
                scope: Scope {
                    kind: ScopeKind::Repo,
                    value: None,
                },
                body: "Logout must clear the session cookie before redirect.".into(),
                source: AuthorSource::Cli,
            })
            .await
            .unwrap();

        let captured = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
        let captured2 = std::sync::Arc::clone(&captured);
        let emit_fn = move |ev: crate::mission_event::MissionEventKind| {
            captured2.lock().unwrap().push(ev);
        };

        let brief = crate::memory::RetrievalBrief {
            mission_id: "mid-retrieval-1".into(),
            worker_id: "mock-1".into(),
            turn: 0,
            vendor: Vendor::Claude,
            task_title: "Add logout button".into(),
            task_description: None,
            mission_objective: "Improve session security".into(),
            upstream_handoffs: Vec::new(),
        };
        let rendered = attach_with_retrieval(&kernel, &brief, worktree.path(), Some(&emit_fn))
            .await
            .expect("retrieval attach renders bundle");

        // Anchor file written into the worktree.
        assert!(rendered.native_file_path.exists());

        // Exactly one ContextBundleComposed(Retrieval) telemetry.
        let events = captured.lock().unwrap();
        let composed: Vec<_> = events
            .iter()
            .filter_map(|ev| match ev {
                crate::mission_event::MissionEventKind::ContextBundleComposed {
                    source,
                    mmr_lambda,
                    ..
                } => Some((*source, *mmr_lambda)),
                _ => None,
            })
            .collect();
        assert_eq!(
            composed.len(),
            1,
            "expected one ContextBundleComposed event"
        );
        assert_eq!(
            composed[0].0,
            crate::mission_event::ComposeSource::Retrieval
        );
        // No embeddings stored in this test setup (no `embeddings`
        // feature seeded vectors), so MMR is bypassed and the
        // retrieval path must report `mmr_lambda: None` to let
        // replay distinguish hybrid+MMR from BM25-only fallback.
        assert!(
            composed[0].1.is_none(),
            "BM25-only retrieval fallback must report mmr_lambda=None; got {:?}",
            composed[0].1
        );
    }

    #[tokio::test]
    async fn attach_writes_promoted_note_into_worktree() {
        let (kernel, _root, worktree) = fresh_kernel().await;

        // Pin a note — user-oracle path promotes immediately.
        let outcome = kernel
            .pin_note(crate::memory::PinInput {
                kind: NoteKind::Standard(StandardNoteKind::Hazard),
                scope: Scope {
                    kind: ScopeKind::Repo,
                    value: None,
                },
                body: "Always run tests before merging.".into(),
                source: AuthorSource::Cli,
            })
            .await
            .unwrap();
        assert!(matches!(
            outcome,
            crate::memory::PinOutcome::Pinned { promoted: true, .. }
        ));

        let rendered = attach_to_worktree(
            &kernel,
            "mission-x",
            "worker-1",
            0,
            Vendor::Claude,
            worktree.path(),
        )
        .await
        .expect("attach should succeed");

        let claude_md = std::fs::read_to_string(&rendered.native_file_path).unwrap();
        assert!(claude_md.contains("Always run tests"));
        assert!(claude_md.contains("hazard:"));
    }

    #[tokio::test]
    async fn attach_skips_owned_only_notes() {
        let (kernel, _root, worktree) = fresh_kernel().await;
        // note_add records UserAuthored which promotes through the
        // shortcut — for this test we want a note that *isn't*
        // promoted. Use the test-only seed helper.
        kernel
            .store
            ._test_seed_owned_note(NewNote {
                kind: NoteKind::Standard(StandardNoteKind::Fact),
                scope: Scope {
                    kind: ScopeKind::Repo,
                    value: None,
                },
                body: "this note never promoted".into(),
            })
            .await
            .unwrap();

        let rendered = attach_to_worktree(
            &kernel,
            "mission-x",
            "worker-1",
            0,
            Vendor::Claude,
            worktree.path(),
        )
        .await
        .expect("attach renders an empty block when no notes promoted");

        let claude_md = std::fs::read_to_string(&rendered.native_file_path).unwrap();
        assert!(!claude_md.contains("this note never promoted"));
        // Still writes the anchor + placeholder, so drift detection
        // has a ground truth.
        assert!(claude_md.contains("vigla:memory:begin v1"));
        assert!(claude_md.contains("no notes selected"));
    }

    #[tokio::test]
    async fn attach_picks_vendor_scoped_notes_for_matching_vendor() {
        let (kernel, _root, worktree) = fresh_kernel().await;
        kernel
            .pin_note(crate::memory::PinInput {
                kind: NoteKind::Standard(StandardNoteKind::Hazard),
                scope: Scope {
                    kind: ScopeKind::Vendor,
                    value: Some("claude".into()),
                },
                body: "Claude-specific gotcha.".into(),
                source: AuthorSource::Cli,
            })
            .await
            .unwrap();
        kernel
            .pin_note(crate::memory::PinInput {
                kind: NoteKind::Standard(StandardNoteKind::Hazard),
                scope: Scope {
                    kind: ScopeKind::Vendor,
                    value: Some("gemini".into()),
                },
                body: "Gemini-only thing.".into(),
                source: AuthorSource::Cli,
            })
            .await
            .unwrap();

        // Claude worker sees claude-scoped only.
        let claude_rendered = attach_to_worktree(
            &kernel,
            "mission-x",
            "worker-c",
            0,
            Vendor::Claude,
            &worktree.path().join("cl"),
        )
        .await
        .unwrap();
        let cl = std::fs::read_to_string(&claude_rendered.native_file_path).unwrap();
        assert!(cl.contains("Claude-specific gotcha"));
        assert!(!cl.contains("Gemini-only thing"));

        // Gemini worker sees gemini-scoped only.
        let gemini_rendered = attach_to_worktree(
            &kernel,
            "mission-x",
            "worker-g",
            0,
            Vendor::Gemini,
            &worktree.path().join("gm"),
        )
        .await
        .unwrap();
        let gm = std::fs::read_to_string(&gemini_rendered.native_file_path).unwrap();
        assert!(gm.contains("Gemini-only thing"));
        assert!(!gm.contains("Claude-specific gotcha"));
    }

    #[tokio::test]
    async fn attach_under_budget_emits_no_truncation_event() {
        let (kernel, _root, worktree) = fresh_kernel().await;
        let _ = kernel
            .pin_note(crate::memory::PinInput {
                kind: NoteKind::Standard(StandardNoteKind::Fact),
                scope: Scope {
                    kind: ScopeKind::Repo,
                    value: None,
                },
                body: "small".into(),
                source: AuthorSource::Cli,
            })
            .await
            .unwrap();

        let captured = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
        let captured2 = std::sync::Arc::clone(&captured);
        let emit_fn: Box<dyn Fn(crate::mission_event::MissionEventKind) + Send + Sync> =
            Box::new(move |ev| captured2.lock().unwrap().push(ev));

        let rendered = attach_to_worktree_with_budget(
            &kernel,
            "mid-1",
            "mock-1",
            0,
            Vendor::Claude,
            worktree.path(),
            Some(100_000),
            Some(emit_fn),
        )
        .await
        .expect("attach");
        let _ = rendered;
        // V1.3: ContextBundleComposed fires unconditionally on every
        // compose, so the captured stream is no longer empty — but
        // there must be no ContextBudgetExceeded / Truncated events
        // because the budget was generous.
        let events = captured.lock().unwrap();
        for ev in events.iter() {
            assert!(
                !matches!(
                    ev,
                    crate::mission_event::MissionEventKind::ContextBudgetExceeded { .. }
                        | crate::mission_event::MissionEventKind::ContextBudgetTruncated { .. }
                ),
                "no truncation events expected under generous budget, got {ev:?}"
            );
        }
        // Sanity: the Manual ContextBundleComposed telemetry fired.
        assert!(
            events.iter().any(|ev| matches!(
                ev,
                crate::mission_event::MissionEventKind::ContextBundleComposed {
                    source: crate::mission_event::ComposeSource::Manual,
                    ..
                }
            )),
            "expected one ContextBundleComposed(Manual) event under generous budget"
        );
    }

    #[tokio::test]
    async fn attach_over_budget_truncates_and_emits_event() {
        let (kernel, _root, worktree) = fresh_kernel().await;
        for i in 0..5 {
            let _ = kernel
                .pin_note(crate::memory::PinInput {
                    kind: NoteKind::Standard(StandardNoteKind::Fact),
                    scope: Scope {
                        kind: ScopeKind::Repo,
                        value: None,
                    },
                    body: format!("note number {i} ").repeat(200),
                    source: AuthorSource::Cli,
                })
                .await
                .unwrap();
        }

        let captured = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
        let captured2 = std::sync::Arc::clone(&captured);
        let emit_fn: Box<dyn Fn(crate::mission_event::MissionEventKind) + Send + Sync> =
            Box::new(move |ev| captured2.lock().unwrap().push(ev));

        let _rendered = attach_to_worktree_with_budget(
            &kernel,
            "mid-2",
            "mock-1",
            0,
            Vendor::Claude,
            worktree.path(),
            Some(50),
            Some(emit_fn),
        )
        .await
        .expect("attach");

        let events = captured.lock().unwrap();
        let exceeded = events
            .iter()
            .filter(|ev| {
                matches!(
                    ev,
                    crate::mission_event::MissionEventKind::ContextBudgetExceeded { .. }
                )
            })
            .count();
        assert!(
            exceeded >= 1,
            "expected at least one ContextBudgetExceeded event"
        );
    }

    /// V2 (S9): tight budget → page_table fits AND both
    /// ContextBudgetExceeded + ContextBudgetTruncated events fire.
    /// The over-budget archive is deleted; only the truncated
    /// bundle survives in memory_bundles.
    #[tokio::test]
    async fn attach_with_tight_budget_truncates_and_emits_event() {
        let (kernel, _root, worktree) = fresh_kernel().await;
        for i in 0..6 {
            let _ = kernel
                .pin_note(crate::memory::PinInput {
                    kind: NoteKind::Standard(StandardNoteKind::Fact),
                    scope: Scope {
                        kind: ScopeKind::Repo,
                        value: None,
                    },
                    body: format!("large note {i} body ").repeat(50),
                    source: AuthorSource::Cli,
                })
                .await
                .unwrap();
        }

        let captured = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
        let captured2 = std::sync::Arc::clone(&captured);
        let emit_fn: Box<dyn Fn(crate::mission_event::MissionEventKind) + Send + Sync> =
            Box::new(move |ev| captured2.lock().unwrap().push(ev));

        let rendered = attach_to_worktree_with_budget(
            &kernel,
            "mid-tight",
            "worker-tight",
            0,
            Vendor::Claude,
            worktree.path(),
            Some(200),
            Some(emit_fn),
        )
        .await
        .expect("attach should succeed after V2 re-render");

        let total: u32 = rendered.page_table.iter().map(|s| s.tokens).sum();
        assert!(
            total as usize <= 200,
            "page_table tokens {total} must be ≤ budget 200 after V2 re-render",
        );

        {
            let events = captured.lock().unwrap();
            let kinds: Vec<_> = events.iter().map(std::mem::discriminant).collect();
            let exceeded_disc = std::mem::discriminant(
                &crate::mission_event::MissionEventKind::ContextBudgetExceeded {
                    worker_id: "x".into(),
                    requested_tokens: 0,
                    granted_tokens: 0,
                    dropped_entries: 0,
                },
            );
            let truncated_disc = std::mem::discriminant(
                &crate::mission_event::MissionEventKind::ContextBudgetTruncated {
                    worker_id: "x".into(),
                    original_bytes: 0,
                    rendered_bytes: 0,
                    dropped_note_ids: vec![],
                },
            );
            assert!(
                kinds.contains(&exceeded_disc),
                "expected ContextBudgetExceeded event"
            );
            assert!(
                kinds.contains(&truncated_disc),
                "expected ContextBudgetTruncated event"
            );
        }

        // Only the truncated bundle should remain in memory_bundles
        // for this (mission_id, worker_id, turn). The over-budget one was deleted.
        let count: (i64,) =
            sqlx::query_as(
                "SELECT COUNT(*) FROM memory_bundles WHERE mission_id = ? AND worker_id = ? AND turn = ?",
            )
                .bind("mid-tight")
                .bind("worker-tight")
                .bind(0i64)
                .fetch_one(&kernel.pool)
                .await
                .unwrap();
        assert_eq!(
            count.0, 1,
            "exactly one bundle row survives the V2 re-render"
        );
    }

    /// V2 (S9): a generous budget must NOT emit truncation telemetry.
    #[tokio::test]
    async fn attach_within_budget_does_not_emit_truncation() {
        let (kernel, _root, worktree) = fresh_kernel().await;
        let _ = kernel
            .pin_note(crate::memory::PinInput {
                kind: NoteKind::Standard(StandardNoteKind::Fact),
                scope: Scope {
                    kind: ScopeKind::Repo,
                    value: None,
                },
                body: "small note".into(),
                source: AuthorSource::Cli,
            })
            .await
            .unwrap();

        let captured = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
        let captured2 = std::sync::Arc::clone(&captured);
        let emit_fn: Box<dyn Fn(crate::mission_event::MissionEventKind) + Send + Sync> =
            Box::new(move |ev| captured2.lock().unwrap().push(ev));

        let _ = attach_to_worktree_with_budget(
            &kernel,
            "mid-roomy",
            "worker-roomy",
            0,
            Vendor::Claude,
            worktree.path(),
            Some(10_000),
            Some(emit_fn),
        )
        .await
        .expect("attach");

        let events = captured.lock().unwrap();
        for ev in events.iter() {
            assert!(
                !matches!(
                    ev,
                    crate::mission_event::MissionEventKind::ContextBudgetTruncated { .. }
                ),
                "did not expect ContextBudgetTruncated for in-budget bundle: got {ev:?}"
            );
        }
    }
}
