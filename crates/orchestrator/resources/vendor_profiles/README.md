# Vigla Vendor Profiles

Vendor profiles are the single source of truth for vendor CLI launch
shape and declared side effects.

They are intentionally small. A profile does not describe model
capabilities, tool capability parity, or a provider abstraction. It
only records:

- which CLI binary Vigla launches,
- which adapter crate reads that CLI's worker event stream,
- which command templates Vigla may use for supervisor and worker
roles,
- which side effects the vendor CLI may perform.

The only allowed declared side-effect kinds are:

- `package_install`
- `paid_api_call`
- `external_mutation`
- `network_egress`

The orchestrator validates bundled profiles in Rust tests. Routing
that needs vendor-specific CLI flags should be added here, not spread
through mission runtime code.
