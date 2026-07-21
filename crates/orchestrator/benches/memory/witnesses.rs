//! Bench: witness insert into a clean in-memory SQLite kernel.
use criterion::{black_box, criterion_group, criterion_main, Criterion};
use event_schema::memory::WitnessKind;
use orchestrator::memory::witnesses::record;
use sqlx::sqlite::SqliteConnectOptions;
use sqlx::SqlitePool;
use std::str::FromStr;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;

async fn fresh_pool() -> SqlitePool {
    let opts = SqliteConnectOptions::from_str("sqlite::memory:")
        .unwrap()
        .create_if_missing(true);
    let pool = SqlitePool::connect_with(opts).await.unwrap();
    sqlx::migrate!("./migrations").run(&pool).await.unwrap();
    // Seed a parent note so witnesses can FK to something real.
    sqlx::query(
        "INSERT INTO memory_notes (id, kind, scope_kind, scope_value, body_path, body_hash, state, created_event_id, created_at) \
         VALUES ('note-bench', 'fact', 'repo', NULL, '/tmp/x', 'h', 'pending', 'ev-genesis', '2026-05-18T00:00:00.000Z')",
    ).execute(&pool).await.unwrap();
    pool
}

fn bench_witness_record(c: &mut Criterion) {
    let rt = tokio::runtime::Runtime::new().unwrap();
    let pool = rt.block_on(fresh_pool());
    let counter = Arc::new(AtomicU64::new(0));
    c.bench_function("witness_record_one", |b| {
        b.iter(|| {
            let c = counter.fetch_add(1, Ordering::SeqCst);
            let pool = pool.clone();
            let src = format!("ev-{c}");
            rt.block_on(async {
                let _ = record(
                    &pool,
                    black_box("note-bench"),
                    black_box(WitnessKind::WorkerProposed),
                    black_box(&src),
                )
                .await;
            })
        });
    });
}

criterion_group!(benches, bench_witness_record);
criterion_main!(benches);
