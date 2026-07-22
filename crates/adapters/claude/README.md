# Claude Code adapter

Translates Claude Code stream-JSON stdout/stderr into canonical events and
captures resumable session IDs, usage, quota signals, and memory intents.

Deterministic synthetic transcripts and goldens are the normal development path.
The optional production gate is listed in the root README and requires an
authenticated local `claude` binary.

```sh
cargo test -p vigla-adapter-claude
```
