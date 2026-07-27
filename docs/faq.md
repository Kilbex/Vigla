# Frequently asked questions

## What problem is Vigla solving?

Vigla is a local control plane for supervising several AI coding CLI workers as
one mission. It adds cross-vendor coordination, isolated git worktrees,
independent pre-merge audit, a structured completion verdict, event replay,
and mission-level rollback. It does not try to replace the CLIs or models.

## Is this another LLM API wrapper?

No. Vigla launches installed vendor CLIs and normalizes their process output
into canonical events. Vendor credentials remain with those CLIs, and their
network traffic still goes to their providers. Vigla adds no cloud control
plane, product account, model proxy, or billing layer.

## Does Vigla make network requests of its own?

The default build does not. Vendor CLIs still contact their configured model
providers. The optional `EMBEDDINGS=1` build downloads a public FastEmbed model
to a per-user cache on first use and then runs that model locally; an offline or
failed download degrades memory retrieval to BM25. Vigla has no product
telemetry or Vigla-operated backend in either build.

## Why not use a vendor's native multi-agent app?

Vendor-native tools are usually the shortest path when every worker belongs to
one vendor and you want that vendor's workflow. Vigla is for mixed rosters and
for teams that want a neutral merge-governance layer: one authority envelope,
one audit vocabulary, and one mission-level revert path across workers. The
[README comparison](../README.md#how-it-compares) links the current primary
sources and states where a capability is merely undocumented rather than
claiming it cannot exist.

## Does “cross-vendor” include the supervisor?

No. Workers are cross-vendor; supervision is Claude-only. `supervisor_model`
accepts exactly two values — `claude`, which spawns the real Claude CLI, and
`auto`, the scripted mock that launches no vendor process — and
`host_services` rejects anything else with `UnsupportedSupervisorModel`. There
is no experimental non-Claude supervisor to enable. The supervisor playbook is
vendor-neutral in design, but a second real supervisor does not exist until
its end-to-end gate does.

## What happened to Gemini CLI support?

Gemini remains a maintained legacy and enterprise compatibility adapter; it is
not a primary consumer launch path. Antigravity is the current verified Google
worker path in Vigla. See the dated vendor table and upstream notice linked in
the [README](../README.md#vendor-support).

## Won't vendor CLI changes break an orchestrator?

They can break adapters, which is why the boundary is deliberately narrow. A
vendor adapter is a pure crate that translates CLI bytes into canonical
events. Process management, mission policy, and persistence do not depend on a
vendor-specific parser.

All six shipping adapters run through the shared `vigla-adapter-conformance`
harness, but the fixtures behind them are not equally strong, so here is the
exact coverage:

| Adapter | Conformance cases | Fixture suite | What the fixtures are |
|---|---|---|---|
| `claude` | 6 | `claude_simple.jsonl` via `from_fixture.rs` | stream-JSON shapes the parser dispatches on |
| `codex` | 4 | `codex_simple.jsonl` via `from_fixture.rs` | JSONL shapes the parser dispatches on |
| `gemini` | 4 | `happy_path.jsonl` via `from_fixture.rs` | stream-JSON shapes the parser dispatches on |
| `antigravity` | 4 | none | synthetic plain text |
| `kiro` | 4 | none | synthetic plain text |
| `copilot` | 4 | none | synthetic plain text |

Every fixture is synthetic. None is a recording of a real session: they use
`/Users/alice/…` placeholders and contain no tokens, accounts, or repository
names.

The last three wrap `RawLogAdapter` and parse nothing, so there is no vendor
output shape to pin. Their goldens pin the schema-side contract only — the
opening `idle`→`executing` pair, verbatim log passthrough on stdout and
stderr, empty lines emitting nothing, the shared quota detector firing, and
the terminal event synthesised from exit status.

Quota coverage exists for all six but is not uniform, and the goldens record
the difference. `claude` and `gemini` parse the vendor's own rate-limit error,
so their failure event carries category `rate_limit`. The other four —
`codex`, `antigravity`, `kiro`, and `copilot` — surface the quota signal but
not a parsed failure: their failure event is synthesised from the process exit
status and carries category `internal` with `exit_code: 1`. What differs among
those four is only how the signal is detected: `codex` reads a structured
`usage_limit_exceeded` turn, the other three match raw stderr text.

Real-CLI gates catch integration drift that fixtures cannot, and they run
locally on request, never in CI.

## Can Vigla merge unsafe code automatically?

Any tool authorized to modify a repository can cause harm. Vigla reduces that
risk; it cannot erase it. Workers operate in isolated worktrees, the authority
envelope defines scope and risk boundaries, submissions pass through an audit,
and each accepted task integration creates a pre-integration tag. Final merge adds
durable target-branch rollback anchors. Review the
[security model](../SECURITY.md) and use the credential-free harness before
granting real CLIs access to sensitive repositories.

## What happens to Git artifacts when I abort a mission?

Abort stops the mission but deliberately retains its Vigla-owned worktrees,
branches, and intermediate snapshot tags for inspection. The mission's target
branch is not changed. When you no longer need those artifacts, use **Clean up
artifacts** on the aborted mission; Vigla removes only that mission's artifacts
and records completion so the action is safe to retry after an interruption.

## Does the browser replay run an agent?

No. It is the real React Operations Room projected from three committed,
hand-authored canonical event streams (`app/src/demo/recordings.ts`) — a
scripted, deterministic replay, not a capture of a real session. The
timestamps, costs, and test counts in it are authored values. The app surface
is inert, only replay controls are enabled, and the Tauri IPC imports resolve
to in-process browser mocks. It cannot reach a vendor CLI or local
repository.

## Why is there no download button?

The supported distribution path today is a source-built, ad-hoc-signed local
DMG. Vigla publishes no maintainer-built binaries, so there is no external
signing identity, notarization credential, or binary update channel to trust.
Run `./scripts/build.sh`; it verifies the app and disk image and prints a
checksum for your local artifact.

Whether that stays permanent is open. A [Mac App Store
release](./roadmap/mac-app-store.md) is under feasibility investigation, and
it would replace this trust model with Apple's rather than add to it — that
trade has not been decided, and the feasibility answer comes first.

## Is Linux or Windows supported?

The portable Rust workspace is continuously checked, linted, and tested on
Linux. Desktop packaging and platform UX are not supported yet. Their public
specifications are [Linux](./roadmap/linux.md) and
[Windows](./roadmap/windows.md); those documents distinguish proven portable
code from packaging work that remains.

## Can I migrate from vibe-kanban?

There is no database importer, and Vigla is not a kanban-board replacement.
The [migration guide](./migrations/from-vibe-kanban.md) maps the concepts,
shows what changes in the workflow, and gives a low-risk way to evaluate Vigla
without moving existing project data.

## How do I ask for help or report a vulnerability?

Use [`SUPPORT.md`](../SUPPORT.md) to choose Issues, Discussions, or the private
security route. Never put credentials, unredacted agent transcripts, or
private repository content in a public report.
