//! DAG validation. Pure function: `validate(tasks)` returns either
//! a [`Dag`] (indegree map + topo order) or a [`GraphError`].
//!
//! Topological order uses Kahn's algorithm. Cycle detection falls
//! out for free — any node still un-emitted when the worklist is
//! empty is part of a cycle.

use crate::mission_event::TaskDescriptor;
use serde::{Deserialize, Serialize};
use specta::Type;
use std::collections::{BTreeMap, BTreeSet, VecDeque};

/// Validated decomposition. `topo_order` is one valid topological
/// sort; the scheduler may emit ready tasks in any order consistent
/// with the indegree map.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Dag {
    /// Indegree per task index (count of unresolved deps).
    pub indegree: BTreeMap<u32, u32>,
    /// One valid topological ordering. Tests use this for shape
    /// assertions; the scheduler does NOT consume it directly —
    /// it consults `indegree` + reverse adjacency.
    pub topo_order: Vec<u32>,
    /// Reverse adjacency: for each task index, the set of tasks
    /// that list it in their `depends_on`. Used by the scheduler
    /// to decrement indegree as predecessors finish.
    pub dependents: BTreeMap<u32, BTreeSet<u32>>,
}

/// Possible validation failures. Surfaced as
/// `MissionEventKind::DecompositionRejected { reason }` by the
/// supervisor loop.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Type)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum GraphError {
    /// The supervisor produced zero tasks. Distinct from "validation
    /// not reached" — empty decompositions cannot make progress.
    EmptyDecomposition,
    /// Two or more tasks share the same `index`. Indices must be
    /// unique across a decomposition.
    DuplicateIndex { index: u32 },
    /// A task lists an index in `depends_on` that no other task
    /// publishes. Either a typo or a stale reference; reject.
    OrphanDependency { from: u32, to: u32 },
    /// One or more tasks participate in a cycle. `involved` is the
    /// (sorted, deduplicated) set of indices that could not be
    /// topologically sorted.
    Cycle { involved: Vec<u32> },
}

/// Validate a decomposition. Returns a [`Dag`] on success; a
/// [`GraphError`] on the first detected failure.
///
/// Failure-priority order (matters because tests rely on it):
///   1. Empty decomposition.
///   2. Duplicate indices.
///   3. Self-loop or orphan reference.
///   4. Cycle.
pub fn validate(tasks: &[TaskDescriptor]) -> Result<Dag, GraphError> {
    if tasks.is_empty() {
        return Err(GraphError::EmptyDecomposition);
    }

    // (1) Duplicate index check.
    let mut seen_indices: BTreeSet<u32> = BTreeSet::new();
    for t in tasks {
        if !seen_indices.insert(t.index) {
            return Err(GraphError::DuplicateIndex { index: t.index });
        }
    }

    // (2) Orphan + self-loop check.
    for t in tasks {
        for &dep in &t.depends_on {
            if dep == t.index {
                return Err(GraphError::Cycle {
                    involved: vec![t.index],
                });
            }
            if !seen_indices.contains(&dep) {
                return Err(GraphError::OrphanDependency {
                    from: t.index,
                    to: dep,
                });
            }
        }
    }

    // (3) Build indegree + reverse adjacency.
    let mut indegree: BTreeMap<u32, u32> = BTreeMap::new();
    let mut dependents: BTreeMap<u32, BTreeSet<u32>> = BTreeMap::new();
    for t in tasks {
        indegree.entry(t.index).or_insert(0);
        dependents.entry(t.index).or_default();
    }
    for t in tasks {
        for &dep in &t.depends_on {
            *indegree.entry(t.index).or_insert(0) += 1;
            dependents.entry(dep).or_default().insert(t.index);
        }
    }

    // (4) Kahn's algorithm.
    let mut queue: VecDeque<u32> = indegree
        .iter()
        .filter(|(_, &d)| d == 0)
        .map(|(&i, _)| i)
        .collect();
    let mut topo_order: Vec<u32> = Vec::with_capacity(tasks.len());
    let mut working_indegree = indegree.clone();
    while let Some(idx) = queue.pop_front() {
        topo_order.push(idx);
        if let Some(succs) = dependents.get(&idx) {
            for &s in succs {
                let d = working_indegree.entry(s).or_insert(0);
                *d = d.saturating_sub(1);
                if *d == 0 {
                    queue.push_back(s);
                }
            }
        }
    }

    if topo_order.len() != tasks.len() {
        let involved: Vec<u32> = working_indegree
            .iter()
            .filter(|(_, &d)| d > 0)
            .map(|(&i, _)| i)
            .collect();
        return Err(GraphError::Cycle { involved });
    }

    Ok(Dag {
        indegree,
        topo_order,
        dependents,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::mission_event::TaskDescriptor;

    fn t(index: u32, title: &str, deps: Vec<u32>) -> TaskDescriptor {
        TaskDescriptor {
            index,
            title: title.into(),
            depends_on: deps,
            ..Default::default()
        }
    }

    #[test]
    fn empty_decomposition_rejected() {
        assert!(matches!(validate(&[]), Err(GraphError::EmptyDecomposition)));
    }

    #[test]
    fn single_task_validates() {
        let tasks = vec![t(0, "only", vec![])];
        let dag = validate(&tasks).expect("should validate");
        assert_eq!(dag.topo_order, vec![0]);
        assert_eq!(dag.indegree.get(&0), Some(&0));
    }

    #[test]
    fn linear_chain_validates() {
        let tasks = vec![
            t(0, "a", vec![]),
            t(1, "b", vec![0]),
            t(2, "c", vec![1]),
            t(3, "d", vec![2]),
        ];
        let dag = validate(&tasks).expect("should validate");
        assert_eq!(dag.topo_order, vec![0, 1, 2, 3]);
    }

    #[test]
    fn diamond_validates() {
        let tasks = vec![
            t(0, "root", vec![]),
            t(1, "left", vec![0]),
            t(2, "right", vec![0]),
            t(3, "join", vec![1, 2]),
        ];
        let dag = validate(&tasks).expect("should validate");
        let pos = |idx: u32| dag.topo_order.iter().position(|&x| x == idx).unwrap();
        assert!(pos(0) < pos(1));
        assert!(pos(0) < pos(2));
        assert!(pos(1) < pos(3));
        assert!(pos(2) < pos(3));
    }

    #[test]
    fn fan_out_validates() {
        let tasks = vec![
            t(0, "root", vec![]),
            t(1, "leaf1", vec![0]),
            t(2, "leaf2", vec![0]),
            t(3, "leaf3", vec![0]),
        ];
        let dag = validate(&tasks).expect("should validate");
        assert_eq!(dag.topo_order[0], 0);
    }

    #[test]
    fn fan_in_validates() {
        let tasks = vec![
            t(0, "src1", vec![]),
            t(1, "src2", vec![]),
            t(2, "src3", vec![]),
            t(3, "join", vec![0, 1, 2]),
        ];
        let dag = validate(&tasks).expect("should validate");
        assert_eq!(dag.topo_order.last().copied(), Some(3));
    }

    #[test]
    fn cycle_two_node_rejected() {
        let tasks = vec![t(0, "a", vec![1]), t(1, "b", vec![0])];
        match validate(&tasks) {
            Err(GraphError::Cycle { involved }) => {
                assert!(involved.contains(&0));
                assert!(involved.contains(&1));
            }
            other => panic!("expected Cycle, got {other:?}"),
        }
    }

    #[test]
    fn cycle_three_node_rejected() {
        let tasks = vec![t(0, "a", vec![2]), t(1, "b", vec![0]), t(2, "c", vec![1])];
        assert!(matches!(validate(&tasks), Err(GraphError::Cycle { .. })));
    }

    #[test]
    fn self_loop_rejected() {
        let tasks = vec![t(0, "a", vec![0])];
        assert!(matches!(validate(&tasks), Err(GraphError::Cycle { .. })));
    }

    #[test]
    fn orphan_dep_rejected() {
        let tasks = vec![t(0, "a", vec![42])];
        match validate(&tasks) {
            Err(GraphError::OrphanDependency { from, to }) => {
                assert_eq!(from, 0);
                assert_eq!(to, 42);
            }
            other => panic!("expected OrphanDependency, got {other:?}"),
        }
    }

    #[test]
    fn duplicate_index_rejected() {
        let tasks = vec![t(0, "a", vec![]), t(0, "b", vec![])];
        match validate(&tasks) {
            Err(GraphError::DuplicateIndex { index }) => {
                assert_eq!(index, 0);
            }
            other => panic!("expected DuplicateIndex, got {other:?}"),
        }
    }
}

#[cfg(test)]
mod proptests {
    use super::*;
    use crate::mission_event::TaskDescriptor;
    use proptest::prelude::*;

    /// Build a strictly acyclic decomposition: task i may only depend
    /// on indices in 0..i. By construction this can never cycle.
    fn any_acyclic_dag() -> impl Strategy<Value = Vec<TaskDescriptor>> {
        (2usize..=8usize).prop_flat_map(|n| {
            let per_task: Vec<_> = (0..n)
                .map(|i| prop::collection::vec(0u32..(i as u32).max(1), 0..=i))
                .collect();
            per_task.prop_map(move |deps_per_task| {
                deps_per_task
                    .into_iter()
                    .enumerate()
                    .map(|(i, mut deps)| {
                        deps.retain(|&d| d < i as u32);
                        deps.sort();
                        deps.dedup();
                        TaskDescriptor {
                            index: i as u32,
                            title: format!("task-{i}"),
                            depends_on: deps,
                            ..Default::default()
                        }
                    })
                    .collect()
            })
        })
    }

    proptest! {
        #[test]
        fn any_acyclic_decomposition_validates(tasks in any_acyclic_dag()) {
            let dag = validate(&tasks).expect("acyclic by construction");
            let pos: std::collections::BTreeMap<u32, usize> = dag
                .topo_order
                .iter()
                .enumerate()
                .map(|(p, &i)| (i, p))
                .collect();
            for t in &tasks {
                for &dep in &t.depends_on {
                    prop_assert!(pos[&dep] < pos[&t.index]);
                }
            }
        }

        #[test]
        fn closing_a_cycle_always_rejects(seed in 0u32..32) {
            let tasks = vec![
                TaskDescriptor {
                    index: 0,
                    title: "a".into(),
                    depends_on: vec![3],
                    ..Default::default()
                },
                TaskDescriptor {
                    index: 1,
                    title: "b".into(),
                    depends_on: vec![0],
                    ..Default::default()
                },
                TaskDescriptor {
                    index: 2,
                    title: "c".into(),
                    depends_on: vec![1],
                    ..Default::default()
                },
                TaskDescriptor {
                    index: 3,
                    title: "d".into(),
                    depends_on: vec![2],
                    ..Default::default()
                },
            ];
            let _ = seed;
            let err = validate(&tasks).expect_err("cycle must reject");
            let is_cycle = matches!(err, GraphError::Cycle { .. });
            prop_assert!(is_cycle);
        }
    }
}
