# Roadmap

Where Vigla is headed. This file is direction, not a contract —
items move based on what operators actually need. Open a GitHub issue
(or comment on an existing one) to influence priorities; the fastest
way to move something up this list is a focused PR.

## Now — v0.1.x (hardening)

The orchestrator, vendor adapters, memory kernel, and operations UI
are implemented. Current focus:

- **Real-CLI coverage.** Keep the Claude / Codex / Antigravity gates green
  against fast-moving vendor CLIs; expand fixture coverage for failure and
  quota paths. Gemini remains a legacy / enterprise compatibility path after
  consumer Login with Google ended on 2026-06-18.
- **Local packaging.** Keep the one-command, ad-hoc-signed `.dmg` build reliable
  on clean Macs without publishing maintainer-built artifacts.
- **First-run polish.** CLI discovery, auth diagnostics, and the
  credential-free mock demo as the default first experience.
- **Regression prevention.** Keep the crate boundaries, canonical event
  contract, browser flows, supply-chain policy, and local package recipe gated
  on every pull request.

## Next — v0.2

- **End-to-end gates for profile-backed vendors.** Antigravity is verified;
  Kiro and GitHub Copilot still need real-CLI integration tests before they
  count as supported.
- **Non-Claude supervisors.** The supervisor playbook is
  vendor-portable in design; verification currently gates on Claude.
  Codex-backed supervision is the first candidate.
- **New vendor adapters.** The adapter boundary (one pure crate per
  vendor: CLI bytes → canonical events) is the designed extension
  point — an OpenCode adapter proposal is already scoped in
  [docs/GOOD_FIRST_ISSUES.md](./docs/GOOD_FIRST_ISSUES.md).
- **Reproducible local updates.** Versioned source tags and a stable local build
  path without a maintainer-operated binary distribution channel.
- **True post-verdict continuation.** The persisted Extend wire shape is kept
  replay-compatible, but the current UI offers only Merge and Discard and the
  runtime rejects Extend. Re-enable it only with a tested supervisor re-entry
  path that cannot strand a mission in an idle `Executing` state.

## Later

- **[Linux](./docs/roadmap/linux.md), then
  [Windows](./docs/roadmap/windows.md).** The orchestrator and adapters are portable
  Rust and the UI is Tauri; the work is packaging, path handling, and
  notification/UX parity — not a rewrite. This is the most-requested
  scope decision; if you want it, say so on the tracking issue so it
  can be prioritized with evidence.
- **Richer memory retrieval.** Embedding-backed retrieval is optional
  today; make hybrid ranking the verified default and surface memory
  provenance in the UI.
- **[Mission templates / playbooks](./docs/roadmap/mission-templates.md).** Reusable mission specs (roster,
  envelope, scope) for recurring jobs like dependency bumps, test
  backfills, and release chores.
- **Deeper audit signals.** Scope and regression scoring beyond test
  pass; per-task acceptance criteria are already in the event schema.
- **[Supervised-fleet benchmark](./docs/roadmap/benchmark.md).** A
  pre-registered, reproducible comparison that publishes losses, cost, and
  uncertainty—not a marketing-only headline number.

## Non-goals

Vigla stays sharp by refusing some scope permanently:

- No LLM API wrapping or provider abstraction layer.
- No in-process agent framework or chat UI.
- No cloud control plane; local-first is a feature, not a phase.
- No per-event approval prompts; the envelope is the contract.
