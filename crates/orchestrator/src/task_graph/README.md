# Task Graph

DAG-based scheduling for parallel workers. Replaces the
sequential per-task loop in `mission_supervisor_run::mission_loop`.

## Modules

- **`descriptor`** — `TaskRole`, `AcceptanceCriteria`,
  `CriteriaOutcome`, plus `effective_scope_paths` helper.
- **`validate`** — `validate(&[TaskDescriptor]) -> Result<Dag, GraphError>`.
  Kahn's algorithm; rejects empty / duplicate / orphan / cyclic
  decompositions.
- **`scheduler`** — `Scheduler::new(Dag)` + `ready()` /
  `mark_running` / `mark_done` state machine.
- **`criteria_eval`** — per-task `evaluate(criteria, audit)`
  folded into the arbiter Quality bound.
- **`role_routing`** — heuristic role → vendor mapping.

## Invariants

1. **No cycles.** The supervisor's decomposition is validated
   at emit time; a `Cycle` / `OrphanDependency` / etc. produces
   `MissionEventKind::DecompositionRejected` and aborts the
   mission before any worker spawns.
2. **Parallel within bounds.** The dispatcher in
   `mission_loop` runs up to `ArbiterPolicy::max_parallel_workers`
   concurrent `run_task` futures via `tokio::task::JoinSet`.
3. **Serial integration.** Even with N concurrent workers,
   `MissionWorkspace::integrate_worker` is gated by an
   `Arc<tokio::sync::Mutex<()>>` shared across the JoinSet —
   git can't safely rebase + merge two worker branches into
   `supervisor/main` concurrently.
4. **Memory attach is parallel-safe.** `attach_to_worktree`
   writes only into the worker's own worktree path; concurrent
   calls don't share mutable state.

## Integration

The supervisor adapter carries dependencies and per-task scopes. The mission
loop validates every decomposition, dispatches ready tasks through a bounded
`JoinSet`, evaluates acceptance criteria after audit, and applies the ACL before
integration. Reassign and Split update the same scheduler state rather than
bypassing it.

## Configuration boundary

`ArbiterPolicy::max_parallel_workers` is a code-owned policy setting. The launch
UI does not expose a per-mission override; keeping concurrency policy centralized
prevents an untrusted mission request from expanding its own resource budget.
