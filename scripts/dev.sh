#!/usr/bin/env bash
# Convenience launcher for local development.
#
# Builds the mock-harness binary first so the orchestrator can find
# it, then starts `pnpm tauri dev`.
#
# Two builds are required:
#   * release — vigla-host declares target/release/mock-harness as a
#     Tauri bundle resource, and tauri_build validates that path on
#     every compile of the host crate. Without it, `pnpm tauri dev`
#     fails from a clean clone (see crates/xtask/src/main.rs).
#   * debug — at runtime the dev host binary lives in target/debug/,
#     and Supervisor::locate_mock_harness resolves the harness as a
#     sibling of the host executable.

set -euo pipefail

ROOT="$(cd "$(dirname "$0")/.." && pwd)"
cd "$ROOT"

echo "[dev] cargo build -p vigla-mock-harness --release --bin mock-harness"
cargo build -p vigla-mock-harness --release --bin mock-harness

echo "[dev] cargo build -p vigla-mock-harness --bin mock-harness"
cargo build -p vigla-mock-harness --bin mock-harness

echo "[dev] pnpm tauri dev"
cd app
pnpm tauri dev
