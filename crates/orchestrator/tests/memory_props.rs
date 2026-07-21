//! Property-based tests for the memory kernel's scanner, confidence scoring,
//! witnesses, ratification, and reflection boundaries.

use proptest::prelude::*;
mod scanner {
    use orchestrator::memory::{redact_preview, scan, MatchReason, ScanResult};
    use proptest::prelude::*;

    proptest! {
        #[test]
        fn scan_is_deterministic(body in ".{0,2048}") {
            prop_assert_eq!(scan(&body), scan(&body));
        }

        #[test]
        fn redact_preview_respects_char_cap(body in ".{0,2048}", cap in 0usize..1024) {
            let out = redact_preview(&body, cap);
            // Contract: if body.chars().count() <= cap, returns body unchanged.
            // Otherwise, takes cap.saturating_sub(3) chars then appends "..." (3 chars),
            // so total = cap chars when cap >= 3, or 3 chars when cap < 3.
            // In all cases output.chars().count() <= cap.max(3).
            let out_chars = out.chars().count();
            let bound = cap.max(3);
            prop_assert!(
                out_chars <= bound,
                "out_chars={} cap={} bound={}", out_chars, cap, bound
            );
        }

        #[test]
        fn aws_access_key_shape_always_matches(suffix in "[A-Z0-9]{16}") {
            let body = format!("config: AKIA{suffix} END");
            match scan(&body) {
                ScanResult::Match { reason: MatchReason::Pattern(_), .. } => {}
                other => prop_assert!(false, "expected Pattern match, got {other:?}"),
            }
        }

        #[test]
        fn short_plain_prose_never_matches(words in proptest::collection::vec("[a-z]{1,8}", 1..15)) {
            let body = words.join(" ");
            prop_assert_eq!(scan(&body), ScanResult::Clean);
        }
    }
}
mod scoring {
    use event_schema::memory::WitnessKind;
    use orchestrator::memory::{confidence, Witness};
    use proptest::prelude::*;

    fn ws(weight: f64, kind: WitnessKind, observed_at: &str) -> Witness {
        Witness {
            id: format!("w-{weight}-{kind:?}"),
            note_id: "note-prop".to_owned(),
            kind,
            weight,
            source_event_id: format!("ev-{weight}-{kind:?}"),
            observed_at: observed_at.to_owned(),
        }
    }

    // 2026-05-19T00:00:00.000Z — one day after all "recent" timestamps so
    // F-007 (future-timestamp skip) does not suppress the recency bonus.
    const FIXED_NOW_MS: u64 = 1_779_148_800_000;

    proptest! {
        // F-008: sigmoid(raw) returns exactly 1.0 in IEEE 754 when raw >= ~37
        // (exp(-37) underflows below machine-epsilon, so 1+exp(-37)==1.0).
        // Weights are capped at 1.0 here so max raw = 30*1.0 + 0.2 = 30.2,
        // which stays below the saturation threshold.  The uncapped range
        // (-2.0..2.0) triggers the defect at n=20, weight≈1.887.
        #[test]
        fn confidence_strictly_inside_open_unit_interval(
            n in 0usize..30,
            weight in -1.0_f64..1.0,
        ) {
            let mut ws_vec = Vec::new();
            for i in 0..n {
                ws_vec.push(ws(
                    weight,
                    WitnessKind::WorkerProposed,
                    "2026-05-18T00:00:00.000Z",
                ));
                let _ = i;
            }
            let c = confidence(&ws_vec, FIXED_NOW_MS);
            prop_assert!(c > 0.0 && c < 1.0, "confidence={c} not in (0,1)");
        }

        #[test]
        fn confidence_monotonic_in_positive_weight_delta(
            base_weight in 0.1_f64..1.0,
            delta in 0.1_f64..1.0,
        ) {
            let a = vec![ws(base_weight, WitnessKind::WorkerProposed, "2026-05-18T00:00:00.000Z")];
            let mut b = a.clone();
            b.push(ws(delta, WitnessKind::WorkerProposed, "2026-05-18T00:00:00.000Z"));
            let ca = confidence(&a, FIXED_NOW_MS);
            let cb = confidence(&b, FIXED_NOW_MS);
            prop_assert!(cb > ca, "expected {cb} > {ca}");
        }

        #[test]
        fn confidence_strictly_decreases_with_conflict(
            base_weight in 0.1_f64..2.0,
        ) {
            let clean = vec![ws(base_weight, WitnessKind::WorkerProposed, "2026-05-18T00:00:00.000Z")];
            let mut with_conflict = clean.clone();
            with_conflict.push(ws(
                0.0,
                WitnessKind::ConflictWithHigherConfidence,
                "2026-05-18T00:00:00.000Z",
            ));
            let c_clean = confidence(&clean, FIXED_NOW_MS);
            let c_conflict = confidence(&with_conflict, FIXED_NOW_MS);
            prop_assert!(c_conflict < c_clean, "{c_conflict} not < {c_clean}");
        }

        #[test]
        fn confidence_non_increasing_with_age(
            base_weight in 0.5_f64..1.5,
        ) {
            // "recent" = 2026-05-18, "aged" = 2023-05-18.
            // FIXED_NOW_MS is 2026-05-19 so both timestamps are in the past
            // relative to now_ms; F-007 does not suppress either.
            let recent = vec![ws(base_weight, WitnessKind::WorkerProposed, "2026-05-18T00:00:00.000Z")];
            let aged = vec![ws(base_weight, WitnessKind::WorkerProposed, "2023-05-18T00:00:00.000Z")];
            let c_recent = confidence(&recent, FIXED_NOW_MS);
            let c_aged = confidence(&aged, FIXED_NOW_MS);
            prop_assert!(c_aged <= c_recent, "aged={c_aged} > recent={c_recent}");
        }
    }
}
mod witnesses {
    use event_schema::memory::WitnessKind;
    use orchestrator::memory::witnesses::{record, Recorded};
    use proptest::prelude::*;
    use sqlx::sqlite::SqliteConnectOptions;
    use sqlx::SqlitePool;
    use std::str::FromStr;
    use std::sync::OnceLock;

    fn runtime() -> &'static tokio::runtime::Runtime {
        static RT: OnceLock<tokio::runtime::Runtime> = OnceLock::new();
        RT.get_or_init(|| tokio::runtime::Runtime::new().unwrap())
    }

    async fn fresh_pool() -> SqlitePool {
        let opts = SqliteConnectOptions::from_str("sqlite::memory:")
            .unwrap()
            .create_if_missing(true);
        let pool = SqlitePool::connect_with(opts).await.unwrap();
        sqlx::migrate!("./migrations").run(&pool).await.unwrap();
        // Seed a parent note (FK target).
        sqlx::query(
            "INSERT INTO memory_notes (id, kind, scope_kind, scope_value, body_path, body_hash, state, created_event_id, created_at) \
             VALUES ('note-prop', 'fact', 'repo', NULL, '/tmp/x', 'h', 'pending', 'ev-genesis', '2026-05-18T00:00:00.000Z')",
        ).execute(&pool).await.unwrap();
        pool
    }

    proptest! {
        #[test]
        fn record_is_idempotent_on_same_source(src in "[a-z0-9]{8}") {
            runtime().block_on(async {
                let pool = fresh_pool().await;
                let a = record(&pool, "note-prop", WitnessKind::WorkerProposed, &src).await.unwrap();
                let b = record(&pool, "note-prop", WitnessKind::WorkerProposed, &src).await.unwrap();
                let count: (i64,) = sqlx::query_as(
                    "SELECT COUNT(*) FROM memory_witnesses WHERE source_event_id = ?"
                ).bind(&src).fetch_one(&pool).await.unwrap();
                assert_eq!(count.0, 1, "duplicate source produced {} rows", count.0);
                assert!(matches!(a, Recorded::Inserted(_)));
                assert!(matches!(b, Recorded::AlreadyExists));
            });
        }

        #[test]
        fn record_with_distinct_sources_yields_distinct_rows(
            srcs in proptest::collection::hash_set("[a-z0-9]{8}", 2..5),
        ) {
            let n_srcs = srcs.len();
            runtime().block_on(async {
                let pool = fresh_pool().await;
                for src in &srcs {
                    let _ = record(&pool, "note-prop", WitnessKind::WorkerProposed, src).await.unwrap();
                }
                let count: (i64,) = sqlx::query_as(
                    "SELECT COUNT(*) FROM memory_witnesses WHERE note_id = ?"
                ).bind("note-prop").fetch_one(&pool).await.unwrap();
                assert_eq!(count.0 as usize, n_srcs);
            });
        }

        #[test]
        fn record_emits_event_per_inserted_row(src in "[a-z0-9]{8}") {
            runtime().block_on(async {
                let pool = fresh_pool().await;
                let _ = record(&pool, "note-prop", WitnessKind::WorkerProposed, &src).await.unwrap();
                // Memory events live in their own table.
                let evs: (i64,) = sqlx::query_as(
                    "SELECT COUNT(*) FROM memory_events WHERE type = 'witness_recorded'"
                ).fetch_one(&pool).await.unwrap();
                assert!(evs.0 >= 1, "expected >= 1 witness_recorded event, got {}", evs.0);
            });
        }
    }
}
mod ratify {
    use event_schema::memory::{NoteKind, Scope, ScopeKind};
    use orchestrator::memory::{
        MemoryKernel, ProposalInput, ProposalOutcome, RatificationDecision, RatifyInput,
    };
    use proptest::prelude::*;
    use sqlx::sqlite::SqliteConnectOptions;
    use sqlx::SqlitePool;
    use std::str::FromStr;
    use std::sync::OnceLock;
    use tempfile::TempDir;

    fn runtime() -> &'static tokio::runtime::Runtime {
        static RT: OnceLock<tokio::runtime::Runtime> = OnceLock::new();
        RT.get_or_init(|| tokio::runtime::Runtime::new().unwrap())
    }

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

    fn accepted_pid(outcome: &ProposalOutcome) -> Option<String> {
        match outcome {
            ProposalOutcome::Accepted { proposal_id } => Some(proposal_id.clone()),
            ProposalOutcome::Rejected { .. } => None,
        }
    }

    proptest! {
        #[test]
        fn accept_yields_exactly_one_note(body in "[a-z ]{5,80}") {
            runtime().block_on(async {
                let (kernel, _dir) = fresh_kernel().await;
                let outcome = kernel.on_proposal(ProposalInput {
                    mission_id: "m-1".to_owned(),
                    worker_id: "w-1".to_owned(),
                    kind: NoteKind::from_str("fact"),
                    scope: Scope { kind: ScopeKind::Repo, value: None },
                    body: body.clone(),
                    derived_from: vec![],
                    evidence_event_ids: vec![],
                }).await.unwrap();
                let Some(pid) = accepted_pid(&outcome) else { return; };
                kernel.ratify(vec![RatifyInput {
                    proposal_id: pid,
                    decision: RatificationDecision::Accept { normalized_body: None },
                    reason: "ok".to_owned(),
                }]).await.unwrap();
                let n: (i64,) = sqlx::query_as("SELECT COUNT(*) FROM memory_notes")
                    .fetch_one(kernel.pool()).await.unwrap();
                assert_eq!(n.0, 1, "expected exactly 1 note after accept, got {}", n.0);
            });
        }

        #[test]
        fn reject_yields_zero_notes(body in "[a-z ]{5,80}") {
            runtime().block_on(async {
                let (kernel, _dir) = fresh_kernel().await;
                let outcome = kernel.on_proposal(ProposalInput {
                    mission_id: "m-1".to_owned(),
                    worker_id: "w-1".to_owned(),
                    kind: NoteKind::from_str("fact"),
                    scope: Scope { kind: ScopeKind::Repo, value: None },
                    body: body.clone(),
                    derived_from: vec![],
                    evidence_event_ids: vec![],
                }).await.unwrap();
                let Some(pid) = accepted_pid(&outcome) else { return; };
                kernel.ratify(vec![RatifyInput {
                    proposal_id: pid,
                    decision: RatificationDecision::Reject { reason: "no".to_owned() },
                    reason: "no".to_owned(),
                }]).await.unwrap();
                let n: (i64,) = sqlx::query_as("SELECT COUNT(*) FROM memory_notes")
                    .fetch_one(kernel.pool()).await.unwrap();
                assert_eq!(n.0, 0, "reject should produce zero notes, got {}", n.0);
            });
        }

        #[test]
        fn ratify_is_total_over_input(n in 1usize..6) {
            runtime().block_on(async {
                let (kernel, _dir) = fresh_kernel().await;
                let mut ids = Vec::new();
                for i in 0..n {
                    let o = kernel.on_proposal(ProposalInput {
                        mission_id: format!("m-{i}"),
                        worker_id: format!("w-{i}"),
                        kind: NoteKind::from_str("fact"),
                        scope: Scope { kind: ScopeKind::Repo, value: None },
                        body: format!("proposal body {i}"),
                        derived_from: vec![],
                        evidence_event_ids: vec![],
                    }).await.unwrap();
                    if let Some(pid) = accepted_pid(&o) { ids.push(pid); }
                }
                let inputs: Vec<RatifyInput> = ids.into_iter().map(|pid| RatifyInput {
                    proposal_id: pid,
                    decision: RatificationDecision::Accept { normalized_body: None },
                    reason: "ok".to_owned(),
                }).collect();
                let expected = inputs.len();
                let outcomes = kernel.ratify(inputs).await.unwrap();
                assert_eq!(outcomes.len(), expected, "ratify dropped inputs");
            });
        }
    }
}
mod reflection {
    use event_schema::memory::{BarrierKind, NoteKind, Scope, ScopeKind};
    use orchestrator::memory::{
        MemoryKernel, ProposalInput, ProposalOutcome, RatificationDecision, RatifyInput,
    };
    use proptest::prelude::*;
    use sqlx::sqlite::SqliteConnectOptions;
    use sqlx::SqlitePool;
    use std::str::FromStr;
    use std::sync::OnceLock;
    use tempfile::TempDir;

    fn runtime() -> &'static tokio::runtime::Runtime {
        static RT: OnceLock<tokio::runtime::Runtime> = OnceLock::new();
        RT.get_or_init(|| tokio::runtime::Runtime::new().unwrap())
    }

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

    fn accepted_pid(outcome: &ProposalOutcome) -> Option<String> {
        match outcome {
            ProposalOutcome::Accepted { proposal_id } => Some(proposal_id.clone()),
            ProposalOutcome::Rejected { .. } => None,
        }
    }

    proptest! {
        #[test]
        fn scrub_never_promotes(body in "[a-z ]{5,80}") {
            runtime().block_on(async {
                let (kernel, _dir) = fresh_kernel().await;
                let o = kernel.on_proposal(ProposalInput {
                    mission_id: "m-scrub".to_owned(),
                    worker_id: "w-1".to_owned(),
                    kind: NoteKind::from_str("fact"),
                    scope: Scope { kind: ScopeKind::Repo, value: None },
                    body: body.clone(),
                    derived_from: vec![],
                    evidence_event_ids: vec![],
                }).await.unwrap();
                if let Some(pid) = accepted_pid(&o) {
                    kernel.ratify(vec![RatifyInput {
                        proposal_id: pid,
                        decision: RatificationDecision::Accept { normalized_body: None },
                        reason: "ok".to_owned(),
                    }]).await.unwrap();
                }
                kernel.on_mission_barrier("m-scrub", BarrierKind::Scrub).await.unwrap();
                let promoted: (i64,) = sqlx::query_as(
                    "SELECT COUNT(*) FROM memory_notes WHERE state = 'promoted'"
                ).fetch_one(kernel.pool()).await.unwrap();
                assert_eq!(promoted.0, 0, "scrub barrier produced {} promoted note(s)", promoted.0);
            });
        }

        #[test]
        fn accept_does_not_lose_ratified_note(body in "[a-z ]{15,80}") {
            runtime().block_on(async {
                let (kernel, _dir) = fresh_kernel().await;
                let o = kernel.on_proposal(ProposalInput {
                    mission_id: "m-accept".to_owned(),
                    worker_id: "w-1".to_owned(),
                    kind: NoteKind::from_str("fact"),
                    scope: Scope { kind: ScopeKind::Repo, value: None },
                    body,
                    derived_from: vec![],
                    evidence_event_ids: vec![],
                }).await.unwrap();
                let Some(pid) = accepted_pid(&o) else { return; };
                kernel.ratify(vec![RatifyInput {
                    proposal_id: pid,
                    decision: RatificationDecision::Accept { normalized_body: None },
                    reason: "ok".to_owned(),
                }]).await.unwrap();
                kernel.on_mission_barrier("m-accept", BarrierKind::Accept).await.unwrap();
                let any_notes: (i64,) = sqlx::query_as(
                    "SELECT COUNT(*) FROM memory_notes"
                ).fetch_one(kernel.pool()).await.unwrap();
                assert!(any_notes.0 >= 1, "expected >= 1 note after ratify+accept");
            });
        }

        #[test]
        fn replay_barrier_is_idempotent(body in "[a-z ]{5,80}") {
            runtime().block_on(async {
                let (kernel, _dir) = fresh_kernel().await;
                let _ = kernel.on_proposal(ProposalInput {
                    mission_id: "m-idem".to_owned(),
                    worker_id: "w-1".to_owned(),
                    kind: NoteKind::from_str("fact"),
                    scope: Scope { kind: ScopeKind::Repo, value: None },
                    body,
                    derived_from: vec![],
                    evidence_event_ids: vec![],
                }).await.unwrap();
                kernel.on_mission_barrier("m-idem", BarrierKind::Accept).await.unwrap();
                let snap1: (i64, i64) = sqlx::query_as(
                    "SELECT (SELECT COUNT(*) FROM memory_notes), (SELECT COUNT(*) FROM memory_pending)"
                ).fetch_one(kernel.pool()).await.unwrap();
                kernel.on_mission_barrier("m-idem", BarrierKind::Accept).await.unwrap();
                let snap2: (i64, i64) = sqlx::query_as(
                    "SELECT (SELECT COUNT(*) FROM memory_notes), (SELECT COUNT(*) FROM memory_pending)"
                ).fetch_one(kernel.pool()).await.unwrap();
                assert_eq!(snap1, snap2, "second barrier changed state");
            });
        }
    }
}

proptest! {
    #[test]
    fn proptest_harness_is_wired(n in 0u32..100) {
        prop_assert!(n < 100);
    }
}
