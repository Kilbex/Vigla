# Linux desktop support

Status: planned for the v0.2 launch wave. This is a public implementation
specification, not a support claim.

## Outcome

A contributor can build Vigla locally on a supported Ubuntu LTS machine, run
all credential-free mission flows, and produce a locally verified AppImage and
Debian package without uploading either artifact from project CI.

The first supported target will be one still-supported Ubuntu LTS release on
`x86_64`, selected and recorded when implementation starts. Other
distributions and architectures require their own evidence; “Tauri can target
Linux” is not evidence that Vigla's PTY, path, notification, and packaging
behavior works there.

## Existing foundation

- CI builds, lints, and tests every workspace crate except the macOS Tauri
  host on `ubuntu-latest`.
- Business logic lives in portable crates; the Tauri host is a narrow adapter.
- The browser E2E suite exercises the frontend without a native shell.
- The local-build-only distribution rule already forbids CI artifact uploads.

## Workstreams

### Host compilation

- Gate macOS-only imports and entitlements behind target-specific modules.
- Document the exact GTK/WebKit and system-package prerequisites.
- Keep platform branching in host adapters; do not fork orchestrator policy.
- Add a Linux host compile job only after its dependency install is pinned and
  reproducible.

### Behavioral parity

- Normalize paths without assuming `/Applications`, Finder, or Apple bundle
  layout.
- Verify worktree creation, cleanup, and target-merge rollback on a case-sensitive
  filesystem.
- Verify PTY lifecycle, termination, stdout/stderr ordering, and process-tree
  cleanup.
- Implement notifications with a tested unavailable-permission fallback.
- Confirm app-data and SQLite locations follow platform conventions.

### Local packaging

- Add a Linux counterpart to `scripts/build.sh` that produces AppImage and
  `.deb` files locally, verifies each package, and prints SHA-256 checksums.
- Keep signing optional and user-owned. Do not add release uploads, hosted
  packages, or an updater channel.
- Ensure a failed build cannot mistake a stale package for a new success.

### User experience

- Audit file pickers, keyboard labels, font rendering, window chrome, and
  notification wording on Linux.
- Add a platform-specific troubleshooting section without weakening the common
  product vocabulary.
- Capture Linux screenshots only from the supported build.

## Acceptance gate

On a fresh supported Ubuntu LTS VM:

1. install documented prerequisites;
2. clone the repository and install locked frontend dependencies;
3. run the full portable Rust, frontend, and browser gates;
4. launch all three mock mission outcomes through the native app;
5. complete a rollback-anchored target merge and revert it;
6. build one AppImage and one `.deb` locally;
7. verify both packages and their checksums;
8. uninstall cleanly without removing user repositories or unrelated config.

The evidence must include commands, OS image identifier, architecture, package
hashes, and any platform-specific skipped checks. No credentialed vendor run is
required for the initial packaging gate.

## Small contribution lanes

- inventory macOS-only host imports with file-and-line evidence;
- add a Linux path-location unit test;
- add a case-sensitive worktree fixture;
- document the prerequisite packages and prove them in a disposable CI job;
- add notification fallback tests behind a platform-neutral interface.

Large packaging changes should begin with a tracking-issue comment naming the
smallest slice and its rollback plan.
