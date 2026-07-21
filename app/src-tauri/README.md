# Tauri host

The macOS desktop edge for the host-independent Rust orchestrator. It owns IPC,
native dialogs and notifications, app lifecycle, logging setup, and forwarding
typed events to the React UI.

Business policy belongs in `crates/orchestrator`; vendor parsing belongs in
`crates/adapters`. Keep commands thin enough to test through the Rust library or
browser IPC mocks.

```sh
cargo test -p vigla-host
```

From a clean checkout, use `cargo xtask test` so the bundled release mock
harness is built before Tauri validates its resources.
