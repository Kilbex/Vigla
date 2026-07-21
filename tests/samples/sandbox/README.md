# tests/samples/sandbox

Step-11 gate target. Tiny Rust crate with a deliberately wrong
`multiply` implementation — `cargo test` fails until it's fixed.

```bash
cd tests/samples/sandbox && cargo test
```

This directory is intentionally outside the Vigla workspace
(see `[workspace.exclude]` in the root `Cargo.toml`) so vendor
CLIs that operate on it don't accidentally treat Vigla files
as their working set.
