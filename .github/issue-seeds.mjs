export const issueSeeds = Object.freeze([
  {
    title: "adapter: add a legacy Gemini failed-command fixture",
    labels: ["good first issue", "help wanted", "adapter", "test"],
    body: `## Why

Gemini is a maintained legacy/enterprise compatibility lane. A redacted failed-command fixture keeps its parser behavior explicit without presenting it as the primary Google path.

## Scope

- Add one minimal golden fixture under \`crates/adapters/gemini/tests/fixtures/\`.
- Assert the canonical failure category, retryability, exit code, and redacted summary.
- Do not add live credentials, process spawning, or unrelated parser cleanup.

## Done when

- [ ] The fixture contains no user path, token, prompt, or repository content.
- [ ] The test fails if the adapter drops or misclassifies the command failure.
- [ ] Existing Gemini fixtures remain green.

## Verify

\`\`\`sh
cargo test -p vigla-adapter-gemini
cargo test -p vigla-adapter-conformance
\`\`\``,
  },
  {
    title: "adapter: cover Codex tool output with no file changes",
    labels: ["good first issue", "help wanted", "adapter", "test"],
    body: `## Why

A successful tool-only Codex turn must not invent file activity. This fixture pins that no-op completion boundary.

## Scope

- Add one redacted stream fixture under \`crates/adapters/codex/tests/fixtures/\`.
- Assert the emitted tool/log and completion events.
- Assert that no \`file_activity\` event is emitted.

## Done when

- [ ] The assertion would fail if a parser fallback fabricated a modified file.
- [ ] Sequence numbers and terminal completion stay canonical.
- [ ] No real Codex account is required.

## Verify

\`\`\`sh
cargo test -p vigla-adapter-codex
cargo test -p vigla-adapter-conformance
\`\`\``,
  },
  {
    title: "adapter: add an Antigravity conformance transcript",
    labels: ["good first issue", "help wanted", "adapter", "test"],
    body: `## Why

Antigravity is the current verified Google worker path. A reviewed transcript in the shared conformance harness catches drift beyond the crate-local happy path.

## Scope

- Add one minimal, redacted Antigravity transcript fixture.
- Route it through \`vigla-adapter-conformance\`.
- Assert state, progress, completion/failure, and monotonically increasing sequence numbers.

## Done when

- [ ] The fixture documents which line-oriented fallback it represents.
- [ ] The test needs no vendor binary or credentials.
- [ ] Parser caps and redaction expectations remain intact.

## Verify

\`\`\`sh
cargo test -p vigla-adapter-antigravity
cargo test -p vigla-adapter-conformance
\`\`\``,
  },
  {
    title: "docs: specify an OpenCode adapter before adding a crate",
    labels: ["good first issue", "help wanted", "adapter", "documentation", "planning"],
    body: `## Why

The adapter boundary is small only when its input contract is understood before implementation. OpenCode needs an evidence-backed proposal, not a speculative crate.

## Scope

Create \`docs/adapters/opencode.md\` covering the CLI invocation, machine-readable output if available, session/resume behavior, terminal signals, sample redacted lines, failure modes, and mapping to canonical events. Name unknowns explicitly.

## Done when

- [ ] Every claimed CLI flag links to primary documentation or a captured \`--help\` version.
- [ ] The proposal keeps process management outside the adapter.
- [ ] At least one happy and one failure fixture shape are sketched.
- [ ] No implementation crate or new dependency is added.

## Verify

\`\`\`sh
test -s docs/adapters/opencode.md
rg -n "canonical|fixture|failure|session|process" docs/adapters/opencode.md
\`\`\``,
  },
  {
    title: "adapter: preserve noisy malformed lines without losing terminal state",
    labels: ["good first issue", "help wanted", "adapter", "test", "resilience"],
    body: `## Why

Vendor CLIs sometimes interleave warnings or malformed lines with structured output. The adapter must tolerate bounded noise without hiding the final state.

## Scope

Choose one adapter with no equivalent fixture. Add a malformed-line fixture between valid events and assert the documented fallback plus the terminal event. Keep the change within that adapter crate.

## Done when

- [ ] The malformed input is preserved or summarized according to the adapter policy.
- [ ] The following valid event and terminal state still arrive.
- [ ] The test proves no panic and no unbounded payload growth.

## Verify

\`\`\`sh
cargo test -p vigla-adapter-<vendor>
cargo test -p vigla-adapter-conformance
\`\`\`

Replace \`<vendor>\` in the issue comment before claiming the task.`,
  },
  {
    title: "docs: add real-CLI PATH and authentication troubleshooting",
    labels: ["good first issue", "help wanted", "documentation"],
    body: `## Why

Most first-run real-worker failures happen at the executable and authentication boundary. A short diagnostic guide should resolve those without exposing secrets.

## Scope

Add a guide under \`docs/\` for Claude Code, Codex, and Antigravity covering binary discovery, version output, vendor-owned login checks, safe redaction, and the difference between mock and real gates. Link it from README vendor support and SUPPORT.md.

## Done when

- [ ] Commands do not print tokens or credential files.
- [ ] Each vendor section names the expected binary from the README table.
- [ ] The guide explains when to use an ignored real-CLI test.
- [ ] All new relative links resolve.

## Verify

\`\`\`sh
rg -n "claude|codex|agy|redact|--ignored" docs README.md SUPPORT.md
node scripts/build-site.mjs --check
\`\`\``,
  },
  {
    title: "docs: add one canonical mission-event glossary entry",
    labels: ["good first issue", "help wanted", "documentation"],
    body: `## Why

The public lexicon should let adapter and frontend contributors use the same event vocabulary without reverse-engineering generated bindings.

## Scope

Choose one event that is not yet explained in \`docs/lexicon.md\`. Document its producer, payload purpose, visibility, persistence/replay behavior, and one consumer. Link the authoritative Rust type.

## Done when

- [ ] The entry distinguishes wire fact from UI interpretation.
- [ ] Field names match the generated schema.
- [ ] No duplicate or conflicting definition is introduced.

## Verify

\`\`\`sh
cargo test -p vigla-event-schema
rg -n "<event-name>" docs/lexicon.md crates/event-schema/src
\`\`\`

Replace \`<event-name>\` in the issue comment before claiming the task.`,
  },
  {
    title: "test: cover worker model-routing edge cases",
    labels: ["good first issue", "help wanted", "test", "resilience"],
    body: `## Why

Model names select a worker backend at a trust boundary. Empty, mixed-case, unknown, and legacy names should never route to a surprising vendor.

## Scope

Add table-driven unit tests around \`host_services::worker_backend_for_model\`. Start with current behavior; change production routing only if a failing case proves a contract bug and document that change.

## Done when

- [ ] Empty, whitespace, case, known-prefix, legacy, and unknown inputs are covered.
- [ ] Every case names its expected backend or error.
- [ ] No Tauri command code is added to the business-logic test.

## Verify

\`\`\`sh
cargo test -p vigla-orchestrator host_services
cargo clippy -p vigla-orchestrator --all-targets -- -D warnings
\`\`\``,
  },
  {
    title: "test: cap stderr noise without losing the final failure",
    labels: ["good first issue", "help wanted", "test", "resilience"],
    body: `## Why

A noisy CLI must not grow parser memory without bound, and truncation must not erase the failure users need to act on.

## Scope

Add a focused parser test with stderr above the documented cap followed by a terminal failure. Assert bounded retained data, truncation signaling, and preservation of the final actionable summary.

## Done when

- [ ] The fixture is generated in-memory rather than committed as a huge log.
- [ ] The assertion would fail if retained data grows with the full input.
- [ ] The terminal failure category and recovery hint survive.

## Verify

\`\`\`sh
cargo test -p vigla-orchestrator parser
cargo clippy -p vigla-orchestrator --all-targets -- -D warnings
\`\`\``,
  },
  {
    title: "roadmap: Linux desktop packaging and parity",
    labels: ["roadmap", "platform/linux", "help wanted"],
    body: `## Outcome

Track the v0.2 Linux desktop work without implying current support. The full scope, non-goals, sequencing, and acceptance gate live in [docs/roadmap/linux.md](https://github.com/Kilbex/Vigla/blob/main/docs/roadmap/linux.md).

## First slice

Inventory macOS-only host imports and map each to a target-gated adapter or a portable replacement. Post file-and-line evidence before proposing packaging changes.

## Acceptance

- [ ] A supported Ubuntu LTS target is named from a fresh-VM proof.
- [ ] Native mock missions, worktree isolation, notifications fallback, and mission revert pass.
- [ ] Local AppImage and Debian package recipes verify checksums and upload nothing.
- [ ] README support wording changes only after the full gate is green.

## Current portable gate

\`\`\`sh
cargo check --workspace --all-targets --exclude vigla-host
cargo clippy --workspace --all-targets --exclude vigla-host -- -D warnings
cargo test --workspace --exclude vigla-host
\`\`\``,
  },
  {
    title: "roadmap: decide and prove the Windows execution boundary",
    labels: ["roadmap", "platform/windows", "help wanted", "planning"],
    body: `## Outcome

Choose and prove either a native Windows lane or an explicit WSL-coordination lane before implementation broadens. The decision and acceptance criteria live in [docs/roadmap/windows.md](https://github.com/Kilbex/Vigla/blob/main/docs/roadmap/windows.md).

## First slice

Prototype path translation, process-tree termination, and git worktrees for both candidate lanes in disposable repositories. Record failures and recommend one primary boundary; do not add a support badge or installer yet.

## Acceptance

- [ ] Evidence covers spaces, drive letters, long paths, line endings, and process cleanup.
- [ ] Credential and app-data ownership is explicit.
- [ ] The rejected lane and tradeoff are documented.
- [ ] Packaging remains local-only with no CI artifact upload.

## Verify

The prototype report must include exact Windows image, shell, git, Node, Rust, and WSL versions plus commands that a second contributor can repeat.`,
  },
]);

export const labels = Object.freeze([
  ["adapter", "5319E7", "Vendor CLI adapter boundary"],
  ["accessibility", "0E8A16", "Accessibility and inclusive interaction"],
  ["blocked", "B60205", "Waiting on a named external condition"],
  ["bug", "D73A4A", "Confirmed or reported defect"],
  ["documentation", "0075CA", "Documentation-only work"],
  ["enhancement", "A2EEEF", "Focused product improvement"],
  ["good first issue", "7057FF", "Scoped first contribution"],
  ["help wanted", "008672", "Maintainer welcomes a contributor"],
  ["needs triage", "D4C5F9", "Not yet reproduced or classified"],
  ["planning", "C5DEF5", "Design or evidence before implementation"],
  ["platform/linux", "FCC624", "Linux platform work"],
  ["platform/windows", "0078D4", "Windows platform work"],
  ["priority/critical", "B60205", "Data, credential, destructive, or launch-stop risk"],
  ["priority/high", "D93F0B", "Core workflow broken without a reasonable workaround"],
  ["priority/normal", "FBCA04", "Confirmed normal-priority work"],
  ["resilience", "D4C5F9", "Failure handling and bounded behavior"],
  ["roadmap", "1D76DB", "Public long-term roadmap track"],
  ["test", "BFDADC", "Automated regression coverage"],
  ["triage/accepted", "0E8A16", "Reproduced and accepted into scope"],
  ["triage/icebox", "C2E0C6", "Valid but outside the active roadmap"],
  ["triage/needs-repro", "E4E669", "Needs a deterministic reproduction"],
]);
