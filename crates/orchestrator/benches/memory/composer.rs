//! Bench: composer.compose_manual() over 30 candidate notes.
use criterion::{black_box, criterion_group, criterion_main, Criterion};
use event_schema::memory::{AuthorSource, NoteKind, Scope, ScopeKind, StandardNoteKind};
use event_schema::Vendor;
use orchestrator::memory::{
    BundleBrief, ClaudeMemoryAdapter, Composer, MemoryStore, NewNote, NoteAuthor,
};
use sqlx::sqlite::SqliteConnectOptions;
use sqlx::SqlitePool;
use std::str::FromStr;
use tempfile::TempDir;

async fn fresh_setup() -> (
    Composer,
    ClaudeMemoryAdapter,
    BundleBrief,
    Vec<String>,
    TempDir,
) {
    let opts = SqliteConnectOptions::from_str("sqlite::memory:")
        .unwrap()
        .create_if_missing(true);
    let pool = SqlitePool::connect_with(opts).await.unwrap();
    sqlx::migrate!("./migrations").run(&pool).await.unwrap();
    let dir = TempDir::new().unwrap();
    let store = MemoryStore::open(pool.clone(), dir.path().to_path_buf())
        .await
        .unwrap();

    let mut ids = Vec::with_capacity(30);
    for i in 0..30 {
        let note_id = store
            .note_add(
                NewNote {
                    kind: NoteKind::Standard(StandardNoteKind::Fact),
                    scope: Scope {
                        kind: ScopeKind::Repo,
                        value: None,
                    },
                    body: format!("Note #{i}: a one-line fact for the composer bench."),
                },
                NoteAuthor::User {
                    source: AuthorSource::Cli,
                },
            )
            .await
            .unwrap();
        ids.push(note_id);
    }

    let composer = Composer::new(pool, store, dir.path().join("missions"));
    let brief = BundleBrief {
        mission_id: "mission-bench".to_owned(),
        worker_id: "worker-bench".to_owned(),
        turn: 0,
        vendor: Vendor::Claude,
    };
    let adapter = ClaudeMemoryAdapter;
    (composer, adapter, brief, ids, dir)
}

fn bench_compose_30_notes(c: &mut Criterion) {
    let rt = tokio::runtime::Runtime::new().unwrap();
    let (composer, adapter, brief, ids, _dir) = rt.block_on(fresh_setup());
    c.bench_function("compose_manual_30_notes", |b| {
        b.iter(|| {
            rt.block_on(async {
                let _ = composer
                    .compose_manual(black_box(&brief), black_box(&adapter), black_box(&ids))
                    .await;
            })
        });
    });
}

criterion_group!(benches, bench_compose_30_notes);
criterion_main!(benches);
