import assert from "node:assert/strict";
import { readFile } from "node:fs/promises";
import test from "node:test";

const buildScript = await readFile(new URL("./build.sh", import.meta.url), "utf8");
const packageWorkflow = await readFile(
  new URL("../.github/workflows/package-smoke.yml", import.meta.url),
  "utf8",
);

test("the DMG verifier uses the pre-macOS-26 hdiutil interface", () => {
  assert.match(buildScript, /\bhdiutil attach\b/);
  assert.doesNotMatch(buildScript, /\bdiskutil image attach\b/);
});

test("package smoke runs on a pre-macOS-26 image", () => {
  assert.match(packageWorkflow, /^\s*runs-on:\s*macos-14\s*$/m);
});
