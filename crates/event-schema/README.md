# Event schema

The canonical contract between vendor adapters, the orchestrator, persistence,
and the frontend. It defines versioned worker events, payload types, vendors,
worker states, and memory events. Unknown payload fields remain parseable so a
new producer does not break older replay readers.

Keep this crate dependency-light and free of I/O. A wire-shape change is a
public contract change: update round-trip tests, generated TypeScript bindings,
and compatibility notes together.

```sh
cargo test -p vigla-event-schema
```
