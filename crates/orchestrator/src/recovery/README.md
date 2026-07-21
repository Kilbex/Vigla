# Failure Recovery Engine (S5)

Implementation of the supervisor-as-arbiter failure-recovery
subsystem (including quota awareness).

## Entry points

- `recovery::classify_failure(error, ctx, fallback_window_ms, now) ->
  FailureClass` — pure mapper from worker failure surface to typed
  class.
- `recovery::recover(class, &mut history, policy, now) -> RecoveryAction`
  — deterministic policy function that records the occurrence only after
  choosing the action, preserving the full declared retry budget.
- `recovery::VendorQuotaTracker::{mark_exhausted, is_exhausted,
  next_reset, load_from_db, clear}` — host-level shared state.
- `recovery::spawn_quota_wakeup_task(tracker, poll_interval) ->
  QuotaWakeupHandle` — background task that emits
  `WakeupEvent::QuotaReset { vendor }` once a vendor's window
  elapses.

## Failure classes (8)

| Class | Action (first pass) | Terminal escalation |
|-------|---------------------|---------------------|
| `MissingFile` | `RequestSupervisor` | `Escalate(Scope)` |
| `CommandError { Transient }` | `Retry` (1x) | `Escalate(Quality)` |
| `CommandError { Persistent }` | `Escalate(Quality)` | — |
| `MergeConflict` | `Escalate(Quality)` | — |
| `Permissions` | `Escalate(Risk)` | — |
| `InadequateContext` | `RequestSupervisor` | (never blocks) |
| `TaskDrift` | `RequestSupervisor` | `Escalate(Scope)` |
| `VendorCrash` | `Retry` (2x) | `Escalate(Risk)` |
| `QuotaExhausted` | `Pause` | (never escalates) |

Retry budgets are tunable via `RecoveryPolicy`; the defaults above
match the rationale in the policy module's docstring.

## Quota awareness

Per the roadmap §2: vendor quota exhaustion is a *planned pause*,
not a failure. Workers do not burn retry budget while paused; the
wake-up task auto-resumes paused missions when the vendor's window
closes.

Adapters detect quota signals in vendor-specific formats:
- **Claude**: `rate_limit_event { status: exceeded | blocked }`,
  result lines with `api_error_status=429`, hook responses
  mentioning the 5-hour message limit.
- **Codex**: `turn.completed` with `error.code=usage_limit_exceeded`
  or `rate_limit_exceeded`; stderr 429 / rate-limit text.
- **Gemini**: result lines with `RESOURCE_EXHAUSTED`, 429, or
  "quota exceeded"; stderr quota messages.

Each adapter's `take_quota_signal()` drains the buffer once per
call. The supervisor reads via the `WorkerEventStream` drain loop
in `mission_worker_dispatch.rs`.

## Mission lifecycle

`MissionState::Paused { reason: PauseReason }` — distinct from
`Attention` because **no user input is required**. Transitions:
`Executing | Reviewing → Paused`; `Paused → Executing` (on resume)
or `Paused → Aborted` (on cancel).

## Persistence

The `vendor_quota_state` table (migration 0009) stores one row per
vendor. `VendorQuotaTracker::with_pool` rehydrates the in-memory
map on startup and drops stale rows whose reset time has already
passed.

## Tests

- Unit tests live alongside each module file under
  `orchestrator/src/recovery/*.rs`.
- Integration tests:
  - `orchestrator/tests/recovery_unit.rs` — exhaustive
    classify+recover coverage.
  - `orchestrator/tests/recovery_quota_e2e.rs` — adapter → tracker
    → wake-up loop, plus persistence and multi-vendor isolation.
  - `orchestrator/tests/recovery_injected_failures.rs` — the U7
    acceptance gate (three injected surfaces across the missing-file and
    vendor-crash paths).
  - `orchestrator/tests/launch_recovery_receipt.rs` — the public 27-case
    bounded-recovery receipt and committed-evidence freshness gate.
- Bench: `cargo bench -p vigla-orchestrator --bench
  recovery_classify_p99` produces p50/p99 for the
  `classify_failure × recover` pair (target: ≤ 50µs p99).
