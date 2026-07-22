# Codex CLI adapter

Translates Codex CLI JSONL into canonical state, log, file, command, usage, and
completion events. Unknown records degrade to trace logs so vendor additions do
not crash a mission.

```sh
cargo test -p vigla-adapter-codex
```

Use deterministic synthetic fixtures for parser changes. The optional real-CLI
gate is documented in the root README.
