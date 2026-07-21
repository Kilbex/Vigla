//! Topological scheduler over a validated [`super::validate::Dag`].
//!
//! The scheduler is single-threaded state: parallel dispatch is
//! the caller's job (mission_loop owns a `JoinSet`). The scheduler
//! says *which* tasks are ready and tracks *which* are
//! running/done; the caller spawns futures up to the policy cap.

use super::validate::Dag;
use serde::{Deserialize, Serialize};
use specta::Type;
use std::collections::BTreeMap;

/// Per-task lifecycle state inside the scheduler. Distinct from
/// `WorkerTaskState` (which tracks an individual worker's pass
/// shape; spawned/working/submitting/etc.). A task is `Done` when
/// the arbiter renders any terminal decision (Accept / Scrub /
/// Escalate), not just on Accept.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Type)]
#[serde(rename_all = "snake_case")]
pub enum TaskState {
    Pending,
    Running,
    Done,
}

#[derive(Debug, Clone)]
pub struct Scheduler {
    dag: Dag,
    states: BTreeMap<u32, TaskState>,
    live_indegree: BTreeMap<u32, u32>,
}

impl Scheduler {
    pub fn new(dag: Dag) -> Self {
        let states: BTreeMap<u32, TaskState> = dag
            .indegree
            .keys()
            .map(|&i| (i, TaskState::Pending))
            .collect();
        let live_indegree = dag.indegree.clone();
        Self {
            dag,
            states,
            live_indegree,
        }
    }

    pub fn ready(&self) -> Vec<u32> {
        self.states
            .iter()
            .filter(|(idx, &state)| {
                state == TaskState::Pending
                    && self.live_indegree.get(idx).copied().unwrap_or(0) == 0
            })
            .map(|(&i, _)| i)
            .collect()
    }

    pub fn mark_running(&mut self, index: u32) {
        if let Some(state) = self.states.get_mut(&index) {
            *state = TaskState::Running;
        }
    }

    pub fn mark_done(&mut self, index: u32) {
        if let Some(state) = self.states.get_mut(&index) {
            *state = TaskState::Done;
        }
        if let Some(deps) = self.dag.dependents.get(&index).cloned() {
            for dep in deps {
                if let Some(d) = self.live_indegree.get_mut(&dep) {
                    *d = d.saturating_sub(1);
                }
            }
        }
    }

    pub fn is_done(&self) -> bool {
        self.states.values().all(|&s| s == TaskState::Done)
    }

    pub fn pending_count(&self) -> usize {
        self.states
            .values()
            .filter(|&&s| s == TaskState::Pending)
            .count()
    }

    pub fn running_count(&self) -> usize {
        self.states
            .values()
            .filter(|&&s| s == TaskState::Running)
            .count()
    }

    pub fn dag(&self) -> &Dag {
        &self.dag
    }

    pub fn states(&self) -> &BTreeMap<u32, TaskState> {
        &self.states
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::mission_event::TaskDescriptor;
    use crate::task_graph::validate;

    fn t(index: u32, deps: Vec<u32>) -> TaskDescriptor {
        TaskDescriptor {
            index,
            title: format!("t{index}"),
            depends_on: deps,
            ..Default::default()
        }
    }

    #[test]
    fn fresh_scheduler_emits_only_dep_free_tasks() {
        let tasks = vec![t(0, vec![]), t(1, vec![0]), t(2, vec![0])];
        let dag = validate(&tasks).unwrap();
        let sched = Scheduler::new(dag);
        let ready = sched.ready();
        assert_eq!(ready, vec![0]);
    }

    #[test]
    fn marking_running_removes_from_ready() {
        let tasks = vec![t(0, vec![]), t(1, vec![0])];
        let dag = validate(&tasks).unwrap();
        let mut sched = Scheduler::new(dag);
        assert_eq!(sched.ready(), vec![0]);
        sched.mark_running(0);
        assert!(sched.ready().is_empty());
        assert!(!sched.is_done());
    }

    #[test]
    fn marking_done_unlocks_dependents() {
        let tasks = vec![t(0, vec![]), t(1, vec![0]), t(2, vec![0]), t(3, vec![1, 2])];
        let dag = validate(&tasks).unwrap();
        let mut sched = Scheduler::new(dag);
        sched.mark_running(0);
        sched.mark_done(0);
        let mut ready = sched.ready();
        ready.sort();
        assert_eq!(ready, vec![1, 2]);

        sched.mark_running(1);
        sched.mark_done(1);
        assert!(sched.ready().is_empty() || sched.ready() == vec![2]);

        sched.mark_running(2);
        sched.mark_done(2);
        assert_eq!(sched.ready(), vec![3]);
    }

    #[test]
    fn linear_chain_full_walk() {
        let tasks: Vec<_> = (0u32..5)
            .map(|i| t(i, if i == 0 { vec![] } else { vec![i - 1] }))
            .collect();
        let dag = validate(&tasks).unwrap();
        let mut sched = Scheduler::new(dag);
        let mut order: Vec<u32> = Vec::new();
        while !sched.is_done() {
            let ready = sched.ready();
            assert_eq!(ready.len(), 1, "linear chain emits one at a time");
            let next = ready[0];
            sched.mark_running(next);
            sched.mark_done(next);
            order.push(next);
        }
        assert_eq!(order, vec![0, 1, 2, 3, 4]);
    }

    #[test]
    fn diamond_full_walk_emits_join_last() {
        let tasks = vec![t(0, vec![]), t(1, vec![0]), t(2, vec![0]), t(3, vec![1, 2])];
        let dag = validate(&tasks).unwrap();
        let mut sched = Scheduler::new(dag);
        let mut completion_order: Vec<u32> = Vec::new();
        while !sched.is_done() {
            let ready = sched.ready();
            for idx in ready {
                sched.mark_running(idx);
                sched.mark_done(idx);
                completion_order.push(idx);
            }
        }
        assert_eq!(completion_order[0], 0);
        assert_eq!(completion_order[3], 3);
        let middle: std::collections::BTreeSet<u32> =
            completion_order[1..3].iter().copied().collect();
        assert_eq!(middle, [1u32, 2].iter().copied().collect());
    }

    #[test]
    fn pending_count_decreases_monotonically() {
        let tasks = vec![t(0, vec![]), t(1, vec![0]), t(2, vec![1])];
        let dag = validate(&tasks).unwrap();
        let mut sched = Scheduler::new(dag);
        assert_eq!(sched.pending_count(), 3);
        sched.mark_running(0);
        assert_eq!(sched.pending_count(), 2);
        sched.mark_done(0);
        assert_eq!(sched.pending_count(), 2);
        sched.mark_running(1);
        sched.mark_done(1);
        assert_eq!(sched.pending_count(), 1);
        sched.mark_running(2);
        sched.mark_done(2);
        assert_eq!(sched.pending_count(), 0);
        assert!(sched.is_done());
    }

    #[test]
    fn task_state_round_trips() {
        let states = [TaskState::Pending, TaskState::Running, TaskState::Done];
        for s in states {
            let json = serde_json::to_string(&s).unwrap();
            let back: TaskState = serde_json::from_str(&json).unwrap();
            assert_eq!(s, back);
        }
    }
}
