# Newcomer Task Queue

These tasks are scoped so a new contributor can land a first adapter PR in
under two hours.

When filing these as GitHub issues, use labels such as `good first issue`, `adapter`, `docs`, `test`, and `help wanted`.

## Adapter Tasks

| Task | Why it helps | Expected files | Suggested labels |
| --- | --- | --- | --- |
| Add one legacy Gemini fixture covering a failed command | Improves confidence in the maintained compatibility path | `crates/adapters/gemini/tests/fixtures/`, `crates/adapters/gemini/tests/from_fixture.rs` | `good first issue`, `adapter`, `test` |
| Add one Codex fixture with tool output and no file changes | Clarifies no-op completion behavior | `crates/adapters/codex/tests/fixtures/`, `crates/adapters/codex/tests/from_fixture.rs` | `good first issue`, `adapter`, `test` |
| Add an Antigravity transcript to the conformance harness | Locks the current line-oriented fallback contract to a reviewed golden | `crates/adapters/antigravity/tests/conformance*` | `good first issue`, `adapter`, `test` |
| Draft an Opencode adapter proposal | Defines scope before creating a new workspace crate | `docs/adapters/opencode.md` | `good first issue`, `adapter`, `planning` |
| Add a malformed-line fixture for one adapter | Prevents parser regressions on noisy CLI output | one `crates/adapters/<vendor>/tests/*` file | `good first issue`, `adapter`, `resilience` |

## Docs Tasks

| Task | Why it helps | Expected files | Suggested labels |
| --- | --- | --- | --- |
| Improve screenshot alt text in the README | Makes the product tour more useful to screen-reader users | `README.md` | `good first issue`, `docs`, `accessibility` |
| Expand the "Real CLI adapters" section with troubleshooting | Helps users set PATH and credentials correctly | `README.md` | `good first issue`, `docs` |
| Add a glossary entry for one mission event | Makes frontend and adapter vocabulary clearer | `ARCHITECTURE.md` or a future docs file | `good first issue`, `docs` |

## Code Tasks

| Task | Why it helps | Expected files | Suggested labels |
| --- | --- | --- | --- |
| Add unit tests around `host_services::worker_backend_for_model` edge cases | Keeps host routing out of Tauri glue | `crates/orchestrator/src/host_services.rs` | `good first issue`, `test` |
| Add a repository replay regression for an unknown future event | Preserves forward compatibility | `crates/orchestrator/tests/persistence.rs` | `good first issue`, `test` |
| Add a focused parser cap test for stderr noise | Protects performance and memory safety | `crates/orchestrator/src/parser.rs` or adapter tests | `good first issue`, `test`, `resilience` |

## Maintainer Checklist for Newcomer Issues

- Keep the task limited to one crate or one doc.
- Include the exact command to run.
- Include a sample expected assertion or output.
- Avoid requiring real vendor credentials for first issues.
- Prefer fixture-based tests over manual screenshots for adapter work.
