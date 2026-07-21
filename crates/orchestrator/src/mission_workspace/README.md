# `orchestrator::mission_workspace`

Git operations for Vigla missions: branches, worktrees, snapshots, final merges,
whole-mission rollback, cleanup, and retention.

## Layout

- `mod.rs` — the `MissionWorkspace` handle, name/path derivation,
  supervisor branch/worktree creation, worker branch/worktree
  creation, `final_merge` (user-facing merge), `discard`
  (full cleanup). Pre-merge tag helpers live here too.
- `merge.rs` — `MergeOutcome`, `ConflictReport`, `ConflictKind`,
  the rebase-first `try_rebase_then_ff` strategy, and the
  `parse_unmerged` porcelain-v2 conflict parser.
- `revert.rs` — merged missions are undone with a Git revert commit on the
  recorded target branch; staged missions can be reset to their earliest
  pre-integration tag for recovery verification.
- `retention.rs` — `RetentionPolicy` (50 missions OR 7 days),
  `compact_once` (one pass), and a repository-scoped due check scheduled
  when a mission opens that repository.

## Tag namespaces

- `vigla/{mid}/supervisor` — supervisor branch.
- `vigla/{mid}/worker/{wid}` — worker branch.
- `vigla/pre-merge/{mid}/{n}` — pre-merge tag at supervisor
  branch HEAD *before* integration n. Used by staged recovery verification.
- `vigla/snap/{mid}/{n}` — post-merge tag at the integrated
  SHA. Used by external tools for archeology.
- `vigla/revert/{mid}/before/{target_ref}` — target branch immediately before
  final merge; the durable user-facing rollback anchor.
- `vigla/revert/{mid}/merged/{target_ref}` — final mission merge commit.

Only the intermediate `pre-merge` and `snap` families are compacted. Final
`revert` anchors are durable because History continues to authorize Revert
after mission worktrees and branches are gone.

## Reversibility envelope

Per roadmap §2:

1. Every accepted task integration gets a pre-integration tag before its
   supervisor-branch merge.
2. Final merge records the target branch immediately before the mission and
   the exact merge commit, then cleans the mission branches and worktrees.
3. User-facing Revert applies `git revert -m 1` to that merge commit on the
   recorded target. Later commits remain intact; a checked-out target must be
   clean.
4. The receipt-only staged rollback resets the supervisor branch to the
   earliest pre-integration tag, proving all task integrations can be removed.
5. Snapshot retention: 50 missions OR 7 days, whichever longer.
   A per-repository checkpoint allows at most one due pass per 24 hours.

## Conflict handling

`integrate_worker` returns `MergeOutcome::Conflict(report)` when
a rebase produces unmerged paths. The supervisor worktree is left
in its pre-rebase state (the rebase is internally aborted). The
caller (`mission_loop.rs`) emits
`ArbiterDecided{ bound: Some(AuthorityBound::Reversibility) }` and
halts the mission to Attention; the inbox surfaces the decision.

Three conflict shapes are recognised:
- `ConflictKind::AddAdd` — both sides added the same path.
- `ConflictKind::EditEdit` — both sides modified the same path.
- `ConflictKind::DeleteEdit` — one side deleted, the other edited.

Anything else falls into `ConflictKind::Other`.

## Tests

- Unit tests inside each submodule's `#[cfg(test)] mod tests`.
- `orchestrator/tests/conflict_shapes.rs` — three integration
  tests + clean-rebase happy path.
- `orchestrator/tests/revert_mission.rs` — staged reset, merged-target revert,
  later-commit preservation, detached-target, and dirty-checkout coverage.
- `orchestrator/benches/integration/rebase_p50.rs` — criterion
  bench. Budget p50 < 200ms.

## Current boundaries

- **Smarter conflict resolution** — only trivial fast-forward and
  clean-rebase auto-merge today. Semantic merge and `git rerere`
  integration are intentionally outside the launch contract.
- **Cross-platform Git worktrees** — the current lifecycle is verified on
  macOS. Native Windows behavior belongs to the public Windows roadmap rather
  than this module's launch contract.
