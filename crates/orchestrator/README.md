# Orchestrator

Vigla's host-independent product core: mission state and event replay,
supervisor/worker process coordination, worktree isolation, audit and arbiter
policy, recovery, persistence, repository memory, and worker skills.

Side effects terminate at explicit boundaries. Vendor wire parsing stays in
`crates/adapters`; raw SQL stays in this crate; desktop transport stays in the
Tauri host. Mechanical architecture tests enforce both rules.

Subsystem guides live beside the implementation (`src/arbiter/README.md`,
`src/audit/README.md`, `src/mission_workspace/README.md`, and peers). The
system-level narrative is in [`ARCHITECTURE.md`](../../ARCHITECTURE.md).

```sh
cargo test -p vigla-orchestrator
cargo clippy -p vigla-orchestrator --all-targets -- -D warnings
```

Real-vendor gates are opt-in and documented in the root README; ordinary tests
must remain deterministic and credential-free.
