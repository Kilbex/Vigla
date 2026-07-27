# Vendor adapters

Adapters are Vigla's intentionally narrow vendor boundary: one input line from
a CLI becomes zero or more canonical [`event-schema`](../event-schema/) events.
They do not launch processes, access the filesystem, run git, persist data, or
make network requests. Those effects belong to the orchestrator.

| Crate | Input contract | Conformance cases (CI) | Fixture suite | Real-CLI gate |
| --- | --- | --- | --- | --- |
| [`core`](core/) | Shared trait, lifecycle state, raw-log fallback, quota and memory-intent parsing | — | — | — |
| [`claude`](claude/) | Claude Code stream JSON | 6 | `tests/fixtures/claude_simple.jsonl` | opt-in, local |
| [`codex`](codex/) | Codex CLI JSONL | 4 | `tests/fixtures/codex_simple.jsonl` | opt-in, local |
| [`gemini`](gemini/) | Gemini CLI stream JSON | 4 | `tests/fixtures/happy_path.jsonl` | opt-in, local (legacy / enterprise path) |
| [`antigravity`](antigravity/) | Line-oriented Antigravity output | 4 | none | opt-in, local |
| [`kiro`](kiro/) | Line-oriented fallback with quota detection and terminal synthesis | 4 | none | none yet |
| [`copilot`](copilot/) | Line-oriented fallback with quota detection and terminal synthesis | 4 | none | none yet |
| [`supervisor`](supervisor/) | Supervisor stream JSON to typed intents | — | `tests/fixtures/` | Claude-backed, opt-in, local |
| [`conformance`](conformance/) | Transcript to golden event stream | — | — | test-only harness |

All six shipping vendor adapters run through the shared
[`conformance`](conformance/) harness on every pull request. Real-CLI gates are
`#[ignore]`d and run in no CI workflow.

Every fixture in this tree is **synthetic**. None is a recording of a real
session; paths use `/Users/alice/…` placeholders and no fixture contains a
token, account, or private repository name. `antigravity`, `kiro`, and
`copilot` wrap `RawLogAdapter` and parse nothing, so there is no vendor output
shape to pin — their goldens pin the schema-side contract only (the opening
`idle`→`executing` pair, verbatim passthrough of stdout and stderr, empty lines
emitting nothing, the shared quota detector firing, and the terminal event
synthesised from exit status). Each `conformance.rs` module doc says so; keep
that framing rather than upgrading it to "recorded vendor transcripts".

## Adapter pull-request checklist

1. Add the smallest transcript that demonstrates the wire shape, built only
   from line types the parser actually dispatches on. Synthetic is fine and
   expected; a real transcript must be redacted.
2. Parse only documented or observed fields; preserve unknown lines as logs.
3. Emit `idle`/`executing` and exactly one terminal state in canonical order.
4. Drain session, quota, memory-intent, and context-request side channels.
5. Add a golden conformance case and review every changed event.
6. Run the package test, then `cargo xtask test` and both clippy gates.

Never put credentials or raw private prompts in fixtures. See the
[`conformance` guide](conformance/) for transcript and golden formats.
