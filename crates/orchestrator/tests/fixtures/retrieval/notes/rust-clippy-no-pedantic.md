# We do not enable clippy::pedantic

Pedantic flags too many style nits (returning by value vs ref,
explicit `Self::` in match arms) that don't catch real bugs.
Our clippy lane runs `cargo clippy --workspace --all-targets -D
warnings` with the default lint set plus a handful of allow-by-
default lints promoted in `clippy.toml`. Pedantic stays off until
we have a dedicated lint-cleanup gauntlet.
