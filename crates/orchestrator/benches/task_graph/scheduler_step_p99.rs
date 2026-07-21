//! Bench the scheduler's per-step cost: ready() over a 32-task
//! DAG with mixed fan-out. Target: well under 100us / step on
//! commodity hardware so the parallel-dispatch loop's overhead
//! is invisible compared to the worker-pass and audit budget.

use criterion::{criterion_group, criterion_main, Criterion};
use orchestrator::mission_event::TaskDescriptor;
use orchestrator::task_graph::{validate, Scheduler};

fn build_diamond_chain(width: u32, depth: u32) -> Vec<TaskDescriptor> {
    let mut tasks = Vec::new();
    let mut last_join: Option<u32> = None;
    for d in 0..depth {
        let root_idx: u32 = tasks.len() as u32;
        let root_deps = match last_join {
            Some(prev) => vec![prev],
            None => vec![],
        };
        tasks.push(TaskDescriptor {
            index: root_idx,
            title: format!("d{d}-root"),
            depends_on: root_deps,
            ..Default::default()
        });
        let mut arm_indices = Vec::new();
        for w in 0..width {
            let arm_idx = tasks.len() as u32;
            tasks.push(TaskDescriptor {
                index: arm_idx,
                title: format!("d{d}-arm{w}"),
                depends_on: vec![root_idx],
                ..Default::default()
            });
            arm_indices.push(arm_idx);
        }
        let join_idx = tasks.len() as u32;
        tasks.push(TaskDescriptor {
            index: join_idx,
            title: format!("d{d}-join"),
            depends_on: arm_indices,
            ..Default::default()
        });
        last_join = Some(join_idx);
    }
    tasks
}

fn bench_scheduler(c: &mut Criterion) {
    let tasks = build_diamond_chain(4, 6);
    let dag = validate(&tasks).expect("acyclic");

    c.bench_function("scheduler/full_walk_24tasks", |b| {
        b.iter(|| {
            let mut sched = Scheduler::new(dag.clone());
            while !sched.is_done() {
                let ready = sched.ready();
                for idx in ready {
                    sched.mark_running(idx);
                    sched.mark_done(idx);
                }
            }
        })
    });

    c.bench_function("scheduler/ready_only", |b| {
        let sched = Scheduler::new(dag.clone());
        b.iter(|| {
            let _ = sched.ready();
        })
    });
}

criterion_group!(benches, bench_scheduler);
criterion_main!(benches);
