# Supervisor adapter

Parses the supervisor's streamed response into typed mission intents:
decomposition, review/rework, test requests, and completion. The embedded
playbooks define the machine-readable response contract and the evaluation suite
guards its principal decision axes.

```sh
cargo test -p vigla-adapter-supervisor
```

Supervisor output is untrusted. Preserve fenced intent extraction, schema
validation, and actionable parse errors when extending the protocol.
