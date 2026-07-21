//! Criterion bench for arbiter::decide(). The function is pure and
//! does no IO, so the budget (10ms p99) is generous. The bench
//! exists to catch regressions if a future check introduces an
//! allocation hotspot or unnecessary serialisation in the hot
//! path.

use criterion::{criterion_group, criterion_main, Criterion};
use orchestrator::arbiter::{decide, ArbiterPolicy, DecisionContext, ReworkKind};
use orchestrator::audit::{AuditReport, ScopeScore, SecurityFlag, SecurityFlagKind};
use orchestrator::mission_event::TaskDescriptor;
use std::path::PathBuf;

fn ctx_default() -> DecisionContext {
    DecisionContext {
        attempts_used_for_task: 0,
        attempts_used_for_mission: 0,
        submission_summary: String::new(),
        touched_files: vec![],
        scope_paths: vec![],
        preferred_rework_kind: None,
    }
}

fn bench_decide(c: &mut Criterion) {
    let mut g = c.benchmark_group("arbiter_decide");

    // Happy path: passing score, in scope, no risk flags.
    g.bench_function("accept", |b| {
        let report = AuditReport {
            overall: 0.85,
            scope: Some(ScopeScore {
                in_scope: 3,
                out_of_scope: 0,
                score: 1.0,
            }),
            ..AuditReport::default()
        };
        let ctx = DecisionContext {
            attempts_used_for_task: 0,
            attempts_used_for_mission: 0,
            submission_summary: "fixed bug".into(),
            touched_files: vec!["src/a.rs".into(), "src/b.rs".into(), "src/c.rs".into()],
            scope_paths: vec![PathBuf::from("src")],
            preferred_rework_kind: None,
        };
        let policy = ArbiterPolicy::default();
        b.iter(|| decide(&report, &ctx, &policy));
    });

    // Risk-flag path.
    g.bench_function("escalate_risk", |b| {
        let report = AuditReport {
            overall: 0.85,
            scope: Some(ScopeScore {
                in_scope: 1,
                out_of_scope: 0,
                score: 1.0,
            }),
            security_flags: vec![SecurityFlag {
                kind: SecurityFlagKind::SecretFile,
                path: ".env".into(),
                detail: "secret".into(),
            }],
            ..AuditReport::default()
        };
        let ctx = ctx_default();
        let policy = ArbiterPolicy::default();
        b.iter(|| decide(&report, &ctx, &policy));
    });

    g.finish();
}

fn bench_decide_per_rework_kind(c: &mut Criterion) {
    let mut g = c.benchmark_group("arbiter_decide_per_kind");

    let low_quality_report = AuditReport {
        overall: 0.5,
        scope: Some(ScopeScore {
            in_scope: 1,
            out_of_scope: 0,
            score: 1.0,
        }),
        ..AuditReport::default()
    };
    let policy = ArbiterPolicy::default();

    let kinds: Vec<(&str, ReworkKind)> = vec![
        (
            "extend_revise",
            ReworkKind::Revise {
                directive: "fix".into(),
            },
        ),
        (
            "extend_reassign",
            ReworkKind::Reassign {
                from_worker: "mock-1".into(),
                to_vendor: Some(event_schema::Vendor::Codex),
            },
        ),
        (
            "extend_split",
            ReworkKind::Split {
                sub_tasks: vec![
                    TaskDescriptor {
                        index: 0,
                        title: "a".into(),
                        ..Default::default()
                    },
                    TaskDescriptor {
                        index: 1,
                        title: "b".into(),
                        ..Default::default()
                    },
                ],
            },
        ),
        (
            "extend_narrow",
            ReworkKind::Narrow {
                reduced_scope: vec![PathBuf::from("src/lib.rs")],
            },
        ),
        (
            "extend_rebrief",
            ReworkKind::Rebrief {
                new_brief: "implement only the parser".into(),
            },
        ),
        (
            "extend_mark_unachievable",
            ReworkKind::MarkUnachievable {
                rationale: "manual review required".into(),
            },
        ),
    ];

    for (name, kind) in kinds {
        let mut ctx = ctx_default();
        ctx.preferred_rework_kind = Some(kind);
        let report = low_quality_report.clone();
        let pol = policy.clone();
        g.bench_function(name, move |b| {
            b.iter(|| decide(&report, &ctx, &pol));
        });
    }

    g.finish();
}

criterion_group!(arbiter_benches, bench_decide, bench_decide_per_rework_kind);
criterion_main!(arbiter_benches);
