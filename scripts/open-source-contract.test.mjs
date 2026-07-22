import assert from "node:assert/strict";
import { execFile } from "node:child_process";
import { mkdtemp, readFile, rm, writeFile } from "node:fs/promises";
import { tmpdir } from "node:os";
import path from "node:path";
import test from "node:test";
import { fileURLToPath } from "node:url";
import { promisify } from "node:util";
import { issueSeeds, labels } from "../.github/issue-seeds.mjs";

const repoUrl = "https://github.com/Kilbex/Vigla";
const rootCargo = await readFile(new URL("../Cargo.toml", import.meta.url), "utf8");
const appPackage = JSON.parse(
  await readFile(new URL("../app/package.json", import.meta.url), "utf8"),
);
const ciWorkflow = await readFile(
  new URL("../.github/workflows/ci.yml", import.meta.url),
  "utf8",
);
const gitignore = await readFile(new URL("../.gitignore", import.meta.url), "utf8");
const governance = await readFile(
  new URL("../GOVERNANCE.md", import.meta.url),
  "utf8",
);
const newcomerGuide = await readFile(
  new URL("../docs/GOOD_FIRST_ISSUES.md", import.meta.url),
  "utf8",
);
const launchGuide = await readFile(
  new URL("../docs/operations/github-launch.md", import.meta.url),
  "utf8",
);
const issueForms = await Promise.all(
  ["bug_report.yml", "feature_request.yml", "adapter_task.yml"].map((name) =>
    readFile(new URL(`../.github/ISSUE_TEMPLATE/${name}`, import.meta.url), "utf8"),
  ),
);
const githubBootstrap = await readFile(
  new URL("./bootstrap-github.mjs", import.meta.url),
  "utf8",
);
const publishableTreeScanner = await readFile(
  new URL("./scan-publishable-tree.mjs", import.meta.url),
  "utf8",
);
const execFileAsync = promisify(execFile);

test("workspace and private app publish canonical project metadata", () => {
  assert.match(rootCargo, /^repository = "https:\/\/github\.com\/Kilbex\/Vigla"$/m);
  assert.match(rootCargo, /^homepage = "https:\/\/kilbex\.github\.io\/Vigla\/"$/m);
  assert.match(rootCargo, /^readme = "README\.md"$/m);

  assert.equal(appPackage.private, true);
  assert.equal(appPackage.license, "Apache-2.0");
  assert.equal(appPackage.repository?.url, `${repoUrl}.git`);
  assert.equal(appPackage.homepage, "https://kilbex.github.io/Vigla/");
  assert.equal(appPackage.engines?.node, ">=22");
  assert.equal(appPackage.engines?.pnpm, ">=10");
});

test("CI regenerates TypeScript bindings and rejects drift", () => {
  assert.match(
    ciWorkflow,
    /VIGLA_REGEN_BINDINGS=1 cargo test -p vigla-host --lib regenerate_typescript_bindings/,
  );
  assert.match(ciWorkflow, /git diff --exit-code -- app\/src\/bindings\.ts/);
});

test("CI regenerates the lock-bound Rust dependency license report", () => {
  assert.match(ciWorkflow, /tool:\s*cargo-about@0\.9\.1/);
  assert.match(
    ciWorkflow,
    /cargo about generate --workspace --all-features --locked --fail/,
  );
  assert.match(ciWorkflow, /--check-rust-report/);
});

test("CI scans the complete reachable history with a checksum-pinned secret scanner", () => {
  assert.match(ciWorkflow, /GITLEAKS_VERSION:\s*"8\.30\.1"/);
  assert.match(
    ciWorkflow,
    /GITLEAKS_SHA256:\s*"551f6fc83ea457d62a0d98237cbad105af8d557003051f41f3e7ca7b3f2470eb"/,
  );
  assert.match(ciWorkflow, /^\s+fetch-depth:\s*0\s*$/m);
  assert.match(ciWorkflow, /gitleaks git --redact --no-banner/);
});

test("the local secret scan covers exactly the publishable worktree", () => {
  assert.match(
    publishableTreeScanner,
    /"ls-files", "--cached", "--others", "--exclude-standard", "-z"/,
  );
  assert.match(publishableTreeScanner, /"--redact", "--no-banner"/);
  assert.match(launchGuide, /node scripts\/scan-publishable-tree\.mjs/);
  assert.doesNotMatch(launchGuide, /gitleaks dir --redact --no-banner \./);
});

test("secret-prone local files are ignored while the public demo env is allowed", () => {
  for (const pattern of [
    ".env",
    "**/.env.*",
    "*.pem",
    "*.p12",
    "*.mobileprovision",
    "credentials.json",
    "**/.vigla/",
    "*.sqlite-shm",
    "*.sqlite-wal",
  ]) {
    assert.ok(gitignore.split("\n").includes(pattern), `missing ${pattern}`);
  }
  assert.match(gitignore, /^!app\/\.env\.webdemo$/m);
});

test("governance documents authority, maintainer changes, conflicts, and succession", () => {
  for (const heading of [
    "## Decision making",
    "## Maintainer lifecycle",
    "## Conflicts and appeals",
    "## Succession",
  ]) {
    assert.match(governance, new RegExp(`^${heading}$`, "m"));
  }
});

test("launch seeds advertise only current work and canonical labels", () => {
  const titles = new Set(issueSeeds.map((issue) => issue.title));
  assert.equal(titles.size, issueSeeds.length, "issue seed titles must be unique");
  assert.equal(titles.has("test: preserve replay across an unknown future event"), false);
  assert.doesNotMatch(newcomerGuide, /Improve screenshot alt text/);
  assert.doesNotMatch(newcomerGuide, /unknown future event/);

  const knownLabels = new Set(labels.map(([name]) => name));
  for (const issue of issueSeeds) {
    for (const label of issue.labels) {
      assert.ok(knownLabels.has(label), `${issue.title} uses unknown label ${label}`);
    }
  }
  for (const form of issueForms) {
    const encoded = form.match(/^labels:\s*(\[[^\n]+\])$/m)?.[1];
    assert.ok(encoded, "issue form must declare an inline label list");
    for (const label of JSON.parse(encoded)) {
      assert.ok(knownLabels.has(label), `issue form uses unknown label ${label}`);
    }
  }
  assert.match(newcomerGuide, /`documentation`/);
  assert.match(launchGuide, new RegExp(`${issueSeeds.length} real issues`));
});

test("the owner bootstrap enables the disclosure route claimed by SECURITY.md", () => {
  assert.match(
    githubBootstrap,
    /"PUT",\s*`repos\/\$\{repository\}\/private-vulnerability-reporting`/,
  );
});

test("the owner bootstrap checks every existing issue before seeding", () => {
  assert.match(githubBootstrap, /"api",\s*"--paginate"/);
  assert.match(
    githubBootstrap,
    /repos\/\$\{repository\}\/issues\?state=all&per_page=100/,
  );
  assert.match(
    githubBootstrap,
    /select\(has\(\"pull_request\"\) \| not\)/,
  );
  assert.doesNotMatch(githubBootstrap, /contains\(\"\/pull\/\"\)/);
  assert.doesNotMatch(githubBootstrap, /--limit",\s*"200/);
  assert.match(githubBootstrap, /\{ capture: true, readOnly: true \}/);
  assert.match(githubBootstrap, /existingTitles\.add\(issue\.title\)/);
});

test("the owner bootstrap dry-run faithfully skips existing issues", async () => {
  const stubRoot = await mkdtemp(path.join(tmpdir(), "vigla-gh-stub-"));
  const stubPath = path.join(stubRoot, "gh");
  const existingTitle = issueSeeds[0].title;
  await writeFile(
    stubPath,
    `#!/usr/bin/env node\nprocess.stdout.write(process.env.VIGLA_TEST_ISSUE_TITLE + "\\n");\n`,
    { mode: 0o700 },
  );

  try {
    const { stdout } = await execFileAsync(
      process.execPath,
      [fileURLToPath(new URL("./bootstrap-github.mjs", import.meta.url)), "--dry-run"],
      {
        env: {
          ...process.env,
          PATH: `${stubRoot}${path.delimiter}${process.env.PATH ?? ""}`,
          VIGLA_TEST_ISSUE_TITLE: existingTitle,
        },
      },
    );
    assert.match(stdout, new RegExp(`^exists: ${escapeRegExp(existingTitle)}$`, "m"));
    assert.doesNotMatch(
      stdout,
      new RegExp(`gh issue create[^\n]+${escapeRegExp(existingTitle)}`),
    );
  } finally {
    await rm(stubRoot, { recursive: true, force: true });
  }
});

function escapeRegExp(value) {
  return value.replace(/[.*+?^${}()|[\]\\]/g, "\\$&");
}
