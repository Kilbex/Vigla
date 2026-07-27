# Security Policy

Vigla runs untrusted AI coding CLIs as workers and derives durable state
(events, memory) from their output. That makes two things security-relevant by
design: the boundary between an agent's output and what Vigla persists, and the
dependency supply chain. This document is the disclosure channel and a summary of
the protections and recent hardening on that boundary.

## Reporting a Vulnerability

If you discover a security issue in Vigla, please **do not** file a public
issue.

1. Use GitHub's [private vulnerability reporting][gh-pvr] on this repo (the
   **Security** tab → **Report a vulnerability**).
2. We will acknowledge receipt within 5 business days and provide an initial
   assessment within 10 business days.
3. Once a fix is ready, we coordinate disclosure with you and credit you in the
   release notes if you wish.

Please include a reproduction (a minimal note body, event fixture, or CLI
transcript is ideal) and the impact you observed.

If the private-report button is unavailable, **do not post vulnerability
details**. Open a minimal public issue titled `Private security channel
unavailable` with no technical details so a maintainer can restore the private
route, then wait for that route before disclosing. Private vulnerability
reporting is a release prerequisite, not an optional repository setting.

[gh-pvr]: https://github.com/Kilbex/Vigla/security/advisories/new

## Supported Versions

Vigla is pre-1.0; only the latest commit on `main` is supported. Security fixes
are not backported to older snapshots.

## Security model

Vigla is local-first: there is no cloud control plane, Vigla-operated server,
account, or product telemetry. The default build originates no network traffic
of its own; installed vendor CLIs still communicate with their configured
providers. A build made with `EMBEDDINGS=1` additionally lets FastEmbed download
its public model artifacts into a per-user cache on first use, after which
inference is local. Download failure disables embeddings and falls back to
local BM25 rather than blocking a mission. The trust boundaries that matter
most are local:

- **Agent output is untrusted input.** Worker CLIs are driven by LLMs and can
  emit anything — including content that tries to steer the supervisor, and
  including secrets the agent happened to read or print. Vigla treats every
  worker-authored byte that way.
- **Persistence is the sensitive sink.** Events and memory notes are written to a
  local SQLite store and can later be replayed, retrieved, and surfaced in the UI.
  Anything that reaches that store outlives the run, so the persist path is where
  leakage matters most. Canonical worker logs preserve vendor output for replay;
  they are not passed through the memory-note scanner described below.

### Credential-leakage protection

Because durable memory is distilled from untrusted agent output, an agent that
prints an API key, token, or private key could otherwise promote it into a
long-lived note and have it resurface in future retrieval. Vigla prevents that
memory-specific path with a **pre-persistence secret scanner** in the memory kernel
(`crates/orchestrator/src/memory/scanner.rs`), wired into both proposal and pin
paths (`memory/kernel/proposal.rs`, `memory/kernel/pin.rs`) so it runs **before**
a `MemoryProposed` record — or any note body — is written.

The scanner is runtime-free (no network, no external service) and combines:

- **Fixed credential patterns** — AWS access keys, GitHub classic and
  fine-grained PATs, OpenAI (`sk-…`) and Anthropic (`sk-ant-…`) keys, and PEM
  private-key blocks.
- **A Shannon-entropy floor** over sliding windows, to catch opaque
  base64-shaped secrets that match no fixed prefix.
- **A long contiguous-hex detector**, to catch hex-encoded keys, HMACs, and
  digests used as bearer tokens (pure hex never reaches the entropy floor, so
  this is a separate length-based rule).

On a hit, the raw memory body is **dropped** — it never enters a memory proposal
or note record. The only downstream artifact is a redacted preview in which the secret span is replaced by
a `[REDACTED:<len>:<blake3-prefix>]` marker (the fingerprint identifies a repeated
secret without revealing it), and a `MemoryProposalRejected{ reason: Secret }`
event is emitted for auditability.

### Hardening in this cycle

The most recent security audit round focused on that scanner and closed the
following, each with a regression test in `scanner.rs`:

- **Whole-span redaction, not just the triggering window.** A secret longer than
  the 20-character entropy window is redacted in full; the detector no longer
  leaves the bytes outside the first matching window in the stored preview.
  Regression: `long_entropy_secret_is_fully_redacted_not_just_the_window`.
- **Fragile redaction-offset parameter removed.** The pattern matcher previously
  took a caller-supplied prefix length validated only by a `debug_assert`, which
  is compiled out of release builds — a future mismatch could have computed a
  wrong redaction span (a miss, or a partial leak) in production with no error.
  The length is now derived internally, so release and debug behave identically.
  Regression: `aws_key_detection_after_find_anchored_refactor`.
- **Entropy threshold documentation corrected.** The module documented a
  4.5 bits/char threshold that is mathematically unreachable for a 20-character
  window (the cap is `log2(20) ≈ 4.32`); the enforced value was always the more
  aggressive 4.0. The docs now match the code, so operators tuning the detector
  are not misled into loosening it.
- **Oversize-with-secret still redacts.** A note body that is both oversized and
  contains a secret is rejected **as a secret** — it is never stored with a raw
  truncated preview. Regression:
  `oversize_body_with_secret_is_rejected_as_secret_and_not_leaked`
  (`memory/kernel/mod.rs`).

This scanner is not a data-lossy filter for the canonical worker-event log.
Vigla intentionally retains adapter-normalized logs and terminal output locally
so operators can audit and replay a mission. A vendor CLI that prints a secret can
therefore put that secret in the local event database or application log. Treat
`vigla.sqlite`, its SQLite sidecars, and `~/Library/Logs/Vigla/` as sensitive;
do not attach them to public issues without reviewing and redacting them. If raw
vendor output reaches a remote service other than the configured vendor CLI, or
if a secret bypasses the scanner and is promoted into durable memory, report it
through the private channel above.

### Related boundary hardening

- **Desktop content policy.** The production Tauri webview enables a restrictive
  Content Security Policy: bundled assets only, no forms or embedded objects,
  and network connections limited to Tauri IPC. The Vite websocket allowance
  lives only in the separate development policy (`app/src-tauri/tauri.conf.json`).
- **Host-owned export authority.** Mind-map export opens the native save dialog
  in the Rust host; the renderer can suggest a filename but cannot submit an
  arbitrary destination path. Unused renderer-path playbook file commands are
  not registered on the IPC surface (`app/src-tauri/src/lib.rs`).
- **Prompt-injection fencing.** Worker-authored summaries and committed-diff
  excerpts fed into the supervisor's review prompt are independently bounded,
  fenced, and stripped of injected close-markers, so worker content cannot pose
  as supervisor instructions (`mission_supervisor_run/run_task.rs`).
- **Argument-injection guard.** A `vendor:-flag`-shaped worker/model selection is
  rejected before it can reach a vendor CLI as an option
  (`mission_supervisor_run/worker_pass.rs`).
- **Architectural fitness tests.** `routing_leak_check_passes_for_runtime_sources`
  and `no_sql_outside_orchestrator.rs` fail the build if vendor CLI launching or
  raw SQL escapes its designated layer.

## Dependency security

Vigla keeps a small dependency surface and audits it in CI:

- `cargo audit --deny warnings` runs on every PR and hard-fails on any advisory
  (vulnerability or unmaintained) in the dependency tree.
- `cargo deny --all-features check` independently enforces allowed licenses, crate bans,
  advisory exceptions, and registry sources against production and development
  dependencies. The policy is reviewed in [`deny.toml`](deny.toml).
- `pnpm audit --audit-level low` blocks known advisories in the locked frontend
  and JavaScript tooling dependency graph.
- Gitleaks scans the complete reachable Git history in CI using a
  checksum-pinned binary. This is a credential-pattern backstop, not a
  substitute for reviewing author identity, captured telemetry, or image
  metadata before publication.
- The only tolerated advisories are the curated, individually-justified entries
  in [`.cargo/audit.toml`](.cargo/audit.toml). Each entry names the crate, the
  dependent that pulls it in, whether it is compiled for the shipped macOS
  target, and a re-evaluation trigger. See
  [the breakdown below](#tolerated-advisories) — the 21 entries are not one
  homogeneous class, and it would be false to describe them all as
  "unexercised platform paths".
- [Dependabot](.github/dependabot.yml) proposes weekly Cargo/npm and monthly
  Actions updates so the tree does not drift.
- Third-party GitHub Actions are pinned to immutable commit SHAs, with the
  corresponding release tag recorded inline for reviewability.

New advisories on newly-introduced dependencies are expected to fail the build;
that is the gate working as intended.

### Tolerated advisories

`.cargo/audit.toml` holds 21 ignore entries. They fall into three classes with
genuinely different risk, and conflating them would overstate the guarantee.
The counts below are advisory IDs, not affected crate copies — where two
versions of one crate are reached by different edges, the advisory is filed
under the class that carries the higher risk.
Membership was determined with
`cargo tree -i <crate> --target aarch64-apple-darwin --all-features`,
cross-checked against `--target all`.

**1 — Not in any resolved dependency graph (1 entry).**
`RUSTSEC-2023-0071` (rsa 0.9.10, Marvin timing side-channel). `cargo tree -i
rsa --target all --all-features` prints nothing. The workspace enables only
sqlx's `runtime-tokio`, `sqlite`, `macros`, and `migrate` features, so
`sqlx-mysql` — the only path to rsa — stays an inactive optional dependency.
rsa appears in `Cargo.lock` because a lockfile records the union of optional
dependencies, not the resolved graph. Nothing links it.

**2 — Compiled for a platform Vigla does not ship (12 entries).** These are
reached only through Linux or Windows dependency edges of Tauri and are absent
from the `aarch64-apple-darwin` tree: `RUSTSEC-2024-0411` through
`RUSTSEC-2024-0420` (gtk-rs GTK3 bindings), `RUSTSEC-2024-0429` (glib 0.18.5
`VariantStrIter` unsoundness), and `RUSTSEC-2024-0370` (proc-macro-error 1.0.4,
via `glib-macros` and `gtk3-macros`) — all through Tauri's GTK/WebKit Linux
stack. The Windows-only copy of quick-xml (0.37.5, via
`tauri-winrt-notification` ← `notify-rust` ← `tauri-plugin-notification`) is
also not compiled on macOS, but its two advisories are filed under class 3
because a macOS-compiled copy of the same crate carries them. **If Linux ever
becomes a shipped target, these need real triage, not a renewed ignore.**

**3 — Compiled into the shipped macOS binary (8 entries).** These are the ones
the earlier wording got wrong. They are tolerated on the merits of the specific
code path, not because they are absent:

| Advisory | Crate | Reached via | Why it is tolerated |
|---|---|---|---|
| `RUSTSEC-2026-0194`, `RUSTSEC-2026-0195` | quick-xml 0.39.3 | `plist` ← `tauri-utils` / `tauri-codegen` / `tauri-plugin` / `tauri`, as both a build and a normal dependency | Real high-severity (7.5) XML denial-of-service vulnerabilities against attacker-supplied documents. This copy's only input is the application's own `Info.plist`, emitted by our build and read back from the signed bundle — never third-party XML. Fixed in quick-xml ≥ 0.41.0, but the dependents require `^0.37` and `^0.39.2`, so no semver-compatible bump exists. |
| `RUSTSEC-2024-0436` | paste 1.0.15 | `specta` (a dependency of essentially every Vigla crate), plus `tokenizers` / `rav1e` under the optional embeddings backend | Unmaintained-crate advisory, no known vulnerability. Proc-macro-only: it runs at build time and emits no runtime surface. |
| `RUSTSEC-2025-0075`, `-0080`, `-0081`, `-0098`, `-0100` | unic-char-range, unic-common, unic-char-property, unic-ucd-version, unic-ucd-ident (all 0.9.0) | `urlpattern` ← `tauri-utils` ← `tauri`, used to match Tauri capability URL patterns; both a build and a normal dependency | Unmaintained-crate advisories, no known vulnerability. Static Unicode identifier tables — no I/O, no unsafe parsing. |

Across all three classes, only **3** of the 21 IDs are vulnerabilities
(`RUSTSEC-2023-0071`, `RUSTSEC-2026-0194`, `RUSTSEC-2026-0195`); the other
**18** are unmaintained-crate or unsoundness warnings. That distinction matters
for how an entry clears: a vulnerability clears when upstream ships a fix and a
semver-compatible bump exists, but an unmaintained-crate advisory has no fix to
wait for — it clears only when the dependent stops using the crate. Every entry
records its own trigger.

## Verifying a local build

Vigla distributes no maintainer-built binaries, signatures, or checksums today.
`./scripts/build.sh` creates one local, ad-hoc-signed DMG and prints its SHA-256
digest. Keep that terminal output and verify the file before moving or sharing
it:

```sh
shasum -a 256 target/release/bundle/dmg/*.dmg
hdiutil verify target/release/bundle/dmg/*.dmg
```

To inspect the application after mounting the DMG, run
`codesign --verify --deep --strict /Volumes/Vigla/Vigla.app` and
`codesign -dv --verbose=4 /Volumes/Vigla/Vigla.app`. The latter should report
`Signature=adhoc` and `TeamIdentifier=not set`.

As long as this section says so, a downloaded Vigla application or a checksum
presented as maintainer-issued is not an official release. A signed
distribution channel would require changing this document first; a
[Mac App Store release](docs/roadmap/mac-app-store.md) is under feasibility
investigation and would make that change, but it is not decided. Treat this
page, not a third party, as the authority on what an official artifact is.
