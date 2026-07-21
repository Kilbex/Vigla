//! Bench for the classify+recover hot path. The pair is invoked on
//! every failure-path worker pass; it must stay well below 1ms p99
//! to avoid dominating the supervisor loop.

use std::path::PathBuf;
use std::time::Duration;

use criterion::{criterion_group, criterion_main, Criterion};
use event_schema::Vendor;
use orchestrator::mission_worker_dispatch::WorkerDispatchError;
use orchestrator::recovery::{
    classify::ClassifyContext, classify_failure, policy::RecoveryPolicy, recover, RecoveryHistory,
};

fn bench_recovery(c: &mut Criterion) {
    let mut g = c.benchmark_group("recovery_classify_p99");

    g.bench_function("classify_timeout_then_recover", |b| {
        let err = WorkerDispatchError::Timeout(Duration::from_secs(900));
        let ctx = ClassifyContext {
            vendor: Vendor::Claude,
            touched_files: vec![],
            declared_scope: vec![],
            quota_signals: vec![],
            context_requests: vec![],
        };
        let policy = RecoveryPolicy::default();
        b.iter(|| {
            let class = classify_failure(Some(&err), &ctx, 0, 0);
            recover(&class, &mut RecoveryHistory::new(), &policy, 0)
        });
    });

    g.bench_function("classify_missing_file_then_recover", |b| {
        let err = WorkerDispatchError::Io("ENOENT \"src/lib.rs\"".into());
        let ctx = ClassifyContext {
            vendor: Vendor::Codex,
            touched_files: vec![],
            declared_scope: vec![PathBuf::from("src")],
            quota_signals: vec![],
            context_requests: vec![],
        };
        let policy = RecoveryPolicy::default();
        b.iter(|| {
            let class = classify_failure(Some(&err), &ctx, 0, 0);
            recover(&class, &mut RecoveryHistory::new(), &policy, 0)
        });
    });

    g.bench_function("classify_quota_then_recover", |b| {
        let mut ctx = ClassifyContext {
            vendor: Vendor::Claude,
            touched_files: vec![],
            declared_scope: vec![],
            quota_signals: vec![],
            context_requests: vec![],
        };
        ctx.quota_signals
            .push(orchestrator::recovery::classify::QuotaSignal {
                vendor: Vendor::Claude,
                estimated_reset_at_ms: Some(2_000),
            });
        let policy = RecoveryPolicy::default();
        b.iter(|| {
            let class = classify_failure(None, &ctx, 5 * 3600 * 1000, 1_000);
            recover(&class, &mut RecoveryHistory::new(), &policy, 1_000)
        });
    });

    g.finish();
}

criterion_group!(recovery_benches, bench_recovery);
criterion_main!(recovery_benches);
