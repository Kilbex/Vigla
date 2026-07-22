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

Workers are cross-vendor today. Production supervisor execution is
Claude-backed and explicitly listed as a current limitation. The supervisor
playbook is vendor-neutral in design, but a different supervisor does not
count as supported until its end-to-end gate exists.

## What happened to Gemini CLI support?

Gemini remains a maintained legacy and enterprise compatibility adapter; it is
not a primary consumer launch path. Antigravity is the current verified Google
worker path in Vigla. See the dated vendor table and upstream notice linked in
the [README](../README.md#vendor-support).

## Won't vendor CLI changes break an orchestrator?

They can break adapters, which is why the boundary is deliberately narrow. A
vendor adapter is a pure crate that translates CLI bytes into canonical events
and is pinned by redacted golden fixtures plus a shared conformance suite.
Process management, mission policy, and persistence do not depend on a
vendor-specific parser. Real-CLI gates catch integration drift that fixtures
cannot.

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
recorded canonical event streams. The app surface is inert, only replay
controls are enabled, and the Tauri IPC imports resolve to in-process browser
mocks. It cannot reach a vendor CLI or local repository.

## Why is there no download button?

The supported distribution path is a source-built, ad-hoc-signed local DMG.
Vigla does not publish maintainer-built binaries, so there is no external
signing identity, notarization credential, or binary update channel to trust.
Run `./scripts/build.sh`; it verifies the app and disk image and prints a
checksum for your local artifact.

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
