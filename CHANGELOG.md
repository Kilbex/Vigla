# Changelog

All notable changes to Vigla are documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).
Pre-1.0, minor versions may contain breaking changes.

Vigla is built from source; there is no maintainer-published binary, so
a version is the state of `main` at the commit carrying that version in
`Cargo.toml`. 0.1.0 shipped untagged when the repository was opened;
from 0.1.1 on, each release is marked with a `vX.Y.Z` git tag.

## [Unreleased]

### Added

- Latest Claude and Codex model choices in the Deploy panel, including
  Claude Opus 4.8 with 1M context and Fable 5.
- A credential-free demo path that a fresh checkout can actually run: a
  **Mock (demo)** supervisor option submitting `supervisor_model:
  "auto"`. The scripted supervisor spends nothing and calls no vendor
  CLI, but the mission is real — branches, worktrees, and commits land
  in the selected folder. The default is still Claude.
- Adapter conformance and golden tests for Antigravity, Kiro, Copilot,
  Codex, and Gemini. The suite previously covered Claude only.
- A maintainer-dispatched **Regenerate notices** workflow, so a
  Dependabot branch that no one can push to is no longer deadlocked by
  the lock-bound dependency license report.
- Per-entry justification in `.cargo/audit.toml` recording which
  advisories are actually compiled into the shipped macOS binary.
- Open-source release hardening: `GOVERNANCE.md`, generated
  `THIRD_PARTY_NOTICES` for both dependency graphs with retained
  upstream legal files, a checksum-pinned secret scan over the complete
  reachable history, and `cargo deny` license, ban, advisory, and
  registry-source policy.
- Rust documentation gates: `missing_docs` is now denied in
  `vigla-event-schema` and `vigla-adapter-core`, and CI runs a rustdoc
  completeness gate over the whole workspace.
- MSRV enforcement. The Linux job now compiles the whole portable
  workspace on the exact `rust-version` declared in `Cargo.toml`, read
  from the manifest rather than hardcoded, so the minimum is proven and
  not merely asserted. Contract tests pin `cargo-deny`'s `rust-version`
  to that same value, require the toolchain pin to be no older than it,
  and hold the published install prerequisites to the pinned toolchain.
- OpenSSF Scorecard analysis on `main`, weekly, and on branch-protection
  changes. It writes to a build artifact; publishing the score to the
  public dataset stays an explicit maintainer decision.

### Changed

- Database location is now resolved per platform instead of always
  producing a macOS path: `~/Library/Application Support/Vigla` on
  macOS (unchanged), `$XDG_DATA_HOME/vigla` on Linux, `%APPDATA%\Vigla`
  on Windows. `VIGLA_DB_PATH` still overrides all of them.
- The supervisor playbook lists every dispatchable vendor, pinned by an
  exhaustive `Vendor` match, instead of a hand-maintained subset.
- `ROADMAP.md` points at label-filtered issue views rather than an
  unnamed "tracking issue".

### Fixed

- Staged revert picked the wrong branch point: `refs/vigla/base/<mid>/0`
  is not the pre-merge anchor, because `integration_index` is assigned
  at spawn while integration is serialised first-come.
- Plan-mode review was silently ignored under the mock supervisor.
- Mission team summaries credited CLIs that never ran.
- Documentation that contradicted the code it describes: the landing
  page declared the evidence screenshot at 1600×1000 (it is 1280×800)
  and the hero preview as 15 seconds (15.9); the FAQ claimed the Codex
  adapter emits no failure event on quota exhaustion (its golden emits
  one, category `internal`); `CONTRIBUTING.md` omitted two Rust gates,
  mislocated two Frontend ones, and named four of the required checks
  instead of all of them; the launch checklist asked for a required
  check named after a workflow rather than its job; `ARCHITECTURE.md`
  gave the mission state machine a `Paused { vendor }` field that does
  not exist and summarised the secret scanner's window as its
  bits-per-character threshold.
- The README's real-CLI gate instructions: the `supervisor_live` command
  was missing `VIGLA_LIVE=1`, so it skipped every test and passed, and
  the gate does not use `tests/samples/sandbox/` as claimed. The mock
  walkthrough sent readers to the review queue for a decision that is
  taken in the mission overlay.

## [0.1.0] - 2026-07-21

Initial public release.

### Added

- Mission supervision: supervisor-driven decomposition, arbiter-bounded
  Accept / Extend / Scrub / Escalate, structured completion verdicts.
- Cross-vendor workers: Claude Code, Codex CLI, Antigravity, legacy
  Gemini CLI, and profile-backed Kiro / Copilot. Claude, Codex,
  Antigravity, and Gemini have opt-in local real-CLI gates (`#[ignore]`d;
  no CI workflow runs them).
- Mission supervision is Claude-only: `supervisor_model` accepts
  `claude` or the scripted `auto` mock and rejects everything else.
- A deterministic mock harness for credential-free demos: seven
  single-worker scripts driven by a canonical event stream.
- Local-only macOS packaging: one-command, ad-hoc-signed DMG builds with
  no maintainer release artifact or personal signing identity.
- Daily GitHub traffic snapshots to a private metrics archive, with
  overlapping 14-day windows deduplicated in generated reports.
- Reversibility: worktree-per-worker isolation, pre-merge snapshot tags,
  one-click mission revert.
- Memory Kernel: local event-sourced repository memory with witness
  scoring, promotion policy, and vendor-native file rendering.
- Operations Room UI: station canvas, worker drawer, review queue,
  mission inbox, history, and replay.
- Plan review workspace: a wider, accessible execution map, four-bound
  envelope readout, progressive plan details, and clearer approval
  controls.
- Defensive plan projection: sanitized model-authored labels and bounded
  graph size before layout.
