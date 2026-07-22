#!/usr/bin/env bash
# Build a local-only macOS disk image in one command.
#
# The bundle is ad-hoc signed. It never uses an Apple account, Developer ID,
# notarization credential, or maintainer-owned release service. The resulting
# DMG stays under target/ and is intended for the person who built it.

set -euo pipefail

# Sanitize the environment before *any* dependency-controlled command runs.
# Package lifecycle hooks and Cargo build scripts execute code, so clearing
# credentials only immediately before Tauri would still expose them earlier in
# this local-only build.
export APPLE_SIGNING_IDENTITY="-"
unset APPLE_CERTIFICATE APPLE_CERTIFICATE_PASSWORD
unset APPLE_ID APPLE_TEAM_ID APPLE_PASSWORD APPLE_API_KEY APPLE_API_KEY_PATH APPLE_API_ISSUER

# The optional embeddings build must use the checksum-verified ONNX Runtime
# selected by the locked ort-sys crate. Do not let an ambient developer override
# substitute a different native runtime while the bundle ships the pinned 1.24.2
# notices. These are all ort-sys build-selection variables as of rc.12.
unset ORT_LIB_PATH ORT_LIB_LOCATION ORT_LIB_PROFILE ORT_VCPKG_TARGET
unset ORT_IOS_XCFWK_PATH ORT_IOS_XCFWK_LOCATION
unset ORT_EXT_IOS_XCFWK_PATH ORT_EXT_IOS_XCFWK_LOCATION
unset ORT_PREFER_DYNAMIC_LINK ORT_SKIP_DOWNLOAD ORT_OFFLINE
unset ORT_CXX_STDLIB ORT_CUDA_VERSION ORT_CACHE_DIR

ROOT="$(cd "$(dirname "$0")/.." && pwd)"
DMG_DIR="$ROOT/target/release/bundle/dmg"
LEGAL_DISTRIBUTION_FILES=(
  LICENSE
  NOTICE
  THIRD_PARTY_NOTICES.md
  THIRD_PARTY_NOTICES.txt
)
BUILD_MARKER="$(mktemp -t vigla-dmg-build.XXXXXX)"
MOUNT_POINT=""
MOUNT_ROOT=""

cleanup() {
  if [[ -n "$MOUNT_POINT" ]]; then
    hdiutil detach "$MOUNT_POINT" >/dev/null 2>&1 || true
  fi
  if [[ -n "$MOUNT_ROOT" ]]; then
    rmdir "$MOUNT_POINT" "$MOUNT_ROOT" >/dev/null 2>&1 || true
  fi
  rm -f "$BUILD_MARKER"
}
trap cleanup EXIT

fail() {
  echo "[build] error: $*" >&2
  exit 1
}

require_command() {
  command -v "$1" >/dev/null 2>&1 || fail "required command not found: $1"
}

[[ "$(uname -s)" == "Darwin" ]] || fail "DMG packaging requires macOS"

for command_name in cargo node pnpm xcode-select hdiutil codesign shasum cmp diff; do
  require_command "$command_name"
done

xcode-select -p >/dev/null 2>&1 || fail "Xcode Command Line Tools are not configured; run: xcode-select --install"

echo "[build] installing locked JavaScript dependencies"
(cd "$ROOT" && pnpm install --frozen-lockfile)

echo "[build] compiling the bundled mock harness"
(cd "$ROOT" && cargo build -p vigla-mock-harness --release --bin mock-harness)

[[ -x "$ROOT/target/release/mock-harness" ]] || \
  fail "target/release/mock-harness is missing or is not executable"

TAURI_BUILD_ARGS=(--bundles dmg --ci)
if [[ -n "${EMBEDDINGS:-}" ]]; then
  echo "[build] enabling the optional embeddings feature"
  TAURI_BUILD_ARGS+=(--features embeddings)
fi

echo "[build] creating an ad-hoc-signed local DMG"
(cd "$ROOT/app" && pnpm tauri build "${TAURI_BUILD_ARGS[@]}")

mapfile_supported=false
if help mapfile >/dev/null 2>&1; then
  mapfile_supported=true
fi

if [[ "$mapfile_supported" == true ]]; then
  mapfile -t dmg_files < <(find "$DMG_DIR" -type f -name '*.dmg' -newer "$BUILD_MARKER" -print 2>/dev/null | sort)
else
  dmg_files=()
  while IFS= read -r dmg_file; do
    dmg_files+=("$dmg_file")
  done < <(find "$DMG_DIR" -type f -name '*.dmg' -newer "$BUILD_MARKER" -print 2>/dev/null | sort)
fi

[[ ${#dmg_files[@]} -eq 1 ]] || \
  fail "expected exactly one new DMG in $DMG_DIR; found ${#dmg_files[@]}"

DMG_PATH="${dmg_files[0]}"
[[ -s "$DMG_PATH" ]] || fail "generated DMG is empty: $DMG_PATH"

echo "[build] verifying disk image and mounted app signature"
hdiutil verify "$DMG_PATH" >/dev/null
MOUNT_ROOT="$(mktemp -d -t vigla-dmg-mount.XXXXXX)"
MOUNT_POINT="$MOUNT_ROOT/Vigla"
mkdir "$MOUNT_POINT"
hdiutil attach -readonly -nobrowse -mountpoint "$MOUNT_POINT" "$DMG_PATH" >/dev/null

MOUNTED_APP="$MOUNT_POINT/Vigla.app"
[[ -d "$MOUNTED_APP" ]] || fail "Vigla.app is missing from the generated DMG"
codesign --verify --deep --strict "$MOUNTED_APP"
SIGNATURE_INFO="$(codesign -dv --verbose=4 "$MOUNTED_APP" 2>&1)"
grep -Fq 'Signature=adhoc' <<<"$SIGNATURE_INFO" || \
  fail "the generated app is not ad-hoc signed"
grep -Fq 'TeamIdentifier=not set' <<<"$SIGNATURE_INFO" || \
  fail "the generated app unexpectedly contains an Apple team identity"

LICENSE_DIR="$MOUNTED_APP/Contents/Resources/licenses"
[[ -d "$LICENSE_DIR" ]] || fail "bundled legal resources are missing"
for legal_file in "${LEGAL_DISTRIBUTION_FILES[@]}"; do
  [[ -f "$LICENSE_DIR/$legal_file" ]] || \
    fail "bundled legal resource is missing: $legal_file"
  cmp -s "$ROOT/$legal_file" "$LICENSE_DIR/$legal_file" || \
    fail "bundled legal resource differs from the repository: $legal_file"
done
[[ -d "$LICENSE_DIR/third_party_licenses" ]] || \
  fail "bundled third-party license directory is missing"
diff -qr "$ROOT/third_party_licenses" "$LICENSE_DIR/third_party_licenses" >/dev/null || \
  fail "bundled third-party licenses differ from the repository"

hdiutil detach "$MOUNT_POINT" >/dev/null
rmdir "$MOUNT_POINT" "$MOUNT_ROOT"
MOUNT_POINT=""
MOUNT_ROOT=""

echo
echo "Local DMG ready: ${DMG_PATH#"$ROOT/"}"
echo "SHA-256: $(shasum -a 256 "$DMG_PATH" | awk '{print $1}')"
echo "This artifact was built locally, ad-hoc signed, and was not uploaded."
