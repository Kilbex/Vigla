# Cargo incremental compile drops dead-code warnings

When `CARGO_INCREMENTAL=1` (the default) cargo reuses crate
artifacts whenever the source hash is unchanged, which means
`unused_imports` and `dead_code` warnings from prior compiles do
NOT re-print on the next `cargo build`. For a faithful warning
audit run `CARGO_INCREMENTAL=0 cargo clean -p <crate> && cargo
build --all-targets`. Don't trust an incremental build to enumerate
warnings.
