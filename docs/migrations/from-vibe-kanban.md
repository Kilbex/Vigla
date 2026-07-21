# Coming from vibe-kanban

Vigla and vibe-kanban overlap at the point where a developer dispatches work
to coding agents, but they organize the work differently. This guide is a
concept map, not an importer or a claim that one workflow replaces every use
of the other.

## Concept map

| In a board-oriented workflow | In Vigla |
| --- | --- |
| Project or workspace | Target git repository and mission history |
| Board item | Mission objective or decomposed subtask |
| Assigned agent | Worker in a mission roster |
| Task status column | Canonical worker and mission state |
| Human review of a finished card | Supervisor audit plus completion verdict |
| Re-open or undo a task | Continue/retry a worker, or revert the integrated mission |

## What changes

Vigla asks you to define an authority envelope rather than manage each task
through a board. The supervisor decomposes one objective, dispatches workers
into isolated worktrees, evaluates their submissions against Scope,
Reversibility, Risk, and Quality, then presents the integrated outcome. The
primary unit is the mission and its audited merge, not a persistent kanban
card.

## What Vigla adds

- a mixed roster of supported vendor CLIs;
- a typed event contract across those vendors;
- one isolated git worktree per worker;
- an independent audit decision before integration;
- a structured verdict with tests and residual risk;
- staged integration snapshots plus target-branch whole-mission revert;
- deterministic replay and credential-free failure scenarios.

## What Vigla does not replace

Vigla is not a general project-management board, team backlog, cloud workspace,
hosted agent service, or cross-project portfolio view. It does not import
vibe-kanban's database, task history, credentials, or vendor sessions. Keep the
existing installation available until the evaluation is complete.

## Low-risk evaluation

1. Open the [read-only browser replay](https://kilbex.github.io/Vigla/demo/)
   and exercise all three outcomes.
2. Clone Vigla and run the credential-free mock mission against a disposable
   test repository.
3. Compare the completion verdict and revert path with one representative
   board task; do not start with production work.
4. Build the local DMG from source only if that mission-level workflow fits.
5. Add one real vendor CLI at a time and keep its normal credential boundary.

Questions about the mapping belong in
[GitHub Discussions](https://github.com/Kilbex/Vigla/discussions/categories/q-a);
reproducible Vigla defects belong in Issues.
