#!/usr/bin/env bash
set -euo pipefail

ROOT="$(cd "$(dirname "$0")/.." && pwd)"

if ! command -v rg >/dev/null 2>&1; then
  echo "routing leak check requires rg" >&2
  exit 2
fi

allowed_file() {
  case "$1" in
    "$ROOT/crates/orchestrator/src/vendor_profile.rs") return 0 ;;
    "$ROOT/crates/adapters/"*) return 0 ;;
    "$ROOT/crates/orchestrator/resources/vendor_profiles/"*) return 0 ;;
    *) return 1 ;;
  esac
}

patterns=(
  'Command::new\("(claude|codex|gemini|antigravity|kiro|copilot)"\)'
  '\.arg\("--(append-system-prompt|output-format|permission-mode|dangerously-skip-permissions|dangerously-bypass-approvals-and-sandbox|skip-git-repo-check|skip-trust|approval-mode)"\)'
)

fail=0
for pattern in "${patterns[@]}"; do
  while IFS=: read -r file line text; do
    [[ -z "${file:-}" ]] && continue
    abs="$file"
    [[ "$abs" = /* ]] || abs="$ROOT/$file"
    if ! allowed_file "$abs"; then
      printf 'routing leak: %s:%s:%s\n' "$file" "$line" "$text" >&2
      fail=1
    fi
  done < <(
    rg -n --glob '*.rs' --glob '!target/**' --glob '!node_modules/**' \
      "$pattern" "$ROOT/crates/orchestrator/src" "$ROOT/app/src-tauri/src" "$ROOT/crates/adapters" || true
  )
done

if [[ "$fail" -ne 0 ]]; then
  echo "vendor-specific CLI launch routing must live in adapters or vendor profiles" >&2
  exit 1
fi

echo "routing leak check passed"
