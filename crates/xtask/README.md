# Workspace tasks

`cargo xtask` makes a clean checkout self-contained. The Tauri host validates
its bundled release `mock-harness` during compilation, so xtask builds that
resource before workspace build, test, and clippy commands.

The crate deliberately has no dependencies. Keep it a thin, transparent wrapper
around Cargo rather than a second build system.

```sh
cargo xtask help
cargo xtask test
cargo xtask clippy
cargo xtask ci
cargo xtask receipt
```

`receipt` reproduces the public 27-case recovery receipt and then proves that a
mission integration returns to its exact pre-merge SHA. It needs no vendor
credentials or network access.
