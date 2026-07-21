//! End-to-end closed-loop tests for the memory kernel (P2 hardening).
//!
//! Each test spins up a real kernel against `sqlite::memory:` + a
//! `TempDir` for the codex, drives it through public API calls, and
//! verifies the observable outcome. No mocks of internal modules.
//!
//! Per-feature closed-loop tests are added in their gauntlet phases.

use orchestrator::memory::MemoryKernel;
use sqlx::sqlite::SqliteConnectOptions;
use sqlx::SqlitePool;
use std::str::FromStr;
use tempfile::TempDir;

async fn fresh_kernel() -> (MemoryKernel, TempDir) {
    let opts = SqliteConnectOptions::from_str("sqlite::memory:")
        .unwrap()
        .create_if_missing(true);
    let pool = SqlitePool::connect_with(opts).await.unwrap();
    sqlx::migrate!("./migrations").run(&pool).await.unwrap();
    let dir = TempDir::new().unwrap();
    let kernel = MemoryKernel::open(pool, dir.path().to_path_buf())
        .await
        .unwrap();
    (kernel, dir)
}

#[tokio::test]
async fn fresh_kernel_opens_and_root_is_accessible() {
    let (kernel, _dir) = fresh_kernel().await;
    let root = kernel.root();
    assert!(root.exists(), "kernel root {root:?} should exist");
}

#[tokio::test]
async fn migrations_are_idempotent_under_double_run() {
    let opts = SqliteConnectOptions::from_str("sqlite::memory:")
        .unwrap()
        .create_if_missing(true);
    let pool = SqlitePool::connect_with(opts).await.unwrap();
    // First run.
    sqlx::migrate!("./migrations").run(&pool).await.unwrap();
    // Second run — sqlx tracks _sqlx_migrations and skips. Verify no error.
    sqlx::migrate!("./migrations")
        .run(&pool)
        .await
        .expect("second run must succeed");
}

#[tokio::test]
async fn closed_loop_scanner_rejects_secret_before_pending_persists() {
    use event_schema::memory::{NoteKind, Scope, ScopeKind};
    use orchestrator::memory::{ProposalInput, ProposalOutcome};

    let (kernel, _dir) = fresh_kernel().await;

    // Body contains a credential shape — scanner must intercept.
    let outcome = kernel
        .on_proposal(ProposalInput {
            mission_id: "m-scan-1".to_owned(),
            worker_id: "w-scan-1".to_owned(),
            kind: NoteKind::from_str("fact"),
            scope: Scope {
                kind: ScopeKind::Repo,
                value: None,
            },
            body: "leaked: AKIAIOSFODNN7EXAMPLE in the config".to_owned(),
            derived_from: vec![],
            evidence_event_ids: vec![],
        })
        .await
        .expect("on_proposal should succeed even when scanner rejects");

    // ProposalOutcome is an enum: Accepted | Rejected. Scanner-rejected
    // bodies emit a proposal_id (for the rejection event), but the
    // variant must be Rejected.
    assert!(
        matches!(outcome, ProposalOutcome::Rejected { .. }),
        "scanner rejection should yield ProposalOutcome::Rejected, got {outcome:?}"
    );

    // Sanity: memory_pending must be empty for this mission.
    let row: (i64,) = sqlx::query_as("SELECT COUNT(*) FROM memory_pending WHERE mission_id = ?")
        .bind("m-scan-1")
        .fetch_one(kernel.pool())
        .await
        .unwrap();
    assert_eq!(row.0, 0, "memory_pending unexpectedly contains rows");
}

#[tokio::test]
async fn closed_loop_witness_lifts_confidence_above_threshold() {
    use event_schema::memory::{AuthorSource, NoteKind, Scope, ScopeKind, WitnessKind};
    use orchestrator::memory::{confidence, witnesses::record, PinInput, PinOutcome, Witness};

    let (kernel, _dir) = fresh_kernel().await;

    // Pin a note directly (skip ratification — we want a real note to score).
    let pinned = kernel
        .pin_note(PinInput {
            kind: NoteKind::from_str("fact"),
            scope: Scope {
                kind: ScopeKind::Repo,
                value: None,
            },
            body: "a fact to score".to_owned(),
            source: AuthorSource::Cli,
        })
        .await
        .unwrap();
    let note_id = match pinned {
        PinOutcome::Pinned { note_id, .. } => note_id,
        other => panic!("pin should succeed for clean body, got {other:?}"),
    };

    // Record 5 positive witnesses.
    for i in 0..5 {
        let src = format!("ev-test-{i}");
        record(kernel.pool(), &note_id, WitnessKind::WorkerProposed, &src)
            .await
            .unwrap();
    }

    // Pull witnesses out of the DB and score directly.
    let rows: Vec<(String, String, String, f64, String, String)> = sqlx::query_as(
        "SELECT id, note_id, kind, weight, source_event_id, observed_at \
         FROM memory_witnesses WHERE note_id = ?",
    )
    .bind(&note_id)
    .fetch_all(kernel.pool())
    .await
    .unwrap();

    // Hydrate Witness structs. WitnessKind::from_str returns Option<Self>;
    // fall back to WorkerProposed for unknown strings (we only inserted that kind).
    let witnesses: Vec<Witness> = rows
        .into_iter()
        .map(
            |(id, note_id, kind, weight, source_event_id, observed_at)| Witness {
                id,
                note_id,
                kind: WitnessKind::from_str(&kind).unwrap_or(WitnessKind::WorkerProposed),
                weight,
                source_event_id,
                observed_at,
            },
        )
        .collect();

    let now_ms = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_millis() as u64;
    let c = confidence(&witnesses, now_ms);
    assert!(
        c > 0.5,
        "confidence after 5 positive witnesses should exceed 0.5, got {c}"
    );
}

#[tokio::test]
async fn closed_loop_witnesses_append_only_no_updates() {
    use event_schema::memory::{AuthorSource, NoteKind, Scope, ScopeKind, WitnessKind};
    use orchestrator::memory::{witnesses::record, PinInput, PinOutcome};

    let (kernel, _dir) = fresh_kernel().await;
    let pinned = kernel
        .pin_note(PinInput {
            kind: NoteKind::from_str("fact"),
            scope: Scope {
                kind: ScopeKind::Repo,
                value: None,
            },
            body: "fact for append-only test".to_owned(),
            source: AuthorSource::Cli,
        })
        .await
        .unwrap();
    let note_id = match pinned {
        PinOutcome::Pinned { note_id, .. } => note_id,
        other => panic!("pin should succeed, got {other:?}"),
    };

    // Record 3 distinct witnesses.
    for i in 0..3 {
        record(
            kernel.pool(),
            &note_id,
            WitnessKind::WorkerProposed,
            &format!("ev-{i}"),
        )
        .await
        .unwrap();
    }

    // Snapshot row ids + observed_at.
    let snapshot: Vec<(String, String)> = sqlx::query_as(
        "SELECT id, observed_at FROM memory_witnesses WHERE note_id = ? ORDER BY id",
    )
    .bind(&note_id)
    .fetch_all(kernel.pool())
    .await
    .unwrap();
    // Pin itself creates a UserAuthored witness, so we have 4 total (1 pin + 3 manual).
    assert!(
        snapshot.len() >= 3,
        "expected >= 3 witness rows, got {}",
        snapshot.len()
    );

    // Re-record the same sources — must be idempotent.
    for i in 0..3 {
        record(
            kernel.pool(),
            &note_id,
            WitnessKind::WorkerProposed,
            &format!("ev-{i}"),
        )
        .await
        .unwrap();
    }

    let after: Vec<(String, String)> = sqlx::query_as(
        "SELECT id, observed_at FROM memory_witnesses WHERE note_id = ? ORDER BY id",
    )
    .bind(&note_id)
    .fetch_all(kernel.pool())
    .await
    .unwrap();
    assert_eq!(
        after, snapshot,
        "witness rows mutated under duplicate record() calls"
    );
}

#[tokio::test]
async fn closed_loop_propose_ratify_promote_reuse() {
    use event_schema::memory::{BarrierKind, NoteKind, Scope, ScopeKind};
    use orchestrator::memory::{ProposalInput, ProposalOutcome, RatificationDecision, RatifyInput};

    let (kernel, _dir) = fresh_kernel().await;

    // Step 1: worker proposes a hazard (threshold=0.55 — clears with
    // WorkerProposed+UserAccepted witnesses; fact threshold=0.70 does not).
    let outcome = kernel
        .on_proposal(ProposalInput {
            mission_id: "m-loop".to_owned(),
            worker_id: "w-loop".to_owned(),
            kind: NoteKind::from_str("hazard"),
            scope: Scope {
                kind: ScopeKind::Repo,
                value: None,
            },
            body: "the codebase uses tokio runtime".to_owned(),
            derived_from: vec![],
            evidence_event_ids: vec![],
        })
        .await
        .unwrap();
    let pid = match outcome {
        ProposalOutcome::Accepted { proposal_id } => proposal_id,
        other => panic!("scanner rejected clean prose: {other:?}"),
    };

    // Step 2: supervisor ratifies.
    kernel
        .ratify(vec![RatifyInput {
            proposal_id: pid,
            decision: RatificationDecision::Accept {
                normalized_body: None,
            },
            reason: "looks right".to_owned(),
        }])
        .await
        .unwrap();

    // Step 3: mission barrier on accept.
    kernel
        .on_mission_barrier("m-loop", BarrierKind::Accept)
        .await
        .unwrap();

    // Step 4: confirm a promoted note now exists.
    let n: (i64,) = sqlx::query_as("SELECT COUNT(*) FROM memory_notes WHERE state = 'promoted'")
        .fetch_one(kernel.pool())
        .await
        .unwrap();
    assert!(
        n.0 >= 1,
        "expected >= 1 promoted note after closed loop, got {}",
        n.0
    );
}

#[tokio::test]
async fn closed_loop_reflection_demotes_below_threshold() {
    use event_schema::memory::{
        AuthorSource, BarrierKind, NoteKind, Scope, ScopeKind, WitnessKind,
    };
    use orchestrator::memory::{witnesses::record, PinInput, PinOutcome};

    let (kernel, _dir) = fresh_kernel().await;

    // Pin a hazard (lower promotion threshold than fact, so CLI pin auto-promotes).
    let pinned = kernel
        .pin_note(PinInput {
            kind: NoteKind::from_str("hazard"),
            scope: Scope {
                kind: ScopeKind::Repo,
                value: None,
            },
            body: "contested fact for demotion test".to_owned(),
            source: AuthorSource::Cli,
        })
        .await
        .unwrap();
    let note_id = match pinned {
        PinOutcome::Pinned { note_id, .. } => note_id,
        other => panic!("pin should succeed for clean body, got {other:?}"),
    };

    // Force promoted state in case the pin didn't auto-promote in this configuration.
    sqlx::query("UPDATE memory_notes SET state = 'promoted' WHERE id = ?")
        .bind(&note_id)
        .execute(kernel.pool())
        .await
        .unwrap();

    // Record explicit conflict witnesses — should drive confidence below the threshold.
    record(
        kernel.pool(),
        &note_id,
        WitnessKind::ConflictWithHigherConfidence,
        "ev-conflict-1",
    )
    .await
    .unwrap();
    record(
        kernel.pool(),
        &note_id,
        WitnessKind::ConflictWithHigherConfidence,
        "ev-conflict-2",
    )
    .await
    .unwrap();

    // Wire the note into a mission's bundle/event graph so notes_touched_by_mission picks it up.
    sqlx::query(
        "INSERT INTO memory_pending (proposal_id, mission_id, worker_id, kind, scope_kind, scope_value, body, derived_from, evidence, state, created_event_id, created_at) \
         VALUES ('p-demote', 'm-demote', 'w-1', 'hazard', 'repo', NULL, 'placeholder', '[]', '[]', 'ratified', 'ev-demote-genesis', '2026-05-18T00:00:00.000Z')"
    ).execute(kernel.pool()).await.unwrap();
    sqlx::query(
        "INSERT INTO memory_provenance (note_id, event_id, role) VALUES (?, 'ev-demote-genesis', 'created_via')"
    ).bind(&note_id).execute(kernel.pool()).await.unwrap();
    sqlx::query(
        "INSERT INTO memory_events (event_id, ts, type, mission_id, worker_id, payload_json, schema_version) \
         VALUES ('ev-demote-genesis', '2026-05-18T00:00:00.000Z', 'ratified', 'm-demote', 'w-1', '{\"proposal_id\":\"p-demote\"}', '1')"
    ).execute(kernel.pool()).await.unwrap();

    // Trigger scrub barrier — demotion fires on scrub (not accept) when confidence
    // is below the promotion threshold due to conflict witnesses.
    kernel
        .on_mission_barrier("m-demote", BarrierKind::Scrub)
        .await
        .unwrap();

    // State should now be `owned` (demoted) or `disputed`, NOT `promoted`.
    let state: (String,) = sqlx::query_as("SELECT state FROM memory_notes WHERE id = ?")
        .bind(&note_id)
        .fetch_one(kernel.pool())
        .await
        .unwrap();
    assert_ne!(
        state.0, "promoted",
        "conflict witnesses should remove promotion (got state={})",
        state.0
    );

    // F-014 verification: confirm a 'demoted' event row was written.
    let demoted_count: (i64,) =
        sqlx::query_as("SELECT COUNT(*) FROM memory_events WHERE type = 'demoted'")
            .fetch_one(kernel.pool())
            .await
            .unwrap();
    assert!(
        demoted_count.0 >= 1,
        "expected >= 1 demoted event, got {}",
        demoted_count.0
    );
}

// ---- Phase 0 (hybrid retrieval) — backfill via second kernel open ----

#[tokio::test]
async fn second_open_backfills_titles_for_pre_existing_rows() {
    use event_schema::memory::AuthorSource;
    use orchestrator::memory::hierarchy::{NoteKind, Scope, ScopeKind, StandardNoteKind};
    use orchestrator::memory::{PinInput, PinOutcome};

    // Build a kernel, seed two notes (one with an H1, one without),
    // then NULL out the title column behind the kernel's back to
    // simulate rows that existed before migration 0012 landed.
    // Reopening the kernel must repopulate the H1 row's title and
    // leave the no-H1 row untouched.
    let opts = SqliteConnectOptions::from_str("sqlite::memory:")
        .unwrap()
        .create_if_missing(true);
    // shared in-memory DB so both opens see the same data
    let pool = sqlx::sqlite::SqlitePoolOptions::new()
        .min_connections(1)
        .max_connections(1)
        .idle_timeout(None)
        .max_lifetime(None)
        .connect_with(opts)
        .await
        .unwrap();
    sqlx::migrate!("./migrations").run(&pool).await.unwrap();
    let dir = TempDir::new().unwrap();

    let kernel = MemoryKernel::open(pool.clone(), dir.path().to_path_buf())
        .await
        .unwrap();

    let PinOutcome::Pinned {
        note_id: with_h1, ..
    } = kernel
        .pin_note(PinInput {
            kind: NoteKind::Standard(StandardNoteKind::Fact),
            scope: Scope {
                kind: ScopeKind::Repo,
                value: None,
            },
            body: "# Hybrid retrieval baseline\n\nV0 baseline numbers go in design doc.".into(),
            source: AuthorSource::Cli,
        })
        .await
        .unwrap()
    else {
        panic!("expected Pinned");
    };
    let PinOutcome::Pinned { note_id: no_h1, .. } = kernel
        .pin_note(PinInput {
            kind: NoteKind::Standard(StandardNoteKind::Fact),
            scope: Scope {
                kind: ScopeKind::Repo,
                value: None,
            },
            body: "plain prose with no heading".into(),
            source: AuthorSource::Cli,
        })
        .await
        .unwrap()
    else {
        panic!("expected Pinned");
    };

    // Simulate pre-migration state: clear titles for both rows.
    sqlx::query("UPDATE memory_notes SET title = NULL")
        .execute(&pool)
        .await
        .unwrap();
    drop(kernel);

    // Reopen — backfill runs inside MemoryKernel::open.
    let kernel2 = MemoryKernel::open(pool.clone(), dir.path().to_path_buf())
        .await
        .unwrap();

    let n1 = kernel2.store.note_show(&with_h1).await.unwrap();
    let n2 = kernel2.store.note_show(&no_h1).await.unwrap();
    assert_eq!(n1.title.as_deref(), Some("Hybrid retrieval baseline"));
    assert_eq!(n2.title, None);
}
