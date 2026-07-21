# Adapter core

Shared primitives for every vendor adapter: the `Adapter` trait, `AdapterCore`
lifecycle bookkeeping, the safe raw-log fallback, quota detection, and
structured memory/context side channels.

This crate owns behavior common to vendors. Vendor-specific wire fields and
message accumulation stay in the vendor crate. It performs no I/O.

```sh
cargo test -p vigla-adapter-core
```
