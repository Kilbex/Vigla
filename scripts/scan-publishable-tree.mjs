#!/usr/bin/env node

import { spawnSync } from "node:child_process";
import {
  copyFile,
  lstat,
  mkdir,
  mkdtemp,
  readlink,
  rm,
  writeFile,
} from "node:fs/promises";
import { tmpdir } from "node:os";
import path from "node:path";
import { fileURLToPath } from "node:url";

const repoRoot = path.resolve(path.dirname(fileURLToPath(import.meta.url)), "..");

function run(command, args, options = {}) {
  const result = spawnSync(command, args, {
    cwd: repoRoot,
    encoding: "utf8",
    ...options,
  });
  if (result.error) throw result.error;
  return result;
}

const staged = run("git", ["diff", "--cached", "--quiet"]);
if (staged.status !== 0) {
  throw new Error(
    "publishable-tree scan requires an unstaged index; commit or unstage first",
  );
}

const listed = run(
  "git",
  ["ls-files", "--cached", "--others", "--exclude-standard", "-z"],
  { encoding: "buffer", maxBuffer: 64 * 1024 * 1024 },
);
if (listed.status !== 0) {
  throw new Error(`git ls-files failed with status ${listed.status}`);
}

const scanRoot = await mkdtemp(path.join(tmpdir(), "vigla-publish-scan-"));
try {
  const files = listed.stdout.toString("utf8").split("\0").filter(Boolean);
  for (const relativePath of files) {
    const sourcePath = path.resolve(repoRoot, relativePath);
    if (
      path.isAbsolute(relativePath) ||
      (!sourcePath.startsWith(`${repoRoot}${path.sep}`) && sourcePath !== repoRoot)
    ) {
      throw new Error(`git returned an unsafe path: ${JSON.stringify(relativePath)}`);
    }

    let metadata;
    try {
      metadata = await lstat(sourcePath);
    } catch (error) {
      if (error?.code === "ENOENT") continue;
      throw error;
    }
    const destinationPath = path.join(scanRoot, relativePath);
    await mkdir(path.dirname(destinationPath), { recursive: true });
    if (metadata.isSymbolicLink()) {
      // Scan the published Git blob (the link target string), never whatever a
      // local symlink happens to reference outside the repository.
      await writeFile(destinationPath, await readlink(sourcePath), "utf8");
    } else if (metadata.isFile()) {
      await copyFile(sourcePath, destinationPath);
    }
  }

  const scan = spawnSync(
    "gitleaks",
    ["dir", scanRoot, "--redact", "--no-banner"],
    { cwd: repoRoot, stdio: "inherit" },
  );
  if (scan.error) throw scan.error;
  if (scan.status !== 0) process.exitCode = scan.status ?? 1;
} finally {
  await rm(scanRoot, { recursive: true, force: true });
}
