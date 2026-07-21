# Mock harness

A deterministic, credential-free worker used by tests, screenshots, demos, and
the packaged application's first-run path. Each named script emits canonical
JSONL with stable ordering and configurable timing.

The harness must never require a vendor account, network access, or ambient
state. Add unhappy paths as named scripts with parser and subprocess tests.

```sh
cargo test -p vigla-mock-harness
cargo run -p vigla-mock-harness --bin mock-harness -- --help
```
