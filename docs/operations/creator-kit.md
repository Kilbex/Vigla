# Vigla creator and reviewer kit

This kit is for an independent technical review, recording, or live demo. It
uses public, deterministic inputs so nobody needs a vendor account, API key, or
private repository. Please show the limitations alongside the interesting
parts; the evidence is stronger when its boundary is visible.

## Fastest path: no install

Open the [read-only browser replay](https://kilbex.github.io/Vigla/demo/). It
runs the real Operations Room UI against three recorded canonical event streams:

1. **Accepted** — a cross-vendor fleet completes and passes audit.
2. **Bound tripped** — a worker crosses the configured authority envelope.
3. **Quota paused** — the mission pauses instead of spinning through retries.

Play, pause, step, scrub, and change speed from the replay controls. The banner
identifies this as recorded data. This surface cannot spawn a process, modify a
repository, use a credential, or call a vendor service.

## Ten-minute review script

| Time | Show | What to establish |
| --- | --- | --- |
| 0:00 | Landing page and launch receipt | Vigla supervises the merge across coding-agent CLIs; it is not another model or API wrapper. |
| 0:45 | **Accepted** browser recording | Point out named workers, isolated tasks, canonical events, progress, tests, spend, and the review queue. |
| 2:30 | Pause, step, and scrub | Replay is a product primitive backed by persisted events, not a video-only mockup. |
| 3:15 | **Bound tripped** recording | The operator sets scope, reversibility, risk, and quality bounds; Vigla escalates when one trips. |
| 4:30 | **Quota paused** recording | Quota exhaustion pauses work rather than spending the remaining retry budget. |
| 5:30 | [Architecture](../../ARCHITECTURE.md) | Vendor adapters are pure byte-to-event translators; process, git, persistence, and policy remain outside them. |
| 6:30 | [Recovery receipt](../evidence/recovery-receipt.md) | Explain the fixed case set, reproduce command, denominator, and exclusions before quoting 27/27. |
| 7:30 | Local mock harness or screenshots | Show plan review, the live fleet, and the completion inbox without vendor credentials. |
| 8:45 | Revert path and known limitations | Whole-mission rollback reverts the recorded target merge while preserving later commits; current desktop and supervisor boundaries are explicit below. |
| 9:30 | Source build and contribution paths | Build locally, inspect the tests, or pick a scoped public roadmap issue. No maintainer binary is offered. |

## Run the desktop mock locally

Requirements: macOS 12+, Rust 1.95, Node 22, pnpm 10, and Xcode Command
Line Tools.

```sh
git clone https://github.com/Kilbex/Vigla.git
cd Vigla
pnpm install --frozen-lockfile
./scripts/dev.sh
```

Use a bundled mock scenario from the Deploy panel. The fixture repository used
by the real-CLI gates is also public at [`tests/samples/sandbox/`](../../tests/samples/sandbox/);
it contains a deliberately failing Rust function and no proprietary content.

## Claims that are safe to quote

- Vigla is Apache-2.0, local-first, and adds no product telemetry or cloud
  control plane.
- Claude Code, Codex CLI, and Antigravity have opt-in real-CLI integration gates.
  Gemini CLI remains a legacy/enterprise compatibility path; Kiro and GitHub
  Copilot end-to-end verification is still pending.
- The desktop app is macOS-only today. Portable Rust crates build, lint, and test
  on Linux CI; Linux and Windows desktop support remain public roadmap work.
- The launch receipt covers **27 fixed, seeded failure trajectories**, all of
  which escalated within the default retry bounds. It is not a production
  success rate, a model benchmark, or evidence that every failure mode exists in
  the case set.
- Vigla does not publish maintainer-built application binaries. The supported
  path produces and verifies an ad-hoc-signed DMG on the user's Mac.

Do not describe the recorded browser runs as live vendor calls, the 27 cases as
27 autonomous missions, Kiro or Copilot as end-to-end verified, non-Claude
supervision as production-gated, or Linux/Windows as supported desktops.

## Media and B-roll

| Asset | Use | Notes |
| --- | --- | --- |
| [`vigla-demo.webp`](../media/vigla-demo.webp) | Motion overview / article embed | 1280×800, ≤20 s loop, deterministic recorded events, no audio |
| [`ops-room.png`](../media/ops-room.png) | Fleet overview | Claude, Codex, Antigravity, and Copilot mock workers |
| [`plan-review.png`](../media/plan-review.png) | Authority-envelope explanation | Proposed plan and bound fit before execution |
| [`mission-inbox.png`](../media/mission-inbox.png) | Audit and recovery close-up | Structured verdict, residual risk, and revert control |
| [`social-preview.png`](../media/social-preview.png) | Link card / title card | Exact 1280×640 crop |

The capture inputs are fictional and contain no tokens, user paths, private
prompts, or vendor traffic. Regenerate stills with
`scripts/capture-readme-media.cjs` and motion with
`scripts/capture-web-demo.cjs`. Preserve the product name and link back to the
canonical repository when editing the assets.

## Evidence and follow-up

- [Source and build instructions](../../README.md#build-a-local-dmg)
- [Recovery method and machine-readable result](../evidence/recovery-receipt.md)
- [Contributor workflow](../../CONTRIBUTING.md)
- [Known limitations](../../README.md#known-limitations)
- [Public roadmap](../../ROADMAP.md)
- [Support and disclosure routes](../../SUPPORT.md)

For technical questions that other users could reuse, open a GitHub Discussion.
For a reproducible defect, use the issue form. Never put a vulnerability,
credential, or private transcript in either place; follow `SECURITY.md` instead.
