//! Criterion bench for audit p50. Measures Smoke / Standard / Deep
//! tiers against a small Rust fixture. The human reads the numbers
//! off Criterion's report and copies them into the gate REPORT.md.

use criterion::{criterion_group, criterion_main, Criterion};
use orchestrator::audit::{audit_submission, AuditInput, AuditTier};
use std::path::PathBuf;
use tempfile::tempdir;

fn fixture(root: &std::path::Path) {
    std::fs::write(
        root.join("Cargo.toml"),
        r#"[package]
name = "audit_bench_fixture"
version = "0.0.1"
edition = "2021"
"#,
    )
    .unwrap();
    std::fs::create_dir_all(root.join("src")).unwrap();
    std::fs::write(
        root.join("src/lib.rs"),
        "pub fn id(x: i32) -> i32 {\n    x\n}\n",
    )
    .unwrap();
}

fn bench_audit(c: &mut Criterion) {
    let rt = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .unwrap();

    let mut g = c.benchmark_group("audit_p50");
    for tier in [AuditTier::Smoke, AuditTier::Standard, AuditTier::Deep] {
        let label = format!("{tier:?}");
        g.bench_function(&label, |b| {
            b.iter(|| {
                rt.block_on(async {
                    let dir = tempdir().unwrap();
                    fixture(dir.path());
                    let input = AuditInput {
                        worktree_root: dir.path().to_path_buf(),
                        test_command: None,
                        touched_files: vec!["src/lib.rs".into()],
                        scope_paths: vec![PathBuf::from("src")],
                        tier,
                        baseline: None,
                        newly_passing: vec![],
                        newly_failing: vec![],
                    };
                    audit_submission(&input).await.unwrap()
                })
            });
        });
    }
    g.finish();
}

criterion_group!(audit_benches, bench_audit);
criterion_main!(audit_benches);
