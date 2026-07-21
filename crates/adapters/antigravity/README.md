# Antigravity adapter

The current Google-path worker adapter. Antigravity does not expose a stable
structured event stream that Vigla can depend on, so this crate deliberately
uses the shared line-oriented contract and synthesizes the terminal outcome from
the process exit.

```sh
cargo test -p vigla-adapter-antigravity
```

The production `MissionRuntime` gate is opt-in and documented in the root
README. Do not infer structured fields until a stable upstream contract exists.
