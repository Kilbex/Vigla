//! Bench rebase + ff-merge against a minimal fixture. Budget: p50 <
//! 200ms (allows generous headroom over typical git overhead).

use criterion::{criterion_group, criterion_main, Criterion};
use orchestrator::mission::MissionId;
use orchestrator::mission_workspace::{MergeOutcome, MissionWorkspace};
use tempfile::TempDir;

async fn run_git(dir: &std::path::Path, args: &[&str]) {
    tokio::process::Command::new("git")
        .args(args)
        .current_dir(dir)
        .status()
        .await
        .unwrap();
}

async fn setup() -> (MissionWorkspace, TempDir) {
    let td = tempfile::tempdir().unwrap();
    let root = td.path().to_path_buf();
    run_git(&root, &["init", "-q", "-b", "main"]).await;
    run_git(&root, &["config", "user.email", "test@example.com"]).await;
    run_git(&root, &["config", "user.name", "Test"]).await;
    tokio::fs::write(root.join("base.txt"), "base")
        .await
        .unwrap();
    run_git(&root, &["add", "."]).await;
    run_git(&root, &["commit", "-q", "-m", "base"]).await;

    let mid: MissionId = "bench".into();
    let w = MissionWorkspace::new(root.clone(), mid).unwrap();
    w.create_supervisor_branch("main").await.unwrap();
    w.create_supervisor_worktree().await.unwrap();
    w.create_worker_branch("mock-1").await.unwrap();
    let worker_wt = w.create_worker_worktree("mock-1").await.unwrap();
    tokio::fs::write(worker_wt.join("worker.txt"), "y")
        .await
        .unwrap();
    run_git(&worker_wt, &["add", "."]).await;
    run_git(&worker_wt, &["commit", "-q", "-m", "worker"]).await;
    (w, td)
}

fn bench_rebase(c: &mut Criterion) {
    let rt = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap();

    let mut g = c.benchmark_group("integration_rebase");
    g.bench_function("clean_rebase_ff_merge", |b| {
        b.to_async(&rt).iter(|| async {
            // Fresh fixture per iteration so each call starts from
            // a clean state; this captures git's real start-up cost,
            // which is what we care about.
            let (w, _td) = setup().await;
            let outcome = w.integrate_worker("mock-1", 0, "bench").await.unwrap();
            match outcome {
                MergeOutcome::Success(_) => (),
                MergeOutcome::Conflict(_) => panic!("clean rebase should succeed"),
            }
        });
    });
    g.finish();
}

criterion_group!(integration_benches, bench_rebase);
criterion_main!(integration_benches);
