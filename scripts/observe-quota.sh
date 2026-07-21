#!/usr/bin/env bash
# L1 row-4 observation harness — quota pause / resume.
#
# Sets up a clean, isolated Vigla L1 environment and launches the
# desktop app via `scripts/dev.sh`. Designed to be re-runnable: each
# invocation wipes the prior `/tmp/vigla-l1-quota/` state and
# clones a fresh `tests/samples/sandbox/` working tree. The mission JSON
# uses the env-gated `claude_quota_exhausted` worker backend so row 4
# is deterministic and no near-quota Claude account is required.
#
# Usage:
#   scripts/observe-quota.sh           # full setup + launch UI
#   scripts/observe-quota.sh --dry-run # print what would happen; no launch
#
# This script never modifies production paths — everything lands under
# `/tmp/vigla-l1-quota/`.

set -euo pipefail

DRY_RUN=0
if [[ "${1:-}" == "--dry-run" ]]; then
    DRY_RUN=1
fi

run() {
    if [[ "$DRY_RUN" -eq 1 ]]; then
        echo "[dry-run] $*"
    else
        echo "[observe-quota] $*"
        eval "$@"
    fi
}

ROOT="$(cd "$(dirname "$0")/.." && pwd)"
L1_DIR="/tmp/vigla-l1-quota"
DB_PATH="$L1_DIR/db.sqlite"
REPO_ROOT="$L1_DIR/repo"
LOG_DIR="$L1_DIR/logs"
MISSION_JSON="$ROOT/crates/orchestrator/tests/fixtures/quota-observation-mission.json"
SANDBOX_SRC="$ROOT/tests/samples/sandbox"

echo "=== L1 row-4 quota-observation environment ==="
echo "L1 dir       : $L1_DIR"
echo "Database     : $DB_PATH"
echo "Repo root    : $REPO_ROOT"
echo "Log dir      : $LOG_DIR"
echo "Mission JSON : $MISSION_JSON"
echo "Sandbox src  : $SANDBOX_SRC"
echo "Dry-run      : $DRY_RUN"
echo

if [[ ! -f "$MISSION_JSON" ]]; then
    echo "[observe-quota] ERROR: mission template missing: $MISSION_JSON" >&2
    exit 1
fi
if [[ ! -d "$SANDBOX_SRC" ]]; then
    echo "[observe-quota] ERROR: sandbox source missing: $SANDBOX_SRC" >&2
    exit 1
fi

# Preflight vendor versions — surface the failure here, not 5 hours in.
if ! command -v claude >/dev/null 2>&1; then
    echo "[observe-quota] ERROR: \`claude\` not on PATH" >&2
    exit 1
fi
echo "[observe-quota] claude --version = $(claude --version 2>&1 || echo '?')"

run "rm -rf '$L1_DIR'"
run "mkdir -p '$L1_DIR' '$REPO_ROOT' '$LOG_DIR'"
run "cp -R '$SANDBOX_SRC/.' '$REPO_ROOT/'"

# Initialise a fresh git repo inside the cloned sandbox so the
# orchestrator's worktree machinery has a HEAD to branch from.
run "(cd '$REPO_ROOT' && git init -q -b main && git add -A && \
    git -c user.email=l1@vigla.local -c user.name='L1 Owner' \
        commit -q -m 'L1 row-4 sandbox baseline')"

cat <<EOF

[observe-quota] Environment ready. Export these in the shell you'll
run the app from (or source this script):

    export VIGLA_DB_PATH='$DB_PATH'
    export VIGLA_REPO_ROOT='$REPO_ROOT'
    export VIGLA_L1_QUOTA_MOCK=1
    export VIGLA_L1_QUOTA_RESET_MS='\${VIGLA_L1_QUOTA_RESET_MS:-90000}'

[observe-quota] Mission JSON mirrored by Settings → Developer →
start quota mission:

    $MISSION_JSON

[observe-quota] Tail the supervisor log here:

    tail -F '$LOG_DIR/'*

[observe-quota] Capture-points for quota pause/resume: paused-card
screenshot, mission_id, exact UTC timestamp,
\`MissionPaused\`/\`MissionResumed\` log lines.

EOF

if [[ "$DRY_RUN" -eq 1 ]]; then
    echo "[observe-quota] --dry-run finished; not launching dev.sh"
    exit 0
fi

export VIGLA_DB_PATH="$DB_PATH"
export VIGLA_REPO_ROOT="$REPO_ROOT"
export VIGLA_L1_QUOTA_MOCK=1
export VIGLA_L1_QUOTA_RESET_MS="${VIGLA_L1_QUOTA_RESET_MS:-90000}"

echo "[observe-quota] handing off to scripts/dev.sh"
exec "$ROOT/scripts/dev.sh"
