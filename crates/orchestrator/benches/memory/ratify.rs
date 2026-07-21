//! Bench: supervisor ratify() on a batch of 10 proposals.
//! Per-iteration setup is heavy (seed 10 pending proposals); the
//! measured region is just the ratify call itself.
use criterion::{black_box, criterion_group, criterion_main, BatchSize, Criterion};
use orchestrator::memory::{MemoryKernel, ProposalInput, RatificationDecision, RatifyInput};
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

async fn seed_ten_pending(kernel: &MemoryKernel) -> Vec<RatifyInput> {
    let mut inputs = Vec::with_capacity(10);
    for i in 0..10 {
        let outcome = kernel
            .on_proposal(ProposalInput {
                mission_id: format!("mission-bench-{i}"),
                worker_id: format!("worker-bench-{i}"),
                kind: event_schema::memory::NoteKind::Standard(
                    event_schema::memory::StandardNoteKind::Fact,
                ),
                scope: event_schema::memory::Scope {
                    kind: event_schema::memory::ScopeKind::Repo,
                    value: None,
                },
                body: format!("benchmark proposal #{i}, plain prose that scanner accepts"),
                derived_from: vec![],
                evidence_event_ids: vec![],
            })
            .await
            .unwrap();
        // ProposalOutcome is an enum: extract proposal_id from Accepted variant.
        use orchestrator::memory::ProposalOutcome;
        if let ProposalOutcome::Accepted { proposal_id } = outcome {
            inputs.push(RatifyInput {
                proposal_id,
                decision: RatificationDecision::Accept {
                    normalized_body: None,
                },
                reason: "bench accept".to_owned(),
            });
        }
    }
    inputs
}

fn bench_ratify_batch_of_10(c: &mut Criterion) {
    let rt = tokio::runtime::Runtime::new().unwrap();
    c.bench_function("ratify_batch_of_10", |b| {
        b.iter_batched(
            || {
                let (kernel, dir) = rt.block_on(fresh_kernel());
                let inputs = rt.block_on(seed_ten_pending(&kernel));
                (kernel, inputs, dir)
            },
            |(kernel, inputs, _dir)| {
                rt.block_on(async {
                    let _ = kernel.ratify(black_box(inputs)).await;
                })
            },
            BatchSize::LargeInput,
        );
    });
}

criterion_group!(benches, bench_ratify_batch_of_10);
criterion_main!(benches);
