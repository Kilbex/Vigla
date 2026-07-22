#!/usr/bin/env node

import { createHash } from "node:crypto";
import { readFile, writeFile } from "node:fs/promises";
import { fileURLToPath } from "node:url";
import path from "node:path";

const repoRoot = path.resolve(path.dirname(fileURLToPath(import.meta.url)), "..");
const retainedRoot = path.join(repoRoot, "third_party_licenses");

export const retainedRemoteLicenses = Object.freeze([
  Object.freeze({
    component: "ONNX Runtime",
    version: "1.24.2",
    fileName: "onnxruntime-1.24.2-LICENSE.txt",
    url: "https://raw.githubusercontent.com/microsoft/onnxruntime/v1.24.2/LICENSE",
    sha256: "2f07c72751aed99790b8a4869cf2311df85a860b22ded05fa22803587a48922c",
  }),
  Object.freeze({
    component: "ONNX Runtime",
    version: "1.24.2",
    fileName: "onnxruntime-1.24.2-ThirdPartyNotices.txt",
    url: "https://raw.githubusercontent.com/microsoft/onnxruntime/v1.24.2/ThirdPartyNotices.txt",
    sha256: "0e07b95f3a8d6230037707c5c4a2b554d12c4cb67369669ac255635528ffcee2",
  }),
  Object.freeze({
    packages: Object.freeze(["Inflector@0.11.4"]),
    licenseIds: Object.freeze(["BSD-2-Clause"]),
    fileName: "inflector-0.11.4-LICENSE.md",
    url: "https://raw.githubusercontent.com/whatisinternet/inflector/a4a95eac75043f4bffb127c7c8ec886b5b106053/LICENSE.md",
    sha256: "03d1bd5bfbee8d44651e7f57faf9e5b4eda4233f0d4eda36a716f2f0533d230b",
  }),
  Object.freeze({
    packages: Object.freeze(["alloc-stdlib@0.2.2"]),
    licenseIds: Object.freeze(["BSD-3-Clause"]),
    fileName: "alloc-stdlib-0.2.2-LICENSE.txt",
    url: "https://raw.githubusercontent.com/dropbox/rust-alloc-no-stdlib/6032b6a9b20e03737135c55a0270ccffcc1438ef/LICENSE",
    sha256: "c0c56f26d9c051cac4d200c34c84e7ae9aaa853e01a982a1df08b09931e518ae",
  }),
  Object.freeze({
    packages: Object.freeze(["brotli@8.0.2"]),
    licenseIds: Object.freeze(["BSD-3-Clause"]),
    fileName: "brotli-8.0.2-LICENSE.BSD-3-Clause.txt",
    url: "https://raw.githubusercontent.com/dropbox/rust-brotli/769efcbca153bfc4737f1986459da5d9d23368b8/LICENSE.BSD-3-Clause",
    sha256: "c0c56f26d9c051cac4d200c34c84e7ae9aaa853e01a982a1df08b09931e518ae",
  }),
  Object.freeze({
    packages: Object.freeze(["block2@0.6.2"]),
    fileName: "objc2-block2-0.6.2-LICENSE.md",
    url: "https://raw.githubusercontent.com/madsmtm/objc2/b4167b582b2f75f9a1be75495c41b765344fd03c/LICENSE.md",
    sha256: "7f976f7e9cb2d87df7230606feb932c3f21ac0e664045a775b600046ff850c54",
  }),
  Object.freeze({
    packages: Object.freeze(["dispatch2@0.3.1", "objc2@0.6.4"]),
    fileName: "objc2-core-0.6.4-LICENSE.md",
    url: "https://raw.githubusercontent.com/madsmtm/objc2/8852b424193ca41602281b3d7540d7c8ed51e49a/LICENSE.md",
    sha256: "7f976f7e9cb2d87df7230606feb932c3f21ac0e664045a775b600046ff850c54",
  }),
  Object.freeze({
    packages: Object.freeze([
      "objc2-app-kit@0.3.2",
      "objc2-core-foundation@0.3.2",
      "objc2-foundation@0.3.2",
      "objc2-web-kit@0.3.2",
    ]),
    fileName: "objc2-frameworks-0.3.2-LICENSE.md",
    url: "https://raw.githubusercontent.com/madsmtm/objc2/7b1abfd750a2cacaea71d6a56ecfb83cb7de560b/LICENSE.md",
    sha256: "7f976f7e9cb2d87df7230606feb932c3f21ac0e664045a775b600046ff850c54",
  }),
  Object.freeze({
    packages: Object.freeze([
      "objc2-encode@4.1.0",
      "objc2-exception-helper@0.1.1",
    ]),
    fileName: "objc2-encode-4.1.0-LICENSE.md",
    url: "https://raw.githubusercontent.com/madsmtm/objc2/8d214f5477365ffcbcbb7de058c86ed9a518efb7/LICENSE.md",
    sha256: "7f976f7e9cb2d87df7230606feb932c3f21ac0e664045a775b600046ff850c54",
  }),
  Object.freeze({
    packages: Object.freeze([
      "specta@2.0.0-rc.22",
      "specta-serde@0.0.9",
      "specta-typescript@0.0.9",
    ]),
    licenseIds: Object.freeze(["MIT"]),
    fileName: "specta-2.0.0-rc.22-LICENSE.txt",
    url: "https://raw.githubusercontent.com/oscartbeaumont/specta/42ca9e89216848f3582e90bf961b12be4a33685e/LICENSE",
    sha256: "9ec86f39e235fcd35e6d91ef3fd4d6b88e5763f3ce1e296c222969986b6f6475",
  }),
  Object.freeze({
    packages: Object.freeze(["specta-macros@2.0.0-rc.18"]),
    licenseIds: Object.freeze(["MIT"]),
    fileName: "specta-macros-2.0.0-rc.18-LICENSE.txt",
    url: "https://raw.githubusercontent.com/oscartbeaumont/specta/60ffc274e4d83134d4f57ea6df7dec8f5d72ecb6/LICENSE",
    sha256: "9ec86f39e235fcd35e6d91ef3fd4d6b88e5763f3ce1e296c222969986b6f6475",
  }),
  Object.freeze({
    packages: Object.freeze(["tauri-specta@2.0.0-rc.21"]),
    licenseIds: Object.freeze(["MIT"]),
    fileName: "tauri-specta-2.0.0-rc.21-LICENSE.txt",
    url: "https://raw.githubusercontent.com/oscartbeaumont/tauri-specta/5cd56fe8a8d681ba5b649dc7fd4e2e2b001e4e57/LICENSE",
    sha256: "79b0c7d284efc4c2dc8da73c01a6d10c19caf61e70fca813e642dc010049eee7",
  }),
  Object.freeze({
    packages: Object.freeze(["tauri-specta-macros@2.0.0-rc.16"]),
    licenseIds: Object.freeze(["MIT"]),
    fileName: "tauri-specta-macros-2.0.0-rc.16-LICENSE.txt",
    url: "https://raw.githubusercontent.com/oscartbeaumont/tauri-specta/6cb4dfbb7d58ff72c83f1b4bdc8471ffd4609c24/LICENSE",
    sha256: "79b0c7d284efc4c2dc8da73c01a6d10c19caf61e70fca813e642dc010049eee7",
  }),
]);

// A few published crates declare a permissive license but omit a complete
// whole-project copyright notice from both the crate archive and pinned source
// revision. Preserve the exact package metadata without inventing ownership.
export const retainedMetadataAttributions = Object.freeze([
  Object.freeze({
    package: "block2@0.6.2",
    licenseIds: Object.freeze(["MIT"]),
    declaredLicense: "MIT",
    authors: Object.freeze(["Mads Marquart <mads@marquart.dk>"]),
    repository: "https://github.com/madsmtm/objc2",
    vcsCommit: "b4167b582b2f75f9a1be75495c41b765344fd03c",
    limitation:
      "Upstream supplies an exact MIT licensing statement and package " +
      "authors but no explicit copyright line; no ownership is inferred.",
  }),
  Object.freeze({
    package: "dpi@0.1.2",
    licenseIds: Object.freeze(["MIT"]),
    declaredLicense: "Apache-2.0 AND MIT",
    authors: Object.freeze([]),
    repository: "https://github.com/rust-windowing/winit",
    vcsCommit: "587ade844dfb0eada3696ba1cb263c66eea80581",
    limitation:
      "Upstream supplies Apache-2.0 and libm-derived MIT files but no " +
      "whole-project MIT copyright line or package authors; none is inferred.",
  }),
  Object.freeze({
    package: "objc2@0.6.4",
    licenseIds: Object.freeze(["MIT"]),
    declaredLicense: "MIT",
    authors: Object.freeze(["Mads Marquart <mads@marquart.dk>"]),
    repository: "https://github.com/madsmtm/objc2",
    vcsCommit: "8852b424193ca41602281b3d7540d7c8ed51e49a",
    limitation:
      "Upstream supplies an exact MIT licensing statement and package " +
      "authors but no explicit copyright line; no ownership is inferred.",
  }),
  Object.freeze({
    package: "objc2-encode@4.1.0",
    licenseIds: Object.freeze(["MIT"]),
    declaredLicense: "MIT",
    authors: Object.freeze(["Mads Marquart <mads@marquart.dk>"]),
    repository: "https://github.com/madsmtm/objc2",
    vcsCommit: "8d214f5477365ffcbcbb7de058c86ed9a518efb7",
    limitation:
      "Upstream supplies an exact MIT licensing statement and package " +
      "authors but no explicit copyright line; no ownership is inferred.",
  }),
  Object.freeze({
    package: "objc2-foundation@0.3.2",
    licenseIds: Object.freeze(["MIT"]),
    declaredLicense: "MIT",
    authors: Object.freeze([]),
    repository: "https://github.com/madsmtm/objc2",
    vcsCommit: "7b1abfd750a2cacaea71d6a56ecfb83cb7de560b",
    limitation:
      "Upstream supplies an exact MIT licensing statement but no package " +
      "authors or explicit copyright line; none is inferred.",
  }),
  Object.freeze({
    package: "simd_helpers@0.1.0",
    licenseIds: Object.freeze(["MIT"]),
    declaredLicense: "MIT",
    authors: Object.freeze(["Luca Barbato <lu_zero@gentoo.org>"]),
    repository: "https://github.com/lu-zero/simd_helpers",
    vcsCommit: "ca1a2f84aa386d758e98f8a609d990263932fb85",
    limitation:
      "Upstream declares MIT and supplies package authors but no license " +
      "file or explicit copyright line; no copyright ownership is inferred.",
  }),
]);

export const retainedArchiveAttributions = Object.freeze([
  Object.freeze({
    package: "exr@1.74.0",
    sourceFile: "LICENSE.md",
    licenseIds: Object.freeze(["BSD-3-Clause"]),
    sha256: "97e4d3aa7a9e8ac31979c16ca8bb2b628a9cb72c3362185ad2bf4ad77f43551f",
  }),
]);

function sha256(contents) {
  return createHash("sha256").update(contents).digest("hex");
}

async function verifiedContents(source, contents) {
  const actual = sha256(contents);
  if (actual !== source.sha256) {
    throw new Error(
      `${source.fileName} SHA-256 mismatch: expected ${source.sha256}, got ${actual}`,
    );
  }
  return contents;
}

export async function verifyRetainedRemoteLicenses() {
  await Promise.all(
    retainedRemoteLicenses.map(async (source) => {
      const contents = await readFile(path.join(retainedRoot, source.fileName));
      await verifiedContents(source, contents);
    }),
  );
}

async function fetchRetainedRemoteLicenses() {
  return await Promise.all(
    retainedRemoteLicenses.map(async (source) => {
      const response = await fetch(source.url, { redirect: "follow" });
      if (!response.ok) {
        throw new Error(
          `could not fetch ${source.url}: HTTP ${response.status}`,
        );
      }
      const contents = Buffer.from(await response.arrayBuffer());
      return [source, await verifiedContents(source, contents)];
    }),
  );
}

async function main() {
  const args = process.argv.slice(2);
  if (args.length !== 1 || !["--check", "--write"].includes(args[0])) {
    throw new Error("usage: retained-license-sources.mjs --check|--write");
  }
  if (args[0] === "--check") {
    await verifyRetainedRemoteLicenses();
    process.stdout.write("retained remote license sources are current\n");
    return;
  }

  for (const [source, contents] of await fetchRetainedRemoteLicenses()) {
    await writeFile(path.join(retainedRoot, source.fileName), contents);
  }
  process.stdout.write("downloaded checksum-verified retained license sources\n");
}

if (process.argv[1] && path.resolve(process.argv[1]) === fileURLToPath(import.meta.url)) {
  await main();
}
