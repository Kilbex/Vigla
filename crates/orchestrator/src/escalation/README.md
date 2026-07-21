# Silent-vs-Notify Policy (Escalation Module)

Implementation of the silent-by-default policy gate.

## Entry point

`escalation::visibility_for(&MissionEventKind) -> EventVisibility`

The function is a pure O(1) exhaustive match. Adding a new
`MissionEventKind` variant fails to compile until classified.
**This invariant is the maintainability backbone of the
silent-by-default behaviour.** Do not bypass with a wildcard
arm — keep every variant explicit.

## Verdict shape

```rust
pub enum EventVisibility {
    Internal,                                       // never surfaced
    PowerUserOnly,                                  // shown when "Show all events" is on
    Inbox { kind: InboxKind, severity: Severity },  // always shown
}
```

Three `InboxKind`s — `Escalation`, `Completion`, `SideEffect` —
combine with three `Severity` levels — `Info`, `Warning`,
`ActionRequired`. Only `ActionRequired` fires the macOS native
banner (gated additionally on app focus on the frontend side).

## Routing summary

| Event variant                        | Verdict                                          |
|--------------------------------------|--------------------------------------------------|
| `Created` / `ExecutionStarted`       | Internal                                         |
| `AuditCompleted`                     | Internal                                         |
| `ArbiterDecided` (Extend)            | Internal                                         |
| `Decomposition`                      | PowerUserOnly                                    |
| `WorkerSpawned`/`Progress`/`Submitted` | PowerUserOnly                                  |
| `ReviewStarted` / `Integrated`       | PowerUserOnly                                    |
| `TestResult`                         | PowerUserOnly                                    |
| `PlanConfirmed` / `PlanRegenerationRequested` / `MissionExtended` | PowerUserOnly |
| `PlanProposed`                       | Inbox{Escalation, ActionRequired}                |
| `Completed`                          | Inbox{Completion, Info}                          |
| `MergeResolved`                      | Inbox{Completion, Info}                          |
| `Aborted`                            | Inbox{Escalation, Warning}                       |
| `SideEffectLogged`                   | Inbox{SideEffect, Warning}                       |
| `SubSupervisorRefused`               | Inbox{Escalation, Warning}                       |
| `ArbiterDecided` (Accept)            | Inbox{Completion, Info}                          |
| `ArbiterDecided` (Scrub)             | Inbox{Escalation, Warning}                       |
| `ArbiterDecided` (Escalate)          | Inbox{Escalation, ActionRequired}                |

## Frontend integration

The frontend ingest (`app/src/missions/ingest.ts`) consults the
Rust mapping via the Tauri command `mission_event_visibility`.
Verdicts are cached per `(type, discriminator)` — most variants
key by `type` alone since their verdict is a pure function of the
variant tag. `arbiter.decided` adds the `bound` field and the
decision-kind tag (`accept` / `extend` / `scrub` / `escalate`) to
the cache key so each distinct Rust verdict caches independently.
The reducer remains pure; an inbox `upsert` fires via a registered
side-channel appender once the visibility verdict resolves. macOS
native banners fire only for `ActionRequired` cards when the app
is not focused.

If the IPC call fails (orchestrator degraded), a conservative
fallback table classifies the most common terminal events into
inbox cards (Escalates as Warnings — favouring false positives
over silent loss). Unknown event types fall back to `Internal`.

## Persistence

Internal events stay in SQLite via the existing event log so
debugging and replay continue to work; the policy gate is a
*surfacing* layer, not a filtering layer.

## When you add a new MissionEventKind variant

The compiler will tell you. Add a match arm in
`visibility::visibility_for` and a corresponding row in the
routing table above. Add tests in
`visibility::mapping_tests` plus the fixture entry in
`visibility::invariant_tests::fixture_event` so the proptest
coverage matches the variant count.
