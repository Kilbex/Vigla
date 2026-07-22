#!/usr/bin/env node

import { spawnSync } from "node:child_process";
import { issueSeeds, labels } from "../.github/issue-seeds.mjs";

const argv = process.argv.slice(2);
const dryRunIndex = argv.indexOf("--dry-run");
const dryRun = dryRunIndex !== -1;
if (dryRun) argv.splice(dryRunIndex, 1);

let repository = "Kilbex/Vigla";
const repoIndex = argv.indexOf("--repo");
if (repoIndex !== -1) {
  repository = argv[repoIndex + 1] ?? "";
  argv.splice(repoIndex, 2);
}
if (argv.length > 0 || !/^[^/]+\/[^/]+$/.test(repository)) {
  throw new Error("usage: node scripts/bootstrap-github.mjs [--dry-run] [--repo owner/name]");
}

function quote(value) {
  return /[^A-Za-z0-9_./:@=-]/.test(value) ? JSON.stringify(value) : value;
}

function gh(args, { capture = false, readOnly = false } = {}) {
  if (dryRun && !readOnly) {
    process.stdout.write(`[dry-run] gh ${args.map(quote).join(" ")}\n`);
    return "";
  }
  const result = spawnSync("gh", args, {
    encoding: "utf8",
    stdio: capture ? ["ignore", "pipe", "inherit"] : "inherit",
  });
  if (result.error) throw result.error;
  if (result.status !== 0) {
    throw new Error(`gh ${args[0]} failed with status ${result.status}`);
  }
  return result.stdout ?? "";
}

const description =
  "Mission control for AI coding agents — run Claude Code, Codex, and Antigravity as one supervised, auditable, one-click-revertable team. Local-first macOS app.";
const topics = [
  "ai-agents",
  "agent-orchestration",
  "claude-code",
  "codex",
  "antigravity",
  "coding-assistant",
  "multi-agent",
  "developer-tools",
  "rust",
  "tauri",
  "react",
  "macos",
  "llm",
  "local-first",
];

gh([
  "repo",
  "edit",
  repository,
  "--description",
  description,
  "--homepage",
  "https://kilbex.github.io/Vigla/",
  "--enable-issues",
  "--enable-discussions",
  "--enable-squash-merge",
  "--enable-merge-commit=false",
  "--enable-rebase-merge=false",
  "--delete-branch-on-merge",
  "--allow-update-branch",
  ...topics.flatMap((topic) => ["--add-topic", topic]),
]);

gh([
  "api",
  "--method",
  "PUT",
  `repos/${repository}/private-vulnerability-reporting`,
]);

for (const [name, color, descriptionText] of labels) {
  gh([
    "label",
    "create",
    name,
    "--repo",
    repository,
    "--color",
    color,
    "--description",
    descriptionText,
    "--force",
  ]);
}

const existingTitles = new Set(
  gh(
    [
      "api",
      "--paginate",
      `repos/${repository}/issues?state=all&per_page=100`,
      "--jq",
      '.[] | select(has("pull_request") | not) | .title',
    ],
    { capture: true, readOnly: true },
  )
    .split("\n")
    .filter(Boolean),
);

for (const issue of issueSeeds) {
  if (existingTitles.has(issue.title)) {
    process.stdout.write(`exists: ${issue.title}\n`);
    continue;
  }
  gh([
    "issue",
    "create",
    "--repo",
    repository,
    "--title",
    issue.title,
    "--body",
    issue.body,
    ...issue.labels.flatMap((label) => ["--label", label]),
  ]);
  existingTitles.add(issue.title);
}

process.stdout.write(
  `${dryRun ? "validated" : "configured"} ${labels.length} labels and ${issueSeeds.length} issue seeds for ${repository}\n`,
);
