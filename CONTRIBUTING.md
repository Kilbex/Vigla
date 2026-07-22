# Contributing to Vigla

Vigla is a local-first desktop app for supervising AI coding CLIs as visible workers. Contributions are welcome, especially adapter work that helps new vendor CLIs produce the canonical event stream.

This guide is intentionally lightweight. It explains how to get a change running locally, exactly what the CI gates enforce so you can reproduce them before you push, how to write tests that will be accepted, and where newcomers can make a useful first PR.

## Fast Start

Prerequisites:

- Rust toolchain from `rust-toolchain.toml`
- Node 22.x
- pnpm 10.x
- macOS with Xcode Command Line Tools for the full Tauri desktop app

Setup:

```sh
pnpm install --frozen-lockfile
cargo xtask test   # builds the release mock-harness, then `cargo test --workspace`
cd app && pnpm exec vitest run && pnpm build
```

> **Use `cargo xtask test`, not bare `cargo test --workspace`.** The Tauri host
> bundles the release `mock-harness` binary as a resource, and `tauri_build`
> validates that path on every compile — so `cargo test --workspace` fails from
> a clean checkout until the binary exists. `cargo xtask test` builds it first.
> `cargo xtask help` lists the rest (`build`, `clippy`, `ci`, `receipt`).

Run the app:

```sh
./scripts/dev.sh
```

Build a release bundle:

```sh
./scripts/build.sh
```

## Good First Contributions

Start with [docs/GOOD_FIRST_ISSUES.md](docs/GOOD_FIRST_ISSUES.md). The best first PR is usually an adapter fixture or parser improvement because it is isolated, testable, and teaches the event model quickly.

Adapter-focused first PR checklist:

1. Read [ARCHITECTURE.md](ARCHITECTURE.md), especially the adapter boundary.
2. Pick a task from the newcomer queue.
3. Add or update one fixture under the relevant `crates/adapters/<vendor>/tests/fixtures/` directory when possible.
4. Add a focused adapter test.
5. Run the narrow package test first (e.g. `cargo test -p vigla-adapter-claude`), then `cargo xtask test`.

## Development Rules of Thumb

- Keep UI-host glue thin. Business logic belongs in the `orchestrator` crate so it can be tested without Tauri.
- Keep adapters pure. They translate vendor stdout/stderr lines into canonical events; they do not spawn processes, read files, write files, or persist data.
- Prefer small PRs with one behavioral goal.
- Preserve existing public event shapes unless the PR explicitly changes the event schema and includes migration notes.
- Avoid broad rewrites unless they remove a real maintenance problem and come with tests.

These are guardrails, not a creativity filter. If a better design needs a different path, explain the tradeoff in the PR.

Some of these rules are enforced mechanically, so crossing a boundary fails your
build rather than waiting for review:

- **Vendor routing stays in adapters / profiles.** `vendor_profile.rs`'s
  `routing_leak_check_passes_for_runtime_sources` test (and
  `scripts/check-routing-leaks.sh`) fail if a vendor-specific `Command::new("claude"|"codex"|…)`
  or a vendor CLI flag leaks into orchestrator or host code.
- **SQL stays in the orchestrator.** `crates/orchestrator/tests/no_sql_outside_orchestrator.rs`
  fails if raw SQL appears outside the `orchestrator` crate.

If your change needs to cross one of these seams, that is a design discussion for
the PR, not a test to delete.

### Project docs

Engineering and end-user docs live in the repo: [ARCHITECTURE.md](ARCHITECTURE.md)
(system design), [ROADMAP.md](ROADMAP.md) (public direction),
[docs/](docs/), and the crate guides linked from [crates/README.md](crates/README.md). Internal product and design
planning is intentionally **not** published — if a `docs/` link looks missing,
it is an internal working note, not something you are expected to have. Propose
product-level changes as a GitHub issue rather than a doc PR.

## Continuous Integration

Every pull request runs [`.github/workflows/ci.yml`](.github/workflows/ci.yml).
A PR is not mergeable until the macOS Rust and frontend jobs, the portable Linux
job, and the supply-chain policy are green. You can reproduce every gate locally
before pushing — please do, it keeps review fast.

**Rust job** — in order:

| CI step | Local equivalent |
| --- | --- |
| Toolchain pinned by `rust-toolchain.toml` + `clippy`, `rustfmt` | `rustup component add clippy rustfmt` |
| `cargo fmt --all -- --check` | same |
| Build the release `mock-harness` (Tauri bundle resource) | `cargo xtask build-mock-harness` |
| `cargo check --workspace --all-targets --all-features` | same |
| `cargo clippy --workspace --all-targets -- -D warnings` | `cargo xtask clippy` |
| `cargo clippy --workspace --release --all-targets -- -D warnings` | `cargo xtask clippy --release` |
| `cargo test --workspace` | `cargo xtask test` |
| Regenerate TypeScript bindings and require a clean diff | `VIGLA_REGEN_BINDINGS=1 cargo test -p vigla-host --lib regenerate_typescript_bindings && git diff --exit-code -- app/src/bindings.ts` |
| Regenerate the lock-bound Rust dependency license report and require a clean diff | See “Dependency license updates” below |
| `cargo xtask receipt` | same |
| `cargo audit --deny warnings` | `cargo audit --deny warnings` |

**Portable Linux job** — all workspace crates except the macOS Tauri host:

```sh
cargo check --workspace --all-targets --exclude vigla-host
cargo clippy --workspace --all-targets --exclude vigla-host -- -D warnings
cargo test --workspace --exclude vigla-host
```

**Supply-chain job** — a checksum-pinned Gitleaks binary scans the complete
reachable Git history before `cargo deny --all-features check` enforces the
license, crate-ban, advisory, and registry-source policy in
[`deny.toml`](deny.toml).

**Frontend job** — in `app/`:

| CI step | Local equivalent |
| --- | --- |
| `pnpm install --frozen-lockfile` | same |
| `pnpm audit --audit-level low` | same |
| `pnpm exec vitest run` | same |
| `pnpm build` | same |
| `pnpm build:webdemo` | same |
| `pnpm site:check` | same |

The same job also runs `node --test scripts/*.test.mjs` and
`node scripts/check-links.mjs` from the repository root.

### Dependency license updates

`THIRD_PARTY_NOTICES.txt` and the web/Tauri copies are generated files. After
changing `Cargo.lock`, regenerate the sanitized Rust report with the same
pinned `cargo-about` version as CI, then refresh the combined distribution:

```sh
cargo install --locked --features cli cargo-about --version 0.9.1
cargo about generate --workspace --all-features --locked --fail \
  --format json --output-file /tmp/vigla-rust-licenses.json
node scripts/generate-license-notices.mjs \
  --write-rust-report /tmp/vigla-rust-licenses.json
pnpm -C app licenses:write
```

After a production JavaScript dependency change, `pnpm -C app licenses:write`
is sufficient. If an `ort-sys` update selects a different ONNX Runtime release,
update the pinned sources and checksums in
[`scripts/retained-license-sources.mjs`](scripts/retained-license-sources.mjs),
then fetch and verify the official tagged legal files before regenerating:

```sh
node scripts/retained-license-sources.mjs --write
node scripts/retained-license-sources.mjs --check
```

The normal script tests and app build perform the checksum verification offline
and reject stale inventories, missing license text, Cargo lock drift, native
runtime version drift, or a generated report containing a local filesystem
path. The Rust report also retains `NOTICE`, `COPYRIGHT`, and `PATENTS` files,
explicit third-party attributions, and both top-level and nested license files
shipped inside dependency source archives, including vendored native libraries.
When an archive omits a workspace license, the report uses a checksum-pinned
file from the crate's exact recorded upstream revision. Generation fails if a
generic SPDX template package lacks that attribution; when upstream publishes
no copyright line at all, the inventory preserves the exact package metadata
and explicitly records the omission instead of inventing an owner.

**Browser E2E job** — installs Chromium, Firefox, and WebKit on Linux, then
runs the desktop UI contract, recorded replay, and GitHub Pages surface:

```sh
pnpm -C app run e2e
pnpm -C app run e2e:webdemo
pnpm -C app run e2e:site
```

The landing suite asserts a dependency-free first load, explicit playback of
the motion asset, responsive layout, reduced-motion behavior, metadata, and no
vendor or third-party requests.

**Package smoke** — changes to the packaging, host, crates, or lockfiles also
run [`.github/workflows/package-smoke.yml`](.github/workflows/package-smoke.yml).
It builds and verifies the local ad-hoc-signed DMG on macOS and deliberately
uploads no artifact.

Warnings are errors: clippy runs `-D warnings` in **both** debug and release
(release surfaces lints debug hides), so a warning fails CI. The one-shot local
gate is:

```sh
cargo fmt --all -- --check
cargo xtask ci          # = cargo xtask test + cargo xtask clippy (debug, -D warnings)
VIGLA_REGEN_BINDINGS=1 cargo test -p vigla-host --lib regenerate_typescript_bindings
git diff --exit-code -- app/src/bindings.ts
cargo xtask receipt     # fixed recovery case set + exact-SHA mission revert
cargo xtask clippy --release
cargo audit --deny warnings
cargo deny --all-features check
gitleaks git --redact --no-banner
node scripts/scan-publishable-tree.mjs
pnpm audit --audit-level low
node --test scripts/*.test.mjs
node scripts/check-links.mjs
cd app
pnpm exec playwright install chromium firefox webkit
pnpm exec vitest run
pnpm build
pnpm build:webdemo
pnpm site:check
pnpm e2e
pnpm e2e:webdemo
pnpm e2e:site
```

Run `cargo fmt --all` before you push and keep formatting changes scoped to your
work. CI checks formatting and never rewrites it.

## Writing Tests

Tests are how a change earns trust here — the orchestrator is the merge-safety
layer, so "it works on my machine" is not enough. Requirements:

- **Every bug fix and behavior change ships with a test that fails before the fix
  and passes after** (RED → GREEN). Name it after what it guards; a one-line
  comment pointing at the bug it locks in is welcome. Pure refactors that change
  no behavior don't need a new test, but must keep the existing suite green.
- **Test at the lowest layer that reproduces the behavior:**
  - *Unit* — in-module `#[cfg(test)] mod tests` for pure logic (parsers, scoring,
    scanners).
  - *Crate integration* — `crates/<crate>/tests/*.rs` for cross-module or
    DB-backed behavior (migrations, persistence, recovery, e2e mission flows).
  - *Adapter golden fixtures* — prefer a fixture over hand-built input: drop a
    captured CLI transcript under `crates/adapters/<vendor>/tests/fixtures/` and
    assert the canonical events it produces. This is the highest-value, lowest-risk
    kind of PR.
  - *Frontend* — Vitest (`app/src/**/__tests__` or `*.test.tsx`) for store and
    component logic; Playwright under `tests/e2e/` for user-visible flows.
- **Keep tests deterministic.** No dependence on wall-clock, randomness, network,
  ambient environment, or hash-map iteration order. Inject time (see the crates'
  `time.rs` seams), seed inputs, and use fixtures; the conformance harness even
  redacts the `ts` envelope field so goldens stay stable. A flaky test is a failing
  test.
- **Keep assertions locale- and OS-independent.** Assert on accessibility
  identifiers and structured values, not on rendered localized strings.
- **Property tests for invariants.** Where a function has an invariant over a wide
  input space (the memory scanner and scoring do), add a `proptest` case rather
  than only a handful of examples.
- **Real-CLI tests are opt-in and never gate CI.** Tests that shell out to an
  installed vendor CLI are `#[ignore]`d and require local credentials, so they
  run only when you ask:

  ```sh
  cargo test -p vigla-orchestrator --test real_claude_gate -- --ignored --nocapture
  cargo test -p vigla-orchestrator --test real_codex_run   -- --ignored --nocapture
  cargo test -p vigla-orchestrator --test real_antigravity_run -- --ignored --nocapture --test-threads=1
  ```

  Never make a first-time contributor need real vendor credentials —
  the deterministic `mock-harness` is the default path for reproducing behavior.

Run the narrow package test first for a fast loop (e.g.
`cargo test -p vigla-orchestrator memory::scanner`), then `cargo xtask test`
for the full workspace before you open the PR.

## Dependency & Security Audit

Vigla is local-first with a deliberately small dependency surface. New
dependencies raise the audit and supply-chain surface, so they get scrutiny:

- **Justify a new dependency in the PR.** Prefer the standard library or a crate
  already in the tree. Adapters must stay pure (no process/file/network/DB deps),
  and `event-schema` stays minimal on purpose.
- **`cargo audit --deny warnings` must pass** — CI runs it and it hard-fails on any
  advisory (vulnerability *or* unmaintained) in the dependency tree. Install it
  with `cargo install --locked cargo-audit`.
- **`cargo deny --all-features check` must pass** — [`deny.toml`](deny.toml) rejects
  unapproved licenses, registry or git sources, wildcard registry versions, and
  new advisories. Development dependencies are included in the license check.
- **`pnpm audit --audit-level low` must pass** — the locked frontend and
  JavaScript tooling graph is not exempt from the same zero-known-advisory floor.
- **The only tolerated advisories live in [`.cargo/audit.toml`](.cargo/audit.toml)**,
  a curated ignore-list of *known, currently-unfixable upstream* advisories. Every
  entry carries a rationale and a re-evaluation trigger (usually "clear when
  `<dep>` bumps"). Do **not** add an entry to silence an advisory you can fix by
  bumping a dependency — bump it. A new advisory on a dependency you introduce is
  expected to hard-fail; that is the gate working.
- **Keep lockfiles honest.** Commit `Cargo.lock`, and keep `pnpm-lock.yaml` in sync
  (`pnpm install` updates it). CI installs with `--locked` / `--frozen-lockfile`,
  so a stale lockfile fails the build.
- **Dependabot** ([`.github/dependabot.yml`](.github/dependabot.yml)) opens weekly
  Cargo and npm PRs and monthly GitHub-Actions PRs; reviewing those is a genuinely
  useful contribution.
- **Never add a real secret to source, fixtures, logs, or tests.** The memory kernel
  scans proposals before they become durable notes and stores only a redacted
  rejection preview (see [SECURITY.md](SECURITY.md#security-model)). If your change
  touches that path, do not weaken it, and add a test proving a planted secret
  does not reach a memory proposal or note. Canonical worker logs intentionally
  preserve local vendor output for audit/replay, so treat captured transcripts and
  local databases as sensitive and redact them before sharing.

## Pull Request Workflow

1. Search existing issues and pull requests, then open or claim an issue for
   changes whose design or scope needs discussion.
2. Branch from current `main`; keep one behavioral goal per branch. Use a short
   descriptive name such as `fix/quota-retry` or `docs/adapter-guide`.
3. Make the smallest complete change, add the narrowest regression test, and run
   that test before the full local gates above.
4. Open a draft pull request early when feedback would prevent rework. Fill in
   every applicable section of the template; use “not run” with a reason rather
   than leaving verification ambiguous.
5. Mark it ready only when the branch is rebased or merged with current `main`,
   all required checks pass, user-visible changes have screenshots or clips, and
   no review thread is unresolved.
6. A code owner reviews security and automation surfaces. Maintainers squash
   merge accepted pull requests so `main` stays bisectable and each commit
   describes one completed change.

Do not weaken a gate to make a pull request pass. If a gate is wrong, fix the
gate in a separate, explained change with evidence.

## Pull Request Shape

A useful PR usually includes:

- What changed and why
- How it was tested
- Any user-visible behavior changes
- Screenshots or short clips for UI changes
- Notes about follow-up work, if intentionally deferred

It is fine to open a draft PR early for design feedback.

## Reporting Security Issues

Please do not file public issues for vulnerabilities. Use GitHub's private vulnerability reporting as described in [SECURITY.md](SECURITY.md).
