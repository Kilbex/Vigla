#!/usr/bin/env node

import { spawnSync } from "node:child_process";
import path from "node:path";
import { fileURLToPath } from "node:url";
import { issueSeeds, labels } from "../.github/issue-seeds.mjs";

/**
 * Public repository metadata. These are exported rather than inlined into the
 * `gh` invocation so `scripts/open-source-contract.test.mjs` can hold them to
 * GitHub's own limits; a description or topic list that only exists inside an
 * owner-run script drifts silently from whatever the launch docs claim.
 */
export const description =
  "Mission control for AI coding agents — run Claude Code, Codex, Antigravity, and more as one supervised team. Every submission audited; every mission reversible.";

export const topics = [
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
  "llm-agents",
  "autonomous-agents",
  "code-review",
  "developer-productivity",
  "desktop-app",
  "git-worktrees",
];

/**
 * Issues GitHub shows above the issue list. GitHub caps pins at three, and a
 * pin can only reference an issue this bootstrap actually seeds — the contract
 * test enforces both, so a renamed seed cannot leave a dangling pin.
 */
export const pinnedTitles = [
  "roadmap: Linux desktop packaging and parity",
  "roadmap: decide and prove the Windows execution boundary",
  "docs: specify an OpenCode adapter before adding a crate",
];

/**
 * Checks the `main` ruleset requires. Each entry is the `name:` of a job in
 * `.github/workflows/ci.yml`; GitHub matches required checks against that
 * display name, so a renamed job silently stops being required. The contract
 * test pins these to the workflow file.
 */
export const requiredChecks = [
  "Rust",
  "Frontend",
  "Browser E2E",
  "Portable Rust (Linux)",
  "Rust docs",
  "Supply chain",
];

const homepage = "https://kilbex.github.io/Vigla/";

// A read-only probe that cannot reach GitHub must not stall a preview.
const probeTimeoutMs = 20_000;

function mainRuleset() {
  return {
    name: "main",
    target: "branch",
    enforcement: "active",
    // Repository-admin role. `pull_request` mode lets the sole maintainer clear
    // the impossible self-review after every required check passes; it does not
    // exempt them from the checks themselves.
    bypass_actors: [
      { actor_id: 5, actor_type: "RepositoryRole", bypass_mode: "pull_request" },
    ],
    conditions: { ref_name: { include: ["~DEFAULT_BRANCH"], exclude: [] } },
    rules: [
      { type: "deletion" },
      { type: "non_fast_forward" },
      {
        type: "pull_request",
        parameters: {
          required_approving_review_count: 1,
          require_code_owner_review: true,
          dismiss_stale_reviews_on_push: true,
          require_last_push_approval: false,
          required_review_thread_resolution: true,
          allowed_merge_methods: ["squash"],
        },
      },
      {
        type: "required_status_checks",
        parameters: {
          strict_required_status_checks_policy: true,
          do_not_enforce_on_create: false,
          required_status_checks: requiredChecks.map((context) => ({ context })),
        },
      },
    ],
  };
}

const pinStateQuery = `query($owner: String!, $name: String!) {
  repository(owner: $owner, name: $name) {
    pinnedIssues(first: 3) { nodes { issue { id title } } }
    issues(first: 100, states: OPEN) { nodes { id title } }
  }
}`;

const pinMutation = `mutation($issueId: ID!) {
  pinIssue(input: { issueId: $issueId }) { issue { number } }
}`;

// GitHub caps a repository at three pinned issues, so a changed pin list has to
// release the old ones before it can claim the new ones.
const unpinMutation = `mutation($issueId: ID!) {
  unpinIssue(input: { issueId: $issueId }) { issue { number } }
}`;

function quote(value) {
  return /[^A-Za-z0-9_./:@=-]/.test(value) ? JSON.stringify(value) : value;
}

function bootstrap(repository, { dryRun }) {
  /**
   * `readOnly` calls run even during a preview so the preview reflects the live
   * repository. A preview must never *require* the network, though, so during
   * `--dry-run` an unreachable, unauthenticated, or missing `gh` degrades to an
   * empty answer instead of aborting.
   */
  function gh(args, options = {}) {
    const { capture = false, readOnly = false, input, optional = false, what } = options;
    if (dryRun && !readOnly) {
      const body = input === undefined ? "" : ` <<< ${quote(input)}`;
      process.stdout.write(`[dry-run] gh ${args.map(quote).join(" ")}${body}\n`);
      return optional ? null : "";
    }
    const tolerant = optional || (readOnly && dryRun);
    const stream = tolerant ? "pipe" : "inherit";
    const result = spawnSync("gh", args, {
      encoding: "utf8",
      input,
      stdio: ["pipe", capture ? "pipe" : stream, stream],
      timeout: readOnly ? probeTimeoutMs : undefined,
    });
    if (result.error || result.status !== 0) {
      if (readOnly && dryRun) {
        process.stdout.write(`[dry-run] offline: assuming no ${what} yet\n`);
        return "";
      }
      if (optional) return null;
      if (result.error) throw result.error;
      throw new Error(`gh ${args[0]} failed with status ${result.status}`);
    }
    return result.stdout ?? "";
  }

  function call(method, endpoint, body, options) {
    const args = ["api", "--method", method, endpoint];
    if (body !== undefined) args.push("--input", "-");
    const input = body === undefined ? undefined : `${JSON.stringify(body)}\n`;
    return gh(args, { ...options, input });
  }

  gh([
    "repo",
    "edit",
    repository,
    "--description",
    description,
    "--homepage",
    homepage,
    "--enable-issues",
    "--enable-discussions",
    "--enable-squash-merge",
    "--enable-merge-commit=false",
    "--enable-rebase-merge=false",
    "--delete-branch-on-merge",
    "--allow-update-branch",
  ]);

  // `gh repo edit --add-topic` only ever adds, so a topic dropped from the list
  // above would linger on the repository forever. The topics endpoint replaces
  // the whole set, which makes re-running converge instead of accumulate.
  call("PUT", `repos/${repository}/topics`, { names: topics });

  call("PUT", `repos/${repository}/private-vulnerability-reporting`);
  call("PUT", `repos/${repository}/vulnerability-alerts`);
  call("PUT", `repos/${repository}/automated-security-fixes`);

  // GitHub offers no upsert for Pages: creating a site that already exists
  // returns 409 and updating one that does not exist returns 404. Create, then
  // update, so either starting state converges on the Actions build type.
  const pages = { build_type: "workflow" };
  if (call("POST", `repos/${repository}/pages`, pages, { optional: true }) === null) {
    call("PUT", `repos/${repository}/pages`, pages);
  }

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
      { capture: true, readOnly: true, what: "issues" },
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

  // Pinning is cosmetic; branch protection is not. Don't let a GraphQL hiccup
  // on the former leave `main` unprotected.
  try {
    pinIssues(gh, repository, dryRun);
  } catch (error) {
    process.stderr.write(
      `warning: could not pin issues (${error.message}); pin them by hand and re-run\n`,
    );
  }
  applyRuleset(gh, call, repository);

  const verb = dryRun ? "validated" : "configured";
  process.stdout.write(
    `${verb} ${labels.length} labels, ${issueSeeds.length} issue seeds, ` +
      `${pinnedTitles.length} pins and ${requiredChecks.length} required checks ` +
      `for ${repository}\n`,
  );
}

// Pinning is GraphQL-only, and re-pinning an already-pinned issue is an error,
// so read the current pins first and pin only what is missing.
function pinIssues(gh, repository, dryRun) {
  const [owner, name] = repository.split("/");
  const raw = gh(
    [
      "api",
      "graphql",
      "-f",
      `query=${pinStateQuery}`,
      "-F",
      `owner=${owner}`,
      "-F",
      `name=${name}`,
    ],
    { capture: true, readOnly: true, what: "pinned issues" },
  );
  const repositoryState = raw ? JSON.parse(raw).data.repository : null;
  const currentPins =
    repositoryState?.pinnedIssues.nodes.map((node) => node.issue) ?? [];
  const pinned = new Set(currentPins.map((issue) => issue.title));
  const identifiers = new Map(
    repositoryState?.issues.nodes.map((node) => [node.title, node.id]) ?? [],
  );

  // Release pins this run no longer wants first — three is a hard cap, so
  // without this a changed list would fail at the fourth pin.
  for (const issue of currentPins) {
    if (pinnedTitles.includes(issue.title)) continue;
    gh(["api", "graphql", "-f", `query=${unpinMutation}`, "-F", `issueId=${issue.id}`]);
    process.stdout.write(`${dryRun ? "[dry-run] " : ""}unpinned: ${issue.title}\n`);
  }

  for (const title of pinnedTitles) {
    if (pinned.has(title)) {
      process.stdout.write(`pinned: ${title}\n`);
      continue;
    }
    const issueId = identifiers.get(title);
    if (issueId === undefined) {
      // The seeding pass above created it in this same run, or the preview
      // could not reach GitHub; either way there is no node id to pin yet.
      process.stdout.write(`${dryRun ? "[dry-run] " : ""}pin pending: ${title}\n`);
      continue;
    }
    gh(["api", "graphql", "-f", `query=${pinMutation}`, "-F", `issueId=${issueId}`]);
  }
}

// Rulesets are created by POST and updated by PUT against their numeric id, so
// look the ruleset up by name before deciding which one to issue.
function applyRuleset(gh, call, repository) {
  const ruleset = mainRuleset();
  const raw = gh(
    [
      "api",
      `repos/${repository}/rulesets`,
      "--jq",
      `.[] | select(.name == "${ruleset.name}") | .id`,
    ],
    { capture: true, readOnly: true, what: "ruleset" },
  ).trim();
  if (raw) {
    // PUT replaces the whole ruleset. The bypass list is the one part an owner
    // is told to verify and repair by hand (docs/operations/github-launch.md),
    // because the numeric RepositoryRole ids are undocumented — so carry the
    // live list forward rather than silently reverting that repair.
    const live = gh(
      [
        "api",
        `repos/${repository}/rulesets/${raw}`,
        "--jq",
        ".bypass_actors",
      ],
      { capture: true, readOnly: true, what: "ruleset bypass actors" },
    ).trim();
    const bypassActors = live ? JSON.parse(live) : null;
    call("PUT", `repos/${repository}/rulesets/${raw}`, {
      ...ruleset,
      ...(bypassActors?.length ? { bypass_actors: bypassActors } : {}),
    });
    return;
  }
  call("POST", `repos/${repository}/rulesets`, ruleset);
}

function main(argv) {
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

  bootstrap(repository, { dryRun });
}

if (process.argv[1] && path.resolve(process.argv[1]) === fileURLToPath(import.meta.url)) {
  main(process.argv.slice(2));
}
