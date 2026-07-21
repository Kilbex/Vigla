# Vendor adapters

Adapters are Vigla's intentionally narrow vendor boundary: one input line from
a CLI becomes zero or more canonical [`event-schema`](../event-schema/) events.
They do not launch processes, access the filesystem, run git, persist data, or
make network requests. Those effects belong to the orchestrator.

| Crate | Input contract | Launch status |
| --- | --- | --- |
| [`core`](core/) | Shared trait, lifecycle state, raw-log fallback, quota and memory-intent parsing | Stable internal contract |
| [`claude`](claude/) | Claude Code stream JSON | Real-CLI gate |
| [`codex`](codex/) | Codex CLI JSONL | Real-CLI gate |
| [`antigravity`](antigravity/) | Line-oriented Antigravity output | Real-CLI gate |
| [`gemini`](gemini/) | Gemini CLI stream JSON | Maintained legacy / enterprise path |
| [`kiro`](kiro/) | Line-oriented fallback with quota detection and terminal synthesis | Profile + adapter tests |
| [`copilot`](copilot/) | Line-oriented fallback with quota detection and terminal synthesis | Profile + adapter tests |
| [`supervisor`](supervisor/) | Supervisor stream JSON to typed intents | Claude-backed production gate |
| [`conformance`](conformance/) | Recorded transcript to golden event stream | Test-only harness |

## Adapter pull-request checklist

1. Capture the smallest redacted transcript that demonstrates the wire shape.
2. Parse only documented or observed fields; preserve unknown lines as logs.
3. Emit `idle`/`executing` and exactly one terminal state in canonical order.
4. Drain session, quota, memory-intent, and context-request side channels.
5. Add a golden conformance case and review every changed event.
6. Run the package test, then `cargo xtask test` and both clippy gates.

Never put credentials or raw private prompts in fixtures. See the
[`conformance` guide](conformance/) for transcript and golden formats.
