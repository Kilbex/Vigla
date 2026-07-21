# Quality Audit Layer

Implementation of the supervisor-as-arbiter quality-audit subsystem.

## Entry point

`audit::audit_submission(&AuditInput) -> Result<AuditReport, AuditError>`.

## Scorers

| Module | Score | Notes |
|--------|-------|-------|
| `test_pass` | `TestPassScore` | Runs `MissionSpec.tests` when set, otherwise `cargo test` (Rust) or `npm test` (Node); skipped at Smoke tier |
| `scope` | `ScopeScore` | Pure: touched_files ∩ scope_paths via stdlib Path::starts_with |
| `regression` | `RegressionScore` | Pre/post baseline delta; v1 takes optional baseline from caller |
| `lint` | `LintScore` | `cargo fmt --check` + `cargo clippy`; biome when config present |
| `security` | `Vec<SecurityFlag>` | Path-based (no content scan) — explicit secret patterns, migrations, mass deletion |

## Tiers

- **Smoke** (~30s budget; typically <1ms in practice) — scope + security only.
- **Standard** (~2min) — Smoke + test_pass + lint.
- **Deep** (~5min) — Standard + regression (requires baseline).

Tier is auto-selected from diff size and security-flag hits via
`AuditTier::auto_select`, or set explicitly via `AuditInput.tier`.

## Composite

`composite::blend_overall(&AuditReport, &WeightProfile)` produces
`AuditReport.overall`. Default weights:
test 0.40, scope 0.20, regression 0.25, lint 0.15;
security penalty 0.10 per flag. Result routed through `clamp_score`
(NaN-safe, clamps to [0.0, 1.0]).

## CLI

`cargo run -p vigla-orchestrator --bin orchestrator_audit -- --help`
prints the usage banner. `--root <path>` (required) plus optional
`--scope a,b,c` and `--tier smoke|standard|deep` emit a pretty-
printed JSON `AuditReport`. Used for debugging only — not user-facing.

## Persistence

`MissionController` records post-integration worker audits and the latest
mission-level audit through a dedicated event subscription. This recorder is
independent of UI forwarding, uses source-event timestamps for idempotency,
and backs the cross-mission History view. Rows live in `audit_reports`
(migration `0007_audit_history.sql`).

## Failure and baseline semantics

Runner errors propagate from `audit_submission`. The mission runtime catches
them at its trust boundary, emits an explanatory `WorkerProgress` event, and
uses an unscored report with overall `0.0`; the arbiter therefore fails closed
instead of accepting work whose checks could not run.

Regression scoring is caller-supplied: it participates only when
`AuditInput.baseline` is present. Without a baseline, the component is omitted
from the blended denominator rather than contributing a synthetic passing
score. Live mission audits currently rely on the independently rerun test and
lint signals and do not claim a pre-mission regression comparison.
