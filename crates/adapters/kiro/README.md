# Kiro adapter

Line-oriented fallback for Kiro CLI output. Non-empty stdout/stderr lines become
canonical logs and process exit determines the terminal event.

```sh
cargo test -p vigla-adapter-kiro
```

This adapter is profile-backed but not yet end-to-end verified. A richer parser
must start from a redacted, reproducible wire-format fixture.
