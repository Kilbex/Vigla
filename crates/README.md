# Rust workspace guide

Vigla keeps product policy in a host-independent Rust workspace. The Tauri host
is an edge adapter: it exposes commands and forwards events, while mission
state, persistence, supervision, auditing, and recovery stay testable here.

| Crate | Responsibility | Boundary |
| --- | --- | --- |
| [`orchestrator`](orchestrator/) | Mission lifecycle, worktrees, persistence, audit, recovery, memory, and skills | No Tauri or frontend concerns |
| [`event-schema`](event-schema/) | Canonical worker event and memory types | Minimal serialization-only dependency surface |
| [`mock-harness`](mock-harness/) | Deterministic worker streams for demos and tests | No credentials or network |
| [`adapters`](adapters/) | Vendor bytes to canonical events | Pure translation; no process, git, or database I/O |
| [`xtask`](xtask/) | Self-contained workspace build/test entrypoint | Standard-library-only command runner |

The desktop host lives at [`app/src-tauri`](../app/src-tauri/). Start with
[`ARCHITECTURE.md`](../ARCHITECTURE.md) for the end-to-end flow and
[`CONTRIBUTING.md`](../CONTRIBUTING.md) for required checks.

## Common commands

```sh
cargo xtask test
cargo xtask clippy
cargo xtask clippy --release
cargo fmt --all -- --check
```

Use a package-level test while iterating, then run the complete gate before a
pull request. The Tauri host bundles the release mock harness, so prefer
`cargo xtask test` over a bare workspace test from a clean checkout.
