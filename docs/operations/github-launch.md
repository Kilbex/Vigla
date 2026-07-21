# GitHub launch controls

Repository files can define workflows, templates, ownership, labels, and issue
content, but several launch controls require repository-owner or personal
account access. Complete this checklist after the launch commit is on `main`
and before announcements.

## Automated owner step

Preview, then apply the idempotent metadata/label/issue bootstrap:

```sh
node scripts/bootstrap-github.mjs --dry-run
node scripts/bootstrap-github.mjs
```

It sets the description, Pages homepage, topics, Discussions/Issues, squash-only
merge policy, branch cleanup, 19 labels, and 12 real issues with commands and
acceptance criteria. It never changes visibility, pushes code, uploads a
binary, or creates a release.

## Repository settings

- Set Pages source to **GitHub Actions** and confirm the `Pages` workflow serves
  `https://kilbex.github.io/Vigla/`, `/demo/`, `/llms.txt`, and
  `/llms-full.txt`.
- Upload `docs/media/social-preview.png` as the repository social preview.
- Enable private vulnerability reporting, Dependabot alerts, and Dependabot
  security updates. Confirm the private-report button from a signed-out/private
  browser without submitting a report.
- Keep Actions permissions read-only by default; allow Pages' workflow-scoped
  `pages: write` and `id-token: write` permissions.
- Do not enable Releases automation or upload application binaries.

## Main branch ruleset

Require pull requests with one approving review, code-owner review, resolved
conversations, and branches updated before merge. Block force pushes and branch
deletion. Allow squash merge only. Require these checks, which run on every PR:

- `Rust`
- `Frontend`
- `Browser E2E`
- `Portable Rust (Linux)`
- `Supply chain`

Do not require the path-filtered `Package smoke` check globally; require and
inspect it whenever it appears on packaging, app, crate, or lockfile changes.
Apply the rules to administrators unless an emergency procedure is documented
in the incident record.

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
