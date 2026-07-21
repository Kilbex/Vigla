#!/usr/bin/env node

import { readdir, readFile, stat } from "node:fs/promises";
import path from "node:path";
import { fileURLToPath } from "node:url";

const repoRoot = path.resolve(path.dirname(fileURLToPath(import.meta.url)), "..");
const ignoredDirectories = new Set([
  ".git",
  ".claude",
  ".private",
  ".superpowers",
  "dist",
  "dist-webdemo",
  "node_modules",
  "target",
  "test-results",
]);

async function markdownFiles(directory) {
  const entries = await readdir(directory, { withFileTypes: true });
  const nested = await Promise.all(
    entries.map(async (entry) => {
      const entryPath = path.join(directory, entry.name);
      if (entry.isDirectory()) {
        return ignoredDirectories.has(entry.name) ? [] : markdownFiles(entryPath);
      }
      return entry.isFile() && entry.name.endsWith(".md") ? [entryPath] : [];
    }),
  );
  return nested.flat();
}

function githubSlug(text) {
  return text
    .trim()
    .toLowerCase()
    .replace(/<[^>]*>/g, "")
    .replace(/[`*_~]/g, "")
    .replace(/[^\p{Letter}\p{Number}\s-]/gu, "")
    .replace(/\s+/g, "-");
}

function markdownAnchors(markdown) {
  const seen = new Map();
  const anchors = new Set();
  for (const line of markdown.split("\n")) {
    const match = line.match(/^ {0,3}#{1,6}\s+(.+?)\s*#*\s*$/);
    if (!match) continue;
    const base = githubSlug(match[1]);
    if (!base) continue;
    const count = seen.get(base) ?? 0;
    seen.set(base, count + 1);
    anchors.add(count === 0 ? base : `${base}-${count}`);
  }
  return anchors;
}

function lineNumber(source, index) {
  return source.slice(0, index).split("\n").length;
}

function destination(raw) {
  const trimmed = raw.trim();
  if (trimmed.startsWith("<")) {
    const end = trimmed.indexOf(">");
    return end === -1 ? trimmed : trimmed.slice(1, end);
  }
  return trimmed.split(/\s+["']/u, 1)[0];
}

const files = await markdownFiles(repoRoot);
const failures = [];
const contentCache = new Map();

for (const filePath of files) {
  const markdown = await readFile(filePath, "utf8");
  for (const match of markdown.matchAll(/!?\[[^\]\n]*\]\(([^)\n]+)\)/g)) {
    const target = destination(match[1]);
    if (
      !target ||
      /^(?:https?:|mailto:)/i.test(target) ||
      target.startsWith("data:")
    ) {
      continue;
    }

    let decoded;
    try {
      decoded = decodeURIComponent(target);
    } catch {
      failures.push(
        `${path.relative(repoRoot, filePath)}:${lineNumber(markdown, match.index)} invalid URL encoding: ${target}`,
      );
      continue;
    }

    const hashIndex = decoded.indexOf("#");
    const relativeTarget = hashIndex === -1 ? decoded : decoded.slice(0, hashIndex);
    const fragment = hashIndex === -1 ? "" : decoded.slice(hashIndex + 1);
    const resolved = relativeTarget
      ? path.resolve(path.dirname(filePath), relativeTarget)
      : filePath;

    if (resolved !== repoRoot && !resolved.startsWith(`${repoRoot}${path.sep}`)) {
      failures.push(
        `${path.relative(repoRoot, filePath)}:${lineNumber(markdown, match.index)} escapes the repository: ${target}`,
      );
      continue;
    }

    let metadata;
    try {
      metadata = await stat(resolved);
    } catch {
      failures.push(
        `${path.relative(repoRoot, filePath)}:${lineNumber(markdown, match.index)} missing target: ${target}`,
      );
      continue;
    }

    if (relativeTarget.endsWith("/") && !metadata.isDirectory()) {
      failures.push(
        `${path.relative(repoRoot, filePath)}:${lineNumber(markdown, match.index)} expected directory: ${target}`,
      );
      continue;
    }

    if (!fragment || metadata.isDirectory()) continue;
    if (path.extname(resolved).toLowerCase() !== ".md") continue;

    let targetMarkdown = contentCache.get(resolved);
    if (targetMarkdown === undefined) {
      targetMarkdown = await readFile(resolved, "utf8");
      contentCache.set(resolved, targetMarkdown);
    }
    if (!markdownAnchors(targetMarkdown).has(fragment.toLowerCase())) {
      failures.push(
        `${path.relative(repoRoot, filePath)}:${lineNumber(markdown, match.index)} missing heading #${fragment} in ${path.relative(repoRoot, resolved)}`,
      );
    }
  }
}

if (failures.length > 0) {
  process.stderr.write(`${failures.join("\n")}\n`);
  process.exitCode = 1;
} else {
  process.stdout.write(
    `checked ${files.length} Markdown files; local links and heading anchors resolve\n`,
  );
}
