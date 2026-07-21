//! Action-replay determinism: driving two fresh kernels through the
//! same sequence of public API calls must produce LOGICALLY-identical
//! state — same notes (by kind/scope/body/state), even though
//! identity fields (UUIDv7 ids, body_hash, timestamps) diverge by
//! design.
//!
//! Satisfies the spec's "replay invariant" (V3 §1.4) at the
//! action-replay / logical-equivalence level. True event-log replay
//! (state reconstruction from `events` rows alone) would require
//! handler infrastructure that does not exist as of 2026-05-18; that
//! gap is filed against the Phase 1 audit.

use blake3::Hasher;
use event_schema::memory::{AuthorSource, BarrierKind, NoteKind, Scope, ScopeKind};
use orchestrator::memory::{
    ListFilter, MemoryKernel, PinInput, PinOutcome, ProposalInput, ProposalOutcome,
    RatificationDecision, RatifyInput,
};
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

/// LOGICAL state digest: hashes only kind/scope/body/state. Excludes
/// id, body_hash, created_at, last_verified_at, created_event_id —
/// all of which are time-varying or identity-bearing and would
/// differ between two kernels driven through equivalent actions.
async fn state_digest(kernel: &MemoryKernel) -> String {
    let summaries = kernel
        .store
        .note_list(ListFilter::default())
        .await
        .expect("note_list");
    let mut fulls = Vec::with_capacity(summaries.len());
    for s in &summaries {
        let f = kernel.store.note_show(&s.id).await.expect("note_show");
        fulls.push(f);
    }
    // Sort by logical key. Same logical state ⇒ same sort order across kernels.
    fulls.sort_by(|a, b| {
        let ka = (
            a.kind.as_str().to_owned(),
            format!("{:?}", a.scope.kind),
            a.scope.value.clone().unwrap_or_default(),
            a.body.clone(),
        );
        let kb = (
            b.kind.as_str().to_owned(),
            format!("{:?}", b.scope.kind),
            b.scope.value.clone().unwrap_or_default(),
            b.body.clone(),
        );
        ka.cmp(&kb)
    });
    let mut hasher = Hasher::new();
    for full in &fulls {
        hasher.update(full.kind.as_str().as_bytes());
        hasher.update(b"|");
        hasher.update(format!("{:?}", full.scope.kind).as_bytes());
        hasher.update(b"|");
        hasher.update(full.scope.value.as_deref().unwrap_or("").as_bytes());
        hasher.update(b"|");
        hasher.update(format!("{:?}", full.state).as_bytes());
        hasher.update(b"|");
        hasher.update(full.body.as_bytes());
        hasher.update(b"\n");
    }
    hasher.finalize().to_hex().to_string()
}

/// Pull the `proposal_id` out of an outcome regardless of branch.
/// Returns `None` when the scanner rejected.
fn pid_of(outcome: &ProposalOutcome) -> Option<&str> {
    match outcome {
        ProposalOutcome::Accepted { proposal_id } => Some(proposal_id),
        ProposalOutcome::Rejected { .. } => None,
    }
}

/// Reusable script: a sequence of public-API calls that exercises
/// propose/ratify/pin/barrier. Returns nothing — observable state is
/// captured via `state_digest` after the script runs.
async fn run_scripted_sequence(kernel: &MemoryKernel) {
    // 1. Worker proposes two notes.
    let p1 = kernel
        .on_proposal(ProposalInput {
            mission_id: "m-1".to_owned(),
            worker_id: "w-1".to_owned(),
            kind: NoteKind::from_str("fact"),
            scope: Scope {
                kind: ScopeKind::Repo,
                value: None,
            },
            body: "plain fact one".to_owned(),
            derived_from: vec![],
            evidence_event_ids: vec![],
        })
        .await
        .unwrap();
    let p2 = kernel
        .on_proposal(ProposalInput {
            mission_id: "m-1".to_owned(),
            worker_id: "w-1".to_owned(),
            kind: NoteKind::from_str("decision"),
            scope: Scope {
                kind: ScopeKind::Repo,
                value: None,
            },
            body: "plain decision two".to_owned(),
            derived_from: vec![],
            evidence_event_ids: vec![],
        })
        .await
        .unwrap();

    // 2. Supervisor ratifies both: accept the fact, reject the decision.
    let pid1 = pid_of(&p1)
        .expect("scanner should accept plain prose")
        .to_owned();
    let pid2 = pid_of(&p2)
        .expect("scanner should accept plain prose")
        .to_owned();
    let inputs = vec![
        RatifyInput {
            proposal_id: pid1,
            decision: RatificationDecision::Accept {
                normalized_body: None,
            },
            reason: "ok".to_owned(),
        },
        RatifyInput {
            proposal_id: pid2,
            decision: RatificationDecision::Reject {
                reason: "low value".to_owned(),
            },
            reason: "low value".to_owned(),
        },
    ];
    kernel.ratify(inputs).await.unwrap();

    // 3. CLI pins a third note directly.
    let pin_outcome = kernel
        .pin_note(PinInput {
            kind: NoteKind::from_str("procedure"),
            scope: Scope {
                kind: ScopeKind::Repo,
                value: None,
            },
            body: "pinned procedure three".to_owned(),
            source: AuthorSource::Cli,
        })
        .await
        .unwrap();
    assert!(
        matches!(pin_outcome, PinOutcome::Pinned { .. }),
        "pin failed: {pin_outcome:?}"
    );

    // 4. Mission barrier on accept → reflection promotes/demotes.
    kernel
        .on_mission_barrier("m-1", BarrierKind::Accept)
        .await
        .unwrap();
}

#[tokio::test]
async fn same_action_sequence_yields_logically_equivalent_state() {
    let (kernel_a, _dir_a) = fresh_kernel().await;
    let (kernel_b, _dir_b) = fresh_kernel().await;
    run_scripted_sequence(&kernel_a).await;
    run_scripted_sequence(&kernel_b).await;
    let da = state_digest(&kernel_a).await;
    let db = state_digest(&kernel_b).await;
    assert_eq!(
        da, db,
        "two kernels driven through the same script disagree (logically)"
    );
}

#[tokio::test]
async fn state_digest_is_stable_across_reads() {
    let (kernel, _dir) = fresh_kernel().await;
    run_scripted_sequence(&kernel).await;
    let d1 = state_digest(&kernel).await;
    let d2 = state_digest(&kernel).await;
    assert_eq!(d1, d2, "digest non-deterministic across read-only calls");
}
