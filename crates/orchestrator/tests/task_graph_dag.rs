//! Integration tests for the S7 DAG validation primitive. Pure
//! library-level — runs against `task_graph::validate(&tasks)` with
//! representative decomposition shapes.
//!
//! Why a `tests/` binary instead of inline `#[cfg(test)] mod`:
//! `GraphError`'s serde representation IS the wire format for the
//! `MissionEventKind::DecompositionRejected { reason }` event the
//! supervisor loop emits when validation fails. Running through
//! the public crate API ensures the integration boundary stays
//! independent of `cfg(test)` helpers — what these tests see is
//! what the host (and the frontend) sees.

use orchestrator::mission_event::TaskDescriptor;
use orchestrator::task_graph::{validate, GraphError};

fn t(index: u32, title: &str, deps: Vec<u32>) -> TaskDescriptor {
    TaskDescriptor {
        index,
        title: title.into(),
        depends_on: deps,
        ..Default::default()
    }
}

#[test]
fn linear_4_task_chain() {
    let tasks = vec![
        t(0, "a", vec![]),
        t(1, "b", vec![0]),
        t(2, "c", vec![1]),
        t(3, "d", vec![2]),
    ];
    let dag = validate(&tasks).expect("linear chain validates");
    assert_eq!(dag.topo_order, vec![0, 1, 2, 3]);
}

#[test]
fn fan_out_one_root_three_leaves() {
    let tasks = vec![
        t(0, "root", vec![]),
        t(1, "leaf-a", vec![0]),
        t(2, "leaf-b", vec![0]),
        t(3, "leaf-c", vec![0]),
    ];
    let dag = validate(&tasks).expect("fan-out validates");
    assert_eq!(dag.topo_order[0], 0);
    let leaves: std::collections::BTreeSet<u32> = dag.topo_order[1..].iter().copied().collect();
    assert_eq!(leaves, [1u32, 2, 3].iter().copied().collect());
}

#[test]
fn fan_in_three_sources_one_join() {
    let tasks = vec![
        t(0, "src-a", vec![]),
        t(1, "src-b", vec![]),
        t(2, "src-c", vec![]),
        t(3, "join", vec![0, 1, 2]),
    ];
    let dag = validate(&tasks).expect("fan-in validates");
    assert_eq!(dag.topo_order.last().copied(), Some(3));
}

#[test]
fn diamond_root_two_arms_join() {
    let tasks = vec![
        t(0, "root", vec![]),
        t(1, "left", vec![0]),
        t(2, "right", vec![0]),
        t(3, "join", vec![1, 2]),
    ];
    let dag = validate(&tasks).expect("diamond validates");
    assert_eq!(dag.topo_order[0], 0);
    assert_eq!(dag.topo_order.last().copied(), Some(3));
}

#[test]
fn cycle_two_node_rejects() {
    let tasks = vec![t(0, "a", vec![1]), t(1, "b", vec![0])];
    let err = validate(&tasks).expect_err("cycle must reject");
    match err {
        GraphError::Cycle { involved } => {
            assert!(involved.contains(&0));
            assert!(involved.contains(&1));
        }
        other => panic!("expected Cycle, got {other:?}"),
    }
}

#[test]
fn cycle_inside_partial_dag_rejects() {
    // Two clean tasks plus a 3-node back-edge cycle. Validation must
    // still flag the cycle even when there are valid prefix tasks.
    let tasks = vec![
        t(0, "clean-a", vec![]),
        t(1, "clean-b", vec![0]),
        t(2, "loop-x", vec![4]),
        t(3, "loop-y", vec![2]),
        t(4, "loop-z", vec![3]),
    ];
    assert!(matches!(validate(&tasks), Err(GraphError::Cycle { .. })));
}

#[test]
fn orphan_dep_rejects_with_indices() {
    let tasks = vec![t(0, "a", vec![]), t(1, "b", vec![99])];
    match validate(&tasks).expect_err("orphan must reject") {
        GraphError::OrphanDependency { from, to } => {
            assert_eq!(from, 1);
            assert_eq!(to, 99);
        }
        other => panic!("expected OrphanDependency, got {other:?}"),
    }
}

#[test]
fn duplicate_index_rejects() {
    let tasks = vec![t(0, "first", vec![]), t(0, "duplicate", vec![])];
    match validate(&tasks).expect_err("duplicate must reject") {
        GraphError::DuplicateIndex { index } => assert_eq!(index, 0),
        other => panic!("expected DuplicateIndex, got {other:?}"),
    }
}

#[test]
fn empty_decomposition_rejects() {
    let tasks: Vec<TaskDescriptor> = vec![];
    assert!(matches!(
        validate(&tasks),
        Err(GraphError::EmptyDecomposition)
    ));
}

#[test]
fn graph_error_serializes_with_kind_tag() {
    // `GraphError`'s serde representation is the wire format for the
    // `MissionEventKind::DecompositionRejected { reason }` event.
    // Lock the JSON shape so a downstream consumer (the frontend
    // visibility router, the inbox card) can parse it reliably.
    let err = GraphError::Cycle {
        involved: vec![0, 1],
    };
    let json = serde_json::to_string(&err).unwrap();
    assert!(json.contains("\"kind\":\"cycle\""));
    let back: GraphError = serde_json::from_str(&json).unwrap();
    assert_eq!(err, back);
}
