# Mac App Store feasibility

Vigla's public-distribution goal is a trustworthy Mac App Store release without
turning the store build into a smaller or misleading product. This track is a
technical gate, not a release promise.

## Why feasibility comes first

Apple requires Mac App Store apps to use
[App Sandbox](https://developer.apple.com/documentation/security/app-sandbox).
Vigla's core workflow crosses three boundaries that the current source-built
app can access directly:

- a repository chosen by the operator, including its nested worktrees;
- installed coding-agent command-line tools and their configuration;
- child processes that run builds, tests, and Git commands for the mission.

Apple documents security-scoped access for user-selected folders and persistent
bookmarks, but its sandbox guidance also limits running programs outside the app
bundle, container, or app-group containers. That makes CLI execution the first
unknown to prove, not a packaging detail to defer.

## Review-policy gate

Technical feasibility is not App Store approval. Apple's
[App Review Guidelines](https://developer.apple.com/app-store/review/guidelines/)
require Mac App Store software to be self-contained and restrict downloaded or
executed code that changes an app's features or functionality. Vigla
intentionally launches user-installed agent tools plus project builds and tests,
so the real workflow must receive a review decision with accurate review notes;
a successful sandbox spike alone does not justify an availability claim.

## Proof required before store work

A disposable sandbox spike must demonstrate all of the following on a clean
Mac without relying on temporary-exception entitlements or developer-machine
permissions:

1. The system folder picker grants recursive access to a repository, and a
   security-scoped bookmark restores that access after relaunch.
2. Vigla can create, inspect, merge, revert, and remove its own Git worktrees
   inside the selected repository.
3. At least one supported, user-installed agent CLI can launch, stream output,
   receive cancellation, and exit without a sandbox violation.
4. The app can run the credential-free mission, persist its event stream, open
   the completion verdict, and revert the mission in the sandboxed build.
5. A release-like package passes the existing unit, browser, receipt, and local
   package-smoke gates with the sandbox entitlement enabled.

If item 3 cannot pass without changing the product's trust boundary, pause this
track and record the decision. Do not hide the limitation behind setup copy or
ship a store build that implies parity with the source-built app.

## Distribution work after the gate

Only after the spike passes:

- add a dedicated store signing and provisioning path without weakening the
  reproducible local build;
- prepare accurate product-page metadata, privacy answers, screenshots, and an
  app preview from the deterministic public capture scenes;
- test installation, first run, folder reauthorization, CLI discovery, mission
  completion, and update behavior on a clean machine;
- publish a support URL, privacy URL, source link, and review notes that state
  the CLI and repository access model plainly;
- submit the release-like workflow for review and record the result before any
  App Store availability announcement.

The source-built DMG remains the supported distribution path until every proof
above is recorded. GitHub traffic and App Store Connect's aggregate product-page
metrics can measure launch discovery without adding product telemetry.

## Revisit trigger

Move this track from feasibility to release execution only when the sandboxed
spike completes a real mission with a user-installed CLI and a user-selected
repository, then repeats it after app relaunch.
