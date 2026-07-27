# GitHub launch controls

Repository files can define workflows, templates, ownership, labels, and issue
content, but several launch controls require repository-owner or personal
account access. Complete this checklist after the launch commit is on `main`
and before announcements.

## Publication preflight

Before the launch commit leaves the workstation:

- Run `gitleaks git --redact --no-banner` and
  `node scripts/scan-publishable-tree.mjs`, then manually review images,
  fixtures, absolute paths, URLs, and author metadata. The helper scans exactly
  tracked plus non-ignored untracked files in a temporary tree, so ignored
  build caches and private notes are neither published nor needlessly scanned.
  A zero secret-detector result does not clear personal identity or captured
  account telemetry.
- Inspect every publishable commit with
  `git log --all --format='%h %an <%ae>'`. Configure a repository-local public
  alias and GitHub noreply address before creating the final commits. If old
  commits contain private identity or captured session data, rewrite or
  recreate the publishable history, re-scan it, and update the remote before
  launch; `.mailmap` does not remove raw commit metadata.
- Run the complete gate in [CONTRIBUTING.md](../../CONTRIBUTING.md), inspect the
  intended release diff, and confirm ignored private directories are absent
  from `git ls-files`.
- Enable private vulnerability reporting before publishing `SECURITY.md` as an
  accepted disclosure route. If it cannot be enabled, publication is blocked.

## Automated owner step

Preview, then apply the sequentially idempotent metadata/label/issue
bootstrap. Run it from one owner shell at a time; GitHub does not offer an
atomic “create issue only if this title is absent” operation, so concurrent
bootstrap invocations can race:

```sh
node scripts/bootstrap-github.mjs --dry-run
node scripts/bootstrap-github.mjs
```

It sets the description, Pages homepage, topics (as a set, so a removed topic
actually leaves the repository), Discussions/Issues, squash-only merge policy,
branch cleanup, private vulnerability reporting, Dependabot alerts, Dependabot
automated security fixes, GitHub Pages with `build_type: workflow`, 21 labels,
11 real issues with commands and acceptance criteria, the three issue pins, and
the `main` ruleset requiring the five CI checks. Every phase is idempotent. It
never changes visibility, pushes code, uploads a binary, or creates a release.

`--dry-run` is fully offline: it needs no network and no authenticated `gh`,
and prints the full ruleset JSON so you can read it before applying.

## Repository settings

Pages source, Dependabot alerts, and Dependabot security updates are set by
the bootstrap; the rest of this section is manual.

- Confirm the `Pages` workflow serves `https://kilbex.github.io/Vigla/`,
  `/demo/`, `/llms.txt`, and `/llms-full.txt`.
- Upload `docs/media/social-preview.png` as the repository social preview.
- Enable secret scanning and push protection. Confirm the private-report button
  (enabled by the bootstrap) from a signed-out/private browser without
  submitting a report.
- Keep Actions permissions read-only by default. The Pages build job receives
  only `contents: read` and `pages: read`; only the deploy job receives
  `pages: write` and `id-token: write`.
- Do not enable Releases automation or upload application binaries.

## Main branch ruleset

The bootstrap creates this ruleset. It requires pull requests with one
approving code-owner review, resolved conversations, and branches updated
before merge; blocks force pushes and branch deletion; allows squash merge
only; and adds a repository-admin bypass in **For pull requests only** mode,
because `CODEOWNERS` names a single maintainer who cannot self-approve.

**Owner verification step — do this after the first real run.** The bypass
actor is written as a numeric `RepositoryRole` id, and GitHub does not publish
those ids in its REST documentation, so the value is conventional rather than
sourced. Open **Settings → Rules → main** and confirm the bypass entry reads
*Repository admin*. If it does not, `main` is unmergeable by the sole
maintainer: enforcement is active and one approving code-owner review is
required. Fix it in the UI before merging anything.

The maintainer must still open a PR and may use the bypass only for the
impossible self-review, after every required check passes. Remove that bypass
as soon as a second trusted maintainer can review changes. The required checks,
which run on every PR:

- `Rust`
- `Frontend`
- `Browser E2E`
- `Portable Rust (Linux)`
- `Rust docs`
- `Supply chain`

Do not require the path-filtered `Verify local DMG recipe` check (the job in
the `Package smoke` workflow — GitHub matches required checks on the job name,
not the workflow name) globally; require and inspect it whenever it appears on
packaging, app, crate, or lockfile changes.

`Scorecard analysis` is likewise not required. OpenSSF Scorecard grades
repository *settings* — branch protection, action pinning, token permissions —
which a contributor cannot influence from a diff, so blocking merges on it
would fail PRs nobody can fix. It runs on `main`, weekly, on
`branch_protection_rule` events, and on demand.

## OpenSSF Scorecard (owner-gated)

[`.github/workflows/scorecard.yml`](../../.github/workflows/scorecard.yml)
writes `results.sarif` to a build artifact and sets `publish_results: false`.
Two follow-ups are the owner's call, and both are one-way doors:

1. **Publish the score.** Setting `publish_results: true` uploads this
   repository's grade to the public `api.scorecard.dev` dataset. Do it only
   after reading a local run's SARIF and accepting the score; there is no
   retraction. Only after a real run produces a grade does a README badge stop
   being a claim about nothing.
2. **Upload SARIF to code scanning** instead of an artifact. That needs code
   scanning enabled on the repository plus `security-events: write` on the job.
   Until it is enabled, adding the permission would fail the run.

The MSRV inputs Scorecard's own analysis does not cover — `cargo-deny`'s
`rust-version` and the published "Rust 1.95" requirement — are pinned to
`Cargo.toml` by `scripts/open-source-contract.test.mjs`, and the declared
minimum is compiled every run by the `Portable Rust (Linux)` job.
Except for the temporary PR-only self-review bypass above, apply the rules to
administrators unless an emergency procedure is documented in the incident
record.

## Discussions

Keep four categories: Announcements, Q&A, Show and tell, and Ideas. Pin a
“Start here” post with this body:

> Welcome to Vigla. Use Q&A for setup and workflow questions, Ideas for
> evidence-backed product proposals, and Show and tell for adapters, mission
> traces, or integrations you can share publicly. Reproducible bugs belong in
> Issues; vulnerabilities follow SECURITY.md and never belong in a public
> thread. Maintainers read daily during launch but reply asynchronously. Useful
> answers and decisions stay on GitHub so the next operator can find them.

Copy the versioned replies in `.github/SAVED_REPLIES.md` into the maintainer's
personal GitHub saved replies.

## Issue presentation

The bootstrap pins these three (GitHub's maximum) after seeding them:

1. `roadmap: Linux desktop packaging and parity`
2. `roadmap: decide and prove the Windows execution boundary`
3. `docs: specify an OpenCode adapter before adding a crate`

The pin phase re-probes GitHub after the seeding phase, so a first real run
pins successfully; `--dry-run` reports "pin pending" because nothing was
created. Confirm every issue has one state/area path, exact verification
commands, and no internal planning codename.

## Community chat

Create Discord only if a maintainer can read it daily during launch. Use at
most `#announcements`, `#help`, `#showcase`, and `#contributors`. Post this norm
before publishing an invite:

> Maintainers read daily and reply asynchronously. File reproducible bugs on
> GitHub; keep reusable answers and decisions in Discussions. Never post
> credentials, private code, or vulnerability details here.

Only after that is live, add the real invite to README, the landing page, and
SUPPORT.md in one PR. Do not commit a placeholder or dead invite.

## Final public checks

- Verify the owner profile has a stable avatar/name, a short public context
  paragraph, and Vigla pinned.
- Decide GitHub Sponsors intentionally. If no payout identity is configured,
  omit `FUNDING.yml` rather than publishing a dead button.
- Confirm two consecutive traffic snapshots reached the private archive.
- From a signed-out browser, click every README/site link and open every issue
  form.
- Have a second person follow the clean-clone mock and DMG instructions with a
  stopwatch; record OS, commit, commands, and result privately.

After every required workflow and signed-out check is green, move the initial
release notes from `Unreleased` to a dated `0.1.0` section and create an
annotated `v0.1.0` tag on that exact source commit. Vigla's first release is a
source reference only: do not attach a DMG or present an ad-hoc local signature
as a maintainer-issued binary.
