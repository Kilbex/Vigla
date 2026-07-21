# `orchestrator::judgment` — Completion Judgment

Mission-level "is this done?" verifier. Replaces the freeform
`DeclareComplete` summary at
`mission_supervisor_run/mission_loop.rs` with a structured
[`CompletionVerdict`] emitted on the mission event bus as
`MissionEventKind::CompletionVerdictRendered`.

## Entry point

```rust
let inputs = AssembleInputs {
    worktree_root: workspace.supervisor_worktree_path(),
    touched_files: &mission_touched_files,
    all_subtasks_accepted,
    mission_audit: Some(&best_audit),
    integrated_test_pass: best_audit.test_pass.as_ref(),
    recovery_history: &mission_recovery_summary,
    events: &mission_telemetry,
    scrubs: &mission_scrubs,
};
let verdict = judgment::assemble_verdict(&inputs);
let payload = serde_json::to_string(&verdict)?;
emit(MissionEventKind::CompletionVerdictRendered { payload_json: payload });
```

`mission_loop.rs` builds `AssembleInputs` by deriving every field
from the mission's event-bus history snapshot (the per-task
`run_task` futures stay decoupled from the verdict path). See the
`derive_*` helpers at the bottom of `mission_loop.rs`.

## Sub-modules

| Module | Purpose | Tests |
|--------|---------|-------|
| `verdict.rs` | Type definitions: `CompletionVerdict`, `RiskBand`, `UnresolvedIssue`. | Round-trip serde. |
| `risk_band.rs` | `score_risk(report, history) -> RiskBand`. | Boundary conditions at 0.85 / 0.7. |
| `unresolved.rs` | `collect_unresolved(events, history, scrubs) -> Vec<UnresolvedIssue>` plus `RecoveryHistorySummary`, `ScrubRecord`. | Per-variant fixture coverage. |
| `doc_coverage.rs` | v1 file-scan doc-coverage scorer. | Ratio cases + ineligible-ext drops. |
| `assemble.rs` | `assemble_verdict(inputs) -> CompletionVerdict`. Recommendation derivation lives here. | Accept and fail-closed Scrub paths. |

## Data flow

```
event stream (per-task loop emissions)
  ├─ AuditCompleted ──────────► derive_mission_audit (fallback)
  ├─ PostIntegrationAuditCompleted ─► derive_mission_audit (preferred)
  ├─ ArbiterDecided(Some) ────► derive_assembler_events + derive_all_subtasks_accepted
  ├─ ArbiterDecided(Scrub) ───► derive_scrubs + derive_all_subtasks_accepted
  ├─ ContextBudgetTruncated ──► derive_assembler_events
  ├─ RecoveryDecided ─────────► derive_recovery_summary
  └─ WorkerResultSubmitted ───► derive_touched_files

after dispatch_dag completes
  └─ assemble_verdict(AssembleInputs { ... })
       ├─ score_risk(audit, history)
       ├─ score_doc_coverage(worktree, touched)
       ├─ collect_unresolved(events, history, scrubs)
       └─ derive_recommendation(...) → ArbiterDecision

emit
  └─ MissionEventKind::CompletionVerdictRendered { payload_json }
  └─ MissionEventKind::Completed { ... }  (legacy back-compat signal)
```

## Recommendation rule

| `all_subtasks_accepted` | `residual_risk` | open escalations | `recommendation` |
|--------------------------|------------------|------------------|--------------------|
| true                     | Low / Medium     | none             | `Accept` |
| true                     | High             | none             | `Scrub` |
| true                     | any              | >= 1             | `Scrub` |
| false                    | any              | any              | `Scrub` |

## Risk-band boundaries

| `audit.overall` | `security_flags.len()` | `RiskBand` |
|------------------|------------------------|------------|
| >= 0.85          | 0                      | Low |
| >= 0.85          | 1                      | Medium |
| >= 0.85          | >= 2                   | High |
| 0.7..0.85        | 0..=1                  | Medium |
| 0.7..0.85        | >= 2                   | High |
| < 0.7            | any                    | High |

Plus: a busy recovery history (>= 3 total occurrences) bumps Low
to Medium. Never bumps any other band downward.

## Design boundaries

- **Doc coverage is a heuristic, not a compiler claim.** It checks top-of-file
  documentation on touched source files; rustdoc and linter diagnostics remain
  independent audit signals.
- **`SubtaskScrubbed.task_index` from the event stream alone** — the
  driver supplies `ScrubRecord` separately because the per-task
  `ArbiterDecided` event doesn't carry the task index. A future
  iteration could extend the event payload; the collector stays
  pure.
- **Post-verdict continuation.** Current runtimes do not schedule a new
  supervisor turn from the review screen. A High-risk verdict therefore
  recommends `Scrub` rather than emitting an action the host cannot execute.
  True continuation is specified in the public roadmap.

## Test invariants

- `score_risk` is monotonic in `audit.overall`: as overall increases
  (flags held constant), the band can only stay or improve.
- `assemble_verdict` is deterministic: given identical inputs, the
  output is byte-identical (sans the embedded `AcceptPayload.summary`
  which the caller may replace with the supervisor's prose).
- `collect_unresolved` ordering: escalations (event order) → recovery
  (sorted by class name) → truncations (event order) → scrubs (input
  order).

## End-to-end

`orchestrator/tests/completion_verdict_e2e.rs` runs a scripted-
supervisor + mock-worker mission and asserts:

1. `CompletionVerdictRendered` lands in the event stream.
2. It lands BEFORE the legacy `Completed` event.
3. The payload deserializes as `CompletionVerdict` with the
   happy-path Accept shape.
4. The supervisor's `DeclareComplete` prose threads into
   `AcceptPayload.summary`.

## Frontend surface

`app/src/bindings.ts` exports the typed surface (`CompletionVerdict`,
`RiskBand`, `UnresolvedIssue`) and includes `mission.completion_verdict_rendered`
plus `supervisor.context_budget_truncated` in the `MissionEventKind` union. The
inbox consumes these through the standard tauri-specta channel.
