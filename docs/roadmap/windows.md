# Windows desktop support

Status: candidate for v0.3, after the Linux host boundary is proven. This
document does not claim that native Windows or WSL is supported today.

## Decision to make first

Choose one primary lane with evidence:

- a native Windows Tauri host managing native git worktrees and CLI processes;
  or
- a native UI deliberately coordinating repositories and CLIs inside WSL.

Trying to present both as one transparent filesystem is a regression risk.
Path semantics, credential stores, process ownership, line endings, file
watchers, and terminal behavior differ. A short prototype must measure these
boundaries before the public support promise or package format is selected.

## Required seams

- platform-specific app-data, notification, and file-picker adapters;
- explicit path-domain types where a value may be native or WSL;
- process-tree termination that cannot orphan worker CLIs;
- safe quoting for PowerShell, `cmd.exe`, and any WSL bridge;
- git worktree tests covering drive letters, spaces, long paths, case behavior,
  and CRLF repositories;
- recovery tests for reboot, shell exit, and unavailable WSL distributions.

## Packaging constraints

The local-build-only rule remains. Any MSIX or installer recipe must run on the
user's machine, verify the package, print a checksum, and avoid project-owned
code-signing or update credentials. CI may prove the recipe but must not upload
a maintainer-built application package.

## Acceptance gate

On a fresh supported Windows image:

1. build the native host from a clean clone using documented prerequisites;
2. run the frontend, portable Rust, and platform host tests;
3. exercise accepted, blocked, and quota-paused mock missions;
4. prove worktree isolation and whole-mission revert in a repository whose path
   contains spaces;
5. prove all worker processes terminate when the app stops a mission;
6. build and verify the chosen local installer format;
7. state the exact native/WSL boundary in README requirements and diagnostics.

An evaluation report must publish failed experiments as well as the chosen
lane. No Windows badge or support wording lands before this gate is green.
