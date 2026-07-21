# `orchestrator::arbiter`

The Authority Model. Consumes an [`audit::AuditReport`] and
emits a typed [`ArbiterDecision`] (`Accept` | `Extend` | `Scrub` |
`Escalate`). The automated floor and supervisor's semantic review are
independent signals; either can prevent integration.

## Entry point

```rust
let decision = arbiter::decide(&audit_report, &context, &policy);
```

- `audit_report` — produced by [`audit::audit_submission`].
- `context` — per-decision state (attempts used, submission
  summary, touched files, scope paths). Built by `mission_loop`.
- `policy` — [`ArbiterPolicy::default()`] gives the enforced quality,
  rework, risk, audit-tier, and parallelism defaults.

`decide()` is pure: deterministic given the same triple, no IO, no
allocations beyond the decision payload.

## Authority bounds

Per roadmap §2, four orthogonal bounds. The first to trip
determines the decision:

1. **Scope** (`scope_check.rs`) — diff intersected with
   `scope_paths`. Out-of-scope changes always escalate.
2. **Reversibility** — Git snapshot/rebase failures are mapped to this
   bound at the mission integration boundary; the pure arbiter does no I/O.
3. **Risk** (`risk_check.rs`) — `audit.security_flags` filtered
   by `policy.risk_detectors_enabled`. Any hit escalates.
4. **Quality** (`quality_check.rs`) — `audit.overall` vs
   `policy.quality_min`. Recoverable via rework budget; below
   floor with budget remaining → `Extend(Revise)`; budget
   exhausted → `Scrub(QualityExhausted)`.

## Decisions

- `Accept(AcceptPayload)` — `mission_loop` integrates.
- `Extend { rework_kind, attempts_remaining }` — `mission_loop`
  re-runs the worker with the selected `ReworkKind`.
- `Scrub { reason, retained_artifacts, partial_audit }` —
  `mission_loop` skips integration and moves to the next task.
- `Escalate { bound, evidence, suggested_user_action }` —
  `mission_loop` halts mission to `Attention`; the inbox surfaces it.

## Policy

`ArbiterPolicy::default()` contains only values currently enforced by
the arbiter or scheduler. Notable choices:

- **T2** `quality_min = 0.7` — audit composite floor.
- **T3** rework budget 2/task, 3/mission.
- **T5** default audit tier = `Standard`.
- Maximum parallel workers = 4.

## Tests

- `mod tests` in each sub-module (`*_check.rs`, `policy.rs`, etc.).
- `proptest_decide` in `mod.rs` — three invariants:
  - Risk flag ⇒ always escalate.
  - Quality below floor + budget exhausted ⇒ scrub, never extend.
  - Passing score + no violations ⇒ always accept.
- `orchestrator/tests/arbiter_decide.rs` — integration tests
  covering Accept + Escalate paths.

## Bench

`orchestrator/benches/arbiter/decide_p99.rs` — criterion bench for
two scenarios. Budget: p99 < 10ms (decide() is pure, so the budget
is generous and exists to catch hotspot regressions).

## Rework Engine

### The six rework kinds

The arbiter's `Extend { rework_kind, attempts_remaining }` decision
variant carries a `ReworkKind` describing what the mission loop
should do between worker passes:

| variant            | data                                | side effect                                  |
|--------------------|-------------------------------------|----------------------------------------------|
| `Revise`           | `directive: String`                 | re-run same worker with directive            |
| `Reassign`         | `from_worker, to_vendor: Option<_>` | fresh worker_id, optional vendor swap        |
| `Split`            | `sub_tasks: Vec<TaskDescriptor>`    | scrub task, queue sub-tasks in the DAG scheduler |
| `Narrow`           | `reduced_scope: Vec<PathBuf>`       | per-pass scope_paths overlay                 |
| `Rebrief`          | `new_brief: String`                 | per-pass task title overlay                  |
| `MarkUnachievable` | `rationale: String`                 | escalate to Attention with rationale         |

### Who chooses the kind

The **supervisor** chooses the kind via its `review` action. The
arbiter does not have its own per-kind decision logic — it
propagates the supervisor's choice into `Extend.rework_kind` via
the `DecisionContext.preferred_rework_kind` carrier.

The supervisor's playbook (in `adapters/supervisor/src/playbook.md`
§3 `review`) documents when each kind is appropriate and provides
JSON envelope examples.

### Dispatch flow

```
worker pass → audit_submission → AuditReport
              ↓
              semantic supervisor review turn → ReviewIntent
              ↓
              rework_kind_from_review_intent → Option<ReworkKind>
              ↓
              decide(audit, ctx.with(preferred_rework_kind), policy)
              ├─ Accept    → integrate
              ├─ Extend    → plan_for_kind → ReworkPlan → apply
              ├─ Scrub     → drop, advance
              └─ Escalate  → Attention

ReworkPlan overlays applied by mission_loop:
  directive / scope_overlay / rebrief_overlay /
  fresh_worker_id / vendor_swap / append_sub_tasks

NextLoopAction: Continue (re-pass), Skip (advance), Scrub, Escalate.
```

### Layer boundaries

- The task-graph scheduler owns Split queueing and dependency readiness.
- The ACL module enforces Narrow's effective scope before audit/integration.
- The inbox renders terminal attention and scrub outcomes.
- Memory and skills modules own their own token budgets.

### Bench baseline

`cargo bench -p vigla-orchestrator --bench decide_p99` —
arbiter_decide_per_kind group:

| kind             | p50 target |
|------------------|------------|
| revise           | < 1µs      |
| reassign         | < 1µs      |
| split            | < 1µs      |
| narrow           | < 1µs      |
| rebrief          | < 1µs      |
| mark_unachievable| < 1µs      |

decide() is pure; the benches catch allocation regressions, not
algorithmic ones.
