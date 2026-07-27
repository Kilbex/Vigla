import assert from "node:assert/strict";
import { execFile } from "node:child_process";
import { mkdtemp, readFile, rm, writeFile } from "node:fs/promises";
import { tmpdir } from "node:os";
import path from "node:path";
import test from "node:test";
import { fileURLToPath } from "node:url";
import { promisify } from "node:util";
import { issueSeeds, labels } from "../.github/issue-seeds.mjs";
import {
  description,
  pinnedTitles,
  requiredChecks,
  topics,
} from "./bootstrap-github.mjs";

const repoUrl = "https://github.com/Kilbex/Vigla";
const rootCargo = await readFile(new URL("../Cargo.toml", import.meta.url), "utf8");
const appPackage = JSON.parse(
  await readFile(new URL("../app/package.json", import.meta.url), "utf8"),
);
const ciWorkflow = await readFile(
  new URL("../.github/workflows/ci.yml", import.meta.url),
  "utf8",
);
const regenerateWorkflow = await readFile(
  new URL("../.github/workflows/regenerate-notices.yml", import.meta.url),
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
const rustToolchain = await readFile(
  new URL("../rust-toolchain.toml", import.meta.url),
  "utf8",
);
const readme = await readFile(new URL("../README.md", import.meta.url), "utf8");
const creatorKit = await readFile(
  new URL("../docs/operations/creator-kit.md", import.meta.url),
  "utf8",
);
const siteHtml = await readFile(new URL("../site/index.html", import.meta.url), "utf8");
const contributing = await readFile(
  new URL("../CONTRIBUTING.md", import.meta.url),
  "utf8",
);
const eventSchemaLib = await readFile(
  new URL("../crates/event-schema/src/lib.rs", import.meta.url),
  "utf8",
);
const adapterCoreLib = await readFile(
  new URL("../crates/adapters/core/src/lib.rs", import.meta.url),
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

test("the notice-regeneration workflow stays maintainer-triggered and pin-identical", () => {
  // A lockfile bump reddens four checks that Dependabot's read-only token can
  // never repair. The escape hatch must not become the usual privilege-
  // escalation shape: no pull_request_target, and no write token handed to a
  // bot-authored branch without a maintainer dispatching the run.
  assert.doesNotMatch(regenerateWorkflow, /^\s*pull_request_target:/m);
  assert.match(regenerateWorkflow, /^on:\n {2}workflow_dispatch:\n/m);
  assert.match(regenerateWorkflow, /tool: cargo-about@0\.9\.1/);
  assert.match(regenerateWorkflow, /--write-rust-report/);
  assert.match(regenerateWorkflow, /pnpm -C app licenses:write/);

  // Both workflows must install the same cargo-about; a different one writes a
  // different report and the branch stays red.
  const pinned = new Set(
    [ciWorkflow, regenerateWorkflow].flatMap((workflow) =>
      [...workflow.matchAll(/tool: (cargo-about@\S+)/g)].map(([, tool]) => tool),
    ),
  );
  assert.deepEqual([...pinned], ["cargo-about@0.9.1"]);

  const gate = "Verify retained Rust dependency licenses";
  const stale = ciWorkflow.slice(ciWorkflow.indexOf(gate));
  assert.match(stale, /--write-rust-report \/tmp\/vigla-rust-licenses\.json/);
  assert.match(stale, /workflows\/regenerate-notices\.yml/);
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
  assert.match(githubBootstrap, /\{ capture: true, readOnly: true, what: "issues" \}/);
  assert.match(githubBootstrap, /existingTitles\.add\(issue\.title\)/);
});

test("the owner bootstrap dry-run faithfully skips existing issues and pins", async () => {
  const existingTitle = issueSeeds[0].title;
  const stub = `#!/usr/bin/env node
const args = process.argv.slice(2);
if (args.includes("graphql")) {
  process.stdout.write(JSON.stringify({
    data: {
      repository: {
        pinnedIssues: { nodes: [{ issue: { title: process.env.VIGLA_TEST_PINNED_TITLE } }] },
        issues: { nodes: [{ id: "I_stub", title: process.env.VIGLA_TEST_PINNABLE_TITLE }] },
      },
    },
  }));
} else if (args.some((arg) => arg.endsWith("/rulesets"))) {
  process.stdout.write("");
} else {
  process.stdout.write(process.env.VIGLA_TEST_ISSUE_TITLE + "\\n");
}
`;

  await withGhStub(stub, { VIGLA_TEST_ISSUE_TITLE: existingTitle }, async (run) => {
    const { stdout } = await run({
      VIGLA_TEST_PINNED_TITLE: pinnedTitles[0],
      VIGLA_TEST_PINNABLE_TITLE: pinnedTitles[1],
    });
    assert.match(stdout, new RegExp(`^exists: ${escapeRegExp(existingTitle)}$`, "m"));
    assert.doesNotMatch(
      stdout,
      new RegExp(`gh issue create[^\n]+${escapeRegExp(existingTitle)}`),
    );
    assert.match(stdout, new RegExp(`^pinned: ${escapeRegExp(pinnedTitles[0])}$`, "m"));
    assert.match(stdout, /gh api graphql .*pinIssue/);
    assert.match(stdout, /gh api --method POST [^\n]+\/rulesets/);
  });
});

test("the owner bootstrap dry-run completes without reaching GitHub", async () => {
  const stub = `#!/usr/bin/env node
process.stderr.write("dial tcp: lookup api.github.com: no such host\\n");
process.exit(1);
`;

  await withGhStub(stub, {}, async (run) => {
    const { stdout } = await run({});
    assert.match(stdout, /^\[dry-run\] gh repo edit Kilbex\/Vigla /m);
    assert.match(stdout, /^\[dry-run\] offline: assuming no issues yet$/m);
    assert.match(stdout, /^\[dry-run\] offline: assuming no pinned issues yet$/m);
    assert.match(stdout, /^\[dry-run\] offline: assuming no ruleset yet$/m);
    for (const title of pinnedTitles) {
      assert.match(
        stdout,
        new RegExp(`^\\[dry-run\\] pin pending: ${escapeRegExp(title)}$`, "m"),
      );
    }
    assert.match(stdout, /^validated 21 labels, 11 issue seeds, 3 pins and 6 required checks/m);
  });
});

test("the owner bootstrap applies converging, limit-respecting repository metadata", () => {
  assert.ok(description.length <= 160, `description is ${description.length} characters`);
  assert.ok(topics.length <= 20, `${topics.length} topics exceeds the GitHub cap`);
  assert.equal(new Set(topics).size, topics.length, "topics must be unique");
  for (const topic of topics) {
    assert.match(topic, /^[a-z0-9][a-z0-9-]{0,49}$/, `invalid topic ${topic}`);
  }

  // --add-topic can only ever add, so the set-replacing endpoint is the only
  // way a removed topic ever leaves the repository.
  assert.doesNotMatch(githubBootstrap, /"--add-topic"/);
  assert.match(githubBootstrap, /"PUT",\s*`repos\/\$\{repository\}\/topics`,\s*\{ names: topics \}/);
  assert.match(githubBootstrap, /`repos\/\$\{repository\}\/vulnerability-alerts`/);
  assert.match(githubBootstrap, /`repos\/\$\{repository\}\/automated-security-fixes`/);
  assert.match(githubBootstrap, /`repos\/\$\{repository\}\/pages`/);
  assert.match(githubBootstrap, /build_type: "workflow"/);
});

test("pinned issues and required checks name things that actually exist", () => {
  const seedTitles = new Set(issueSeeds.map((issue) => issue.title));
  assert.ok(pinnedTitles.length <= 3, "GitHub pins at most three issues");
  for (const title of pinnedTitles) {
    assert.ok(seedTitles.has(title), `pinned title is not a seeded issue: ${title}`);
  }

  const jobNames = [...ciWorkflow.matchAll(/^ {4}name: (.+)$/gm)].map(([, name]) => name);
  for (const check of requiredChecks) {
    assert.ok(jobNames.includes(check), `required check is not a ci.yml job name: ${check}`);
  }
  assert.equal(requiredChecks.length, jobNames.length, "every CI job must be required");
});

async function withGhStub(source, baseEnv, body) {
  const stubRoot = await mkdtemp(path.join(tmpdir(), "vigla-gh-stub-"));
  await writeFile(path.join(stubRoot, "gh"), source, { mode: 0o700 });
  try {
    await body((env) =>
      execFileAsync(
        process.execPath,
        [fileURLToPath(new URL("./bootstrap-github.mjs", import.meta.url)), "--dry-run"],
        {
          env: {
            ...process.env,
            ...baseEnv,
            ...env,
            PATH: `${stubRoot}${path.delimiter}${process.env.PATH ?? ""}`,
          },
        },
      ),
    );
  } finally {
    await rm(stubRoot, { recursive: true, force: true });
  }
}

function escapeRegExp(value) {
  return value.replace(/[.*+?^${}()|[\]\\]/g, "\\$&");
}

// --- MSRV parity -----------------------------------------------------
//
// The minimum supported Rust version is written out in four
// machine-read places and three prose ones. Nothing made them agree, so
// bumping one silently un-proved the others: the supply-chain job would
// keep evaluating policy on an old compiler, and the published
// requirements would keep advertising a version nobody checks.
// `Cargo.toml` is the single source of truth; everything else is
// derived from it here.

function declaredMsrv() {
  const match = rootCargo.match(/^rust-version = "(\d+\.\d+\.\d+)"$/m);
  assert.ok(match, "Cargo.toml [workspace.package] must declare rust-version");
  return match[1];
}

function versionParts(version) {
  return version.split(".").map((part) => Number.parseInt(part, 10));
}

function pinnedToolchain() {
  const match = rustToolchain.match(/^channel = "(\d+\.\d+\.\d+)"$/m);
  assert.ok(match, "rust-toolchain.toml must pin an exact channel");
  return match[1];
}

// Return one workflow step's block by its `- name:` heading, so an
// assertion about that step cannot be satisfied by text elsewhere in
// the file. Throws if the step is missing, which is the point: a
// renamed or deleted gate must fail loudly rather than silently stop
// being asserted.
function ciStep(name) {
  const start = ciWorkflow.indexOf(`- name: ${name}\n`);
  assert.notEqual(start, -1, `ci.yml has no step named "${name}"`);
  const rest = ciWorkflow.slice(start + 1);
  const next = rest.search(/^ *- name: /m);
  return next === -1 ? rest : rest.slice(0, next);
}

test("security tooling evaluates policy on exactly the declared MSRV", () => {
  const msrv = declaredMsrv();

  // cargo-deny resolves feature and license policy against a compiler
  // version. Pointing it at a different one means the advisory, ban,
  // and license verdicts describe a toolchain the project does not
  // claim to support.
  const denyRustVersion = ciWorkflow.match(/^ +rust-version: "(\d+\.\d+\.\d+)"$/m);
  assert.ok(denyRustVersion, "ci.yml must pin cargo-deny's rust-version");
  assert.equal(
    denyRustVersion[1],
    msrv,
    "cargo-deny's rust-version must equal Cargo.toml's rust-version",
  );

  // The MSRV must actually be compiled, not merely declared.
  const proof = ciStep("Prove the declared MSRV still compiles");

  // Assert on what the step must *do*, not on its exact shell spelling:
  // a literal pin would redden CI for a harmless reformat, and would
  // still pass if the whole body were commented out.
  const uncommented = proof
    .split("\n")
    .filter((line) => !/^\s*#/.test(line))
    .join("\n");
  // Must be the assignment itself, not any line that happens to name
  // both — the step's own error message mentions `rust-version` and
  // `Cargo.toml`, so a looser match stays green with the extraction
  // commented out.
  assert.match(
    uncommented,
    /^\s*msrv=.*Cargo\.toml/m,
    "the MSRV proof step must read rust-version out of Cargo.toml, not hardcode it",
  );
  assert.doesNotMatch(
    uncommented,
    new RegExp(`\\+${msrv.replace(/\./g, "\\.")}\\b`),
    "the MSRV proof step must not hardcode the version it reads from Cargo.toml",
  );
  const compile = uncommented.match(/^\s*(cargo "\+\$msrv" check .*--exclude vigla-host)$/m);
  assert.ok(
    compile,
    "the MSRV proof step must compile the portable crates on that toolchain",
  );

  // CONTRIBUTING.md promises every gate is reproducible locally, so its
  // copy of this command must be the same command.
  assert.ok(
    contributing.includes(compile[1]),
    `CONTRIBUTING.md must document the MSRV gate as \`${compile[1]}\``,
  );
});

test("the pinned toolchain is never older than the MSRV it must prove", () => {
  // The pin and the MSRV are deliberately independent (see
  // rust-toolchain.toml): bumping the pin does not restrict adopters.
  // Independence only runs one way, though — a pin *older* than the
  // declared minimum means contributors compile with a compiler the
  // project claims not to require, and the promise is unfalsifiable.
  const [pinMajor, pinMinor, pinPatch] = versionParts(pinnedToolchain());
  const [msrvMajor, msrvMinor, msrvPatch] = versionParts(declaredMsrv());
  const pinIsAtLeastMsrv =
    pinMajor > msrvMajor ||
    (pinMajor === msrvMajor &&
      (pinMinor > msrvMinor || (pinMinor === msrvMinor && pinPatch >= msrvPatch)));
  assert.ok(
    pinIsAtLeastMsrv,
    `rust-toolchain.toml pins ${pinnedToolchain()}, older than the declared MSRV ${declaredMsrv()}`,
  );
});

test("published install prerequisites quote the toolchain contributors install", () => {
  // These sentences are install instructions and link to
  // rust-toolchain.toml, so they describe the *pin*, not the MSRV. The
  // two are allowed to diverge; asserting this prose against
  // Cargo.toml's rust-version would redden CI for the documented
  // pin-only upgrade, in which every file is correct.
  const [pinMajor, pinMinor] = versionParts(pinnedToolchain());
  const quoted = /\bRust (\d+)\.(\d+)/g;

  for (const [name, text] of [
    ["README.md", readme],
    ["docs/operations/creator-kit.md", creatorKit],
    ["site/index.html", siteHtml],
  ]) {
    const found = [...text.matchAll(quoted)];
    assert.ok(found.length > 0, `${name} must state the required Rust version`);
    for (const [whole, statedMajor, statedMinor] of found) {
      assert.equal(
        `${statedMajor}.${statedMinor}`,
        `${pinMajor}.${pinMinor}`,
        `${name} advertises "${whole}" but rust-toolchain.toml pins ${pinMajor}.${pinMinor}`,
      );
    }
  }
});

test("CI documents every public crate and rejects a dead intra-doc link", () => {
  // The canonical event contract's doc comments ARE the spec
  // (crates/event-schema/src/lib.rs), so an undocumented public item or
  // an unresolved link is a hole in the spec, not a style nit.
  //
  // Two halves, and each is useless without the other. `missing_docs`
  // is a rustc lint that is allow-by-default: RUSTDOCFLAGS cannot
  // promote what was never emitted, so deleting the crate attribute
  // would leave the CI job green while the contract rots. Pin both.
  for (const [name, source] of [
    ["crates/event-schema/src/lib.rs", eventSchemaLib],
    ["crates/adapters/core/src/lib.rs", adapterCoreLib],
  ]) {
    assert.match(
      source,
      /^#!\[deny\(missing_docs\)\]$/m,
      `${name} must deny missing_docs — the CI rustdoc job cannot catch it`,
    );
  }

  assert.match(ciWorkflow, /^ {2}rust-docs:$/m, "ci.yml must define a rust-docs job");
  assert.match(ciWorkflow, /^ {4}name: Rust docs$/m, "the rustdoc job must be named Rust docs");

  const docStep = ciStep("Document the portable workspace");
  assert.match(
    docStep,
    /^ +RUSTDOCFLAGS: -D warnings$/m,
    "the rustdoc step must carry RUSTDOCFLAGS: -D warnings in its own env",
  );
  assert.match(
    docStep,
    /^ +run: cargo doc --workspace --exclude vigla-host --no-deps --document-private-items$/m,
    "the rustdoc step must document the whole portable workspace",
  );
});
