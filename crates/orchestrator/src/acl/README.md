# Per-worker File ACL

A fast pre-integration gate
prevents a worker from quietly editing files outside its declared
scope. The two main types are:

- `FileAcl` — effective per-worker allow-list, built as the
  intersection of `MissionSpec.scope_paths` and per-task `scope_paths`.
- `check_diff(diff_paths, &acl)` — pure function returning
  `Result<(), AclViolation>` over a worker's submitted file
  list.

## Layering

`mission_loop.rs` calls `check_diff` immediately after the
worker pass returns, **before** the audit pass. A violation
short-circuits to a synthetic
`ArbiterDecision::Escalate { bound: Scope, … }` with the
denied-paths payload, halting the mission to Attention without
running the audit subprocess.

The audit module's `score_scope` (`orchestrator/src/audit/scope.rs`)
remains as the slower granular backup. It still feeds
`AuditReport.overall` for the cases the pre-flight gate misses
(e.g. when the user has no mission scope declared but a per-task
audit-aware reviewer wants to compute an in-scope ratio anyway).

## ACL sentinel

`MissionWorkspace::write_worker_acl_sentinel` writes
`.vigla/acl.json` into the worker worktree at spawn time.
The sentinel is informational only — the mission loop holds the
live ACL in memory and uses that for the gate. Replay tooling
and post-hoc audit harnesses read the sentinel when the live
ACL has been garbage-collected. Missing sentinel is treated as
"unconstrained" by readers.

## Deliberate non-goals

- **Read isolation through sparse checkout.** Workers can
  still *read* anything in the worktree; only the diff is
  gated. Sparse-checkout has UX cost (workers expect a complete
  tree) and performance cost on large repos, with a marginal
  security improvement over the diff gate. It is not part of the current
  threat model.
- **Glob patterns.** The ACL uses simple path-prefix matching (same
  semantics as `audit::scope::score_scope`). Glob (`src/**/*.rs`)
  syntax is intentionally absent so audit and preflight cannot disagree.
- **Per-vendor scope policy.** ACLs are vendor-agnostic; reassignment preserves
  the same authority envelope.
