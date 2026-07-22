import assert from "node:assert/strict";
import { readFile } from "node:fs/promises";
import test from "node:test";

const buildScript = await readFile(new URL("./build.sh", import.meta.url), "utf8");
const packageWorkflow = await readFile(
  new URL("../.github/workflows/package-smoke.yml", import.meta.url),
  "utf8",
);
const ciWorkflow = await readFile(
  new URL("../.github/workflows/ci.yml", import.meta.url),
  "utf8",
);
const pagesWorkflow = await readFile(
  new URL("../.github/workflows/pages.yml", import.meta.url),
  "utf8",
);
const observeQuotaScript = await readFile(
  new URL("./observe-quota.sh", import.meta.url),
  "utf8",
);

test("the DMG verifier uses the pre-macOS-26 hdiutil interface", () => {
  assert.match(buildScript, /\bhdiutil attach\b/);
  assert.doesNotMatch(buildScript, /\bdiskutil image attach\b/);
});

test("the mounted app must contain byte-exact legal resources", () => {
  const verificationStart = buildScript.indexOf(
    'LICENSE_DIR="$MOUNTED_APP/Contents/Resources/licenses"',
  );
  const detach = buildScript.indexOf(
    'hdiutil detach "$MOUNT_POINT" >/dev/null',
    verificationStart,
  );

  assert.notEqual(verificationStart, -1);
  assert.ok(verificationStart < detach);
  for (const name of [
    "LICENSE",
    "NOTICE",
    "THIRD_PARTY_NOTICES.md",
    "THIRD_PARTY_NOTICES.txt",
  ]) {
    assert.match(buildScript, new RegExp(`^  ${name}$`, "m"));
  }
  assert.match(
    buildScript,
    /cmp -s "\$ROOT\/\$legal_file" "\$LICENSE_DIR\/\$legal_file"/,
  );
  assert.match(
    buildScript,
    /diff -qr "\$ROOT\/third_party_licenses" "\$LICENSE_DIR\/third_party_licenses"/,
  );
});

test("package smoke runs on a pre-macOS-26 image", () => {
  assert.match(packageWorkflow, /^\s*runs-on:\s*macos-14\s*$/m);
});

test("the local DMG build cannot inherit certificate credentials", () => {
  const credentialReset = buildScript.indexOf(
    "unset APPLE_CERTIFICATE APPLE_CERTIFICATE_PASSWORD",
  );
  const dependencyInstall = buildScript.indexOf("pnpm install --frozen-lockfile");
  const cargoBuild = buildScript.indexOf("cargo build -p vigla-mock-harness");
  const tauriBuild = buildScript.indexOf("pnpm tauri build");

  assert.notEqual(credentialReset, -1);
  assert.ok(credentialReset < dependencyInstall);
  assert.ok(credentialReset < cargoBuild);
  assert.ok(credentialReset < tauriBuild);
});

test("the embeddings bundle cannot inherit an alternate ONNX Runtime", () => {
  const runtimeResetStart = buildScript.indexOf(
    "unset ORT_LIB_PATH ORT_LIB_LOCATION ORT_LIB_PROFILE ORT_VCPKG_TARGET",
  );
  const runtimeResetEnd = buildScript.indexOf(
    "unset ORT_CXX_STDLIB ORT_CUDA_VERSION ORT_CACHE_DIR",
  );
  const firstCargoBuild = buildScript.indexOf("cargo build -p vigla-mock-harness");
  const tauriBuild = buildScript.indexOf("pnpm tauri build");
  const resetBlock = buildScript.slice(runtimeResetStart, runtimeResetEnd + 60);
  const ortSysBuildVariables = [
    "ORT_LIB_PATH",
    "ORT_LIB_LOCATION",
    "ORT_LIB_PROFILE",
    "ORT_VCPKG_TARGET",
    "ORT_IOS_XCFWK_PATH",
    "ORT_IOS_XCFWK_LOCATION",
    "ORT_EXT_IOS_XCFWK_PATH",
    "ORT_EXT_IOS_XCFWK_LOCATION",
    "ORT_PREFER_DYNAMIC_LINK",
    "ORT_SKIP_DOWNLOAD",
    "ORT_OFFLINE",
    "ORT_CXX_STDLIB",
    "ORT_CUDA_VERSION",
    "ORT_CACHE_DIR",
  ];

  assert.notEqual(runtimeResetStart, -1);
  assert.notEqual(runtimeResetEnd, -1);
  for (const variable of ortSysBuildVariables) {
    assert.match(resetBlock, new RegExp(`\\b${variable}\\b`));
  }
  assert.ok(runtimeResetStart < firstCargoBuild);
  assert.ok(runtimeResetStart < tauriBuild);
});

test("supply-chain policy covers optional Cargo features", () => {
  assert.match(ciWorkflow, /^\s*arguments:\s*["']--all-features["']\s*$/m);
});

test("CI compile-checks optional Cargo features", () => {
  assert.match(
    ciWorkflow,
    /^\s*run:\s*cargo check --workspace --all-targets --all-features\s*$/m,
  );
});

test("Pages write and OIDC permissions are isolated to deployment", () => {
  const buildJob = pagesWorkflow.slice(
    pagesWorkflow.indexOf("  build:"),
    pagesWorkflow.indexOf("  deploy:"),
  );
  const deployJob = pagesWorkflow.slice(pagesWorkflow.indexOf("  deploy:"));

  assert.match(buildJob, /^\s+contents:\s*read\s*$/m);
  assert.match(buildJob, /^\s+pages:\s*read\s*$/m);
  assert.doesNotMatch(buildJob, /^\s+pages:\s*write\s*$/m);
  assert.doesNotMatch(buildJob, /^\s+id-token:\s*write\s*$/m);
  assert.match(deployJob, /^\s+pages:\s*write\s*$/m);
  assert.match(deployJob, /^\s+id-token:\s*write\s*$/m);
});

test("the quota observation harness executes argv without eval", () => {
  assert.doesNotMatch(observeQuotaScript, /\beval\s+["']?\$@/);
  assert.match(observeQuotaScript, /^\s*"\$@"\s*$/m);
});

test("the quota observation harness uses a private unpredictable directory", () => {
  assert.match(observeQuotaScript, /mktemp -d/);
  assert.match(observeQuotaScript, /chmod 700 "\$L1_DIR"/);
  assert.match(observeQuotaScript, /run chmod 700 "\$LOG_DIR"/);
  assert.doesNotMatch(observeQuotaScript, /L1_DIR="\/tmp\/vigla-l1-quota"/);
  assert.doesNotMatch(observeQuotaScript, /run rm -rf/);
});

test("the quota observation harness routes host logs to the directory it tails", () => {
  assert.match(observeQuotaScript, /^LOG_DIR="\$L1_DIR\/logs"$/m);
  assert.match(observeQuotaScript, /^export VIGLA_LOG_DIR="\$LOG_DIR"$/m);

  const logExport = observeQuotaScript.indexOf(
    'export VIGLA_LOG_DIR="$LOG_DIR"',
  );
  const launch = observeQuotaScript.indexOf('exec "$ROOT/scripts/dev.sh"');
  assert.notEqual(logExport, -1);
  assert.notEqual(launch, -1);
  assert.ok(logExport < launch);
});
