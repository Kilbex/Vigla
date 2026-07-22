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

It sets the description, Pages homepage, topics, Discussions/Issues, squash-only
merge policy, branch cleanup, private vulnerability reporting, 21 labels, and
11 real issues with commands and acceptance criteria. It never changes
visibility, pushes code, uploads a binary, or creates a release.

## Repository settings

- Set Pages source to **GitHub Actions** and confirm the `Pages` workflow serves
  `https://kilbex.github.io/Vigla/`, `/demo/`, `/llms.txt`, and
  `/llms-full.txt`.
- Upload `docs/media/social-preview.png` as the repository social preview.
- Enable private vulnerability reporting, Dependabot alerts, and Dependabot
  security updates, secret scanning, and push protection. Confirm the
  private-report button from a signed-out/private browser without submitting a
  report.
- Keep Actions permissions read-only by default. The Pages build job receives
  only `contents: read` and `pages: read`; only the deploy job receives
  `pages: write` and `id-token: write`.
- Do not enable Releases automation or upload application binaries.

## Main branch ruleset

Require pull requests with one approving code-owner review, resolved
conversations, and branches updated before merge. Block force pushes and branch
deletion. Allow squash merge only. While `CODEOWNERS` names a single
maintainer, add the repository-admin role to the ruleset bypass list in **For
pull requests only** mode. The maintainer must still open a PR and may use the
bypass only for the impossible self-review, after every required check passes.
Remove that bypass as soon as a second trusted maintainer can review changes.
Require these checks, which run on every PR:

- `Rust`
- `Frontend`
- `Browser E2E`
- `Portable Rust (Linux)`
- `Supply chain`

Do not require the path-filtered `Package smoke` check globally; require and
inspect it whenever it appears on packaging, app, crate, or lockfile changes.
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

Pin these three after the bootstrap creates them:

1. `roadmap: Linux desktop packaging and parity`
2. `adapter: add an Antigravity conformance transcript`
3. `docs: specify an OpenCode adapter before adding a crate`

Confirm every issue has one state/area path, exact verification commands, and
no internal planning codename.

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
