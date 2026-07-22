/* Regenerate the README screenshots (docs/media/) by staging
 * scenes in the real UI through the e2e mock harness.
 *
 * Usage, from the repo root:
 *   1. cd app && VITE_VIGLA_E2E=1 pnpm exec vite --strictPort \
 *        --host 127.0.0.1 --port 5180 &
 *   2. node scripts/capture-readme-media.cjs docs/media
 *
 * Requires `pnpm install` and `pnpm -C app exec playwright
 * install chromium` to have run once, plus `cwebp` for optimized
 * landing-page images. */
const path = require("node:path");
const fs = require("node:fs");
const { spawnSync } = require("node:child_process");
const { createRequire } = require("node:module");
const appRequire = createRequire(
  path.join(__dirname, "..", "app", "package.json"),
);
const { chromium } = appRequire("@playwright/test");

const OUT = process.argv[2] || ".";
const BASE = "http://127.0.0.1:5180";
// Match the mocked list_recent_missions row so the history →
// mission-inbox drill-down hydrates from the active mission.
const MID = "msn-e2e-0001";

function renderWebp(input, output, quality) {
  const result = spawnSync(
    "cwebp",
    ["-quiet", "-q", String(quality), "-resize", "1280", "800", input, "-o", output],
    { stdio: "inherit" },
  );
  if (result.error) throw result.error;
  if (result.status !== 0) throw new Error(`cwebp exited with status ${result.status}`);
}

let missionSeq = 0;
// Recent timestamps so elapsed timers read minutes, not days.
const NOW = Date.now();
const T0 = NOW - 14 * 60_000;

function env(type, payload) {
  missionSeq += 1;
  return {
    mission_id: MID,
    seq: missionSeq,
    ts: new Date(T0 + missionSeq * 42_000).toISOString(),
    type,
    payload,
  };
}

const workerSeqs = new Map();
function wev(worker_id, type, payload) {
  const seq = (workerSeqs.get(worker_id) ?? 0) + 1;
  workerSeqs.set(worker_id, seq);
  return {
    schema_version: "2.0",
    worker_id,
    task_id: null,
    seq,
    ts: new Date(T0 + 240_000 + seq * 21_000).toISOString(),
    type,
    payload,
  };
}

async function emitMission(page, e) {
  await page.evaluate((p) => window.__viglaE2e.emitMissionEvent(p), e);
}
async function emitWorker(page, e) {
  await page.evaluate((p) => window.__viglaE2e.emitWorkerEvent(p), e);
}
/** Dispatch a whole batch synchronously in one page task so React renders the
 * fleet deterministically and the roster-keyed fit pass frames it once. */
async function emitBatch(page, batch) {
  await page.evaluate((items) => {
    for (const item of items) {
      if (item.kind === "mission") window.__viglaE2e.emitMissionEvent(item.event);
      else window.__viglaE2e.emitWorkerEvent(item.event);
    }
  }, batch);
}
const m = (event) => ({ kind: "mission", event });
const w = (event) => ({ kind: "worker", event });

const TASKS = [
  {
    index: 0,
    title: "Implement token-bucket rate limiter middleware",
    description: "Add a per-key token bucket with configurable burst and refill.",
    role: "implementer",
    depends_on: [],
    criteria: { summary: "unit tests green", require_tests_pass: true, forbid_new_security_flags: true },
    scope_paths: ["src/middleware/rate_limit.rs"],
  },
  {
    index: 1,
    title: "Wire limiter into the public API pipeline",
    description: "Mount the middleware ahead of auth; add 429 responses.",
    role: "implementer",
    depends_on: [0],
    criteria: { summary: "requests over budget return 429", require_tests_pass: true, forbid_new_security_flags: true },
    scope_paths: ["src/api/router.rs", "src/api/errors.rs"],
  },
  {
    index: 2,
    title: "Integration tests: burst + sustained load",
    description: "Cover burst allowance, steady-state refill, and per-key isolation.",
    role: "tester",
    depends_on: [1],
    criteria: { summary: "load scenarios covered", require_tests_pass: true, forbid_new_security_flags: true },
    scope_paths: ["tests/rate_limit_integration.rs"],
  },
  {
    index: 3,
    title: "Document limits and 429 error contract",
    description: "Update API reference with limits, headers, and retry guidance.",
    role: "implementer",
    depends_on: [1],
    criteria: { summary: "docs match behavior", require_tests_pass: false, forbid_new_security_flags: true },
    scope_paths: ["docs/api/rate-limits.md"],
  },
  {
    index: 4,
    title: "Security review of limiter bypass paths",
    description: "Audit for header spoofing, key collisions, and unbounded memory.",
    role: "reviewer",
    depends_on: [2, 3],
    criteria: { summary: "no new security flags", require_tests_pass: true, forbid_new_security_flags: true },
    scope_paths: ["src/middleware/rate_limit.rs"],
  },
];

function missionSpec(extra = {}) {
  return {
    title: "Add rate limiting to the public API",
    objective:
      "Protect the public API with a per-key token-bucket rate limiter, full test coverage, and updated docs.",
    target_ref: "main",
    tests: null,
    supervisor_model: "claude",
    worker_model: null,
    worker_count: 5,
    confirm_plan: false,
    scope_paths: ["src", "tests", "docs/api"],
    ...extra,
  };
}

const HIDE_OVERLAY_CSS =
  ".mission-overlay { display: none !important; } .mission-overlay__backdrop { display: none !important; }";

async function hydrateRunningFleet(page) {
  const batch = [
    m(env("mission.created", { spec: missionSpec() })),
    m(env("supervisor.decomposition", { tasks: TASKS })),
    m(env("mission.execution_started", {})),
  ];
  const roster = [
    ["wkr-claude-01", 0],
    ["wkr-codex-01", 1],
    ["wkr-antigravity-01", 2],
    ["wkr-copilot-01", 3],
    ["wkr-claude-02", 4],
  ];
  for (const [id, idx] of roster) {
    batch.push(
      m(env("worker.spawned", { worker_id: id, task_index: idx, task_title: TASKS[idx].title })),
    );
  }

  // wkr-claude-01 — done, tests green
  batch.push(
    w(wev("wkr-claude-01", "state_change", { state: "executing", from: "planning", note: "writing token bucket" })),
    w(wev("wkr-claude-01", "file_activity", { path: "src/middleware/rate_limit.rs", op: "create", lines_added: 214, lines_removed: 0 })),
    w(wev("wkr-claude-01", "log", { level: "info", stream: "stdout", line: "Implemented TokenBucket::try_acquire with monotonic clock refill" })),
    w(wev("wkr-claude-01", "test_result", { suite: "rate_limit", passed: 18, failed: 0, skipped: 0, duration_ms: 4210 })),
    w(wev("wkr-claude-01", "cost", { input_tokens: 41230, output_tokens: 9187, usd: 0.87, model: "claude-sonnet-5" })),
    w(wev("wkr-claude-01", "progress", { percent: 100, note: "middleware complete, 18/18 tests" })),
    w(wev("wkr-claude-01", "state_change", { state: "done", from: "executing", note: "submitted for audit" })),
    // wkr-codex-01 — executing 72%
    w(wev("wkr-codex-01", "state_change", { state: "executing", from: "planning", note: "mounting middleware" })),
    w(wev("wkr-codex-01", "file_activity", { path: "src/api/router.rs", op: "modify", lines_added: 38, lines_removed: 6 })),
    w(wev("wkr-codex-01", "file_activity", { path: "src/api/errors.rs", op: "modify", lines_added: 22, lines_removed: 1 })),
    w(wev("wkr-codex-01", "log", { level: "info", stream: "stdout", line: "429 handler wired; Retry-After header emitted from bucket state" })),
    w(wev("wkr-codex-01", "cost", { input_tokens: 28840, output_tokens: 6402, usd: 0.54, model: "gpt-5.2-codex" })),
    w(wev("wkr-codex-01", "progress", { percent: 72, eta_ms: 240000, note: "wiring 429 responses" })),
    // wkr-antigravity-01 — executing 41%
    w(wev("wkr-antigravity-01", "state_change", { state: "executing", from: "idle", note: "writing load scenarios" })),
    w(wev("wkr-antigravity-01", "file_activity", { path: "tests/rate_limit_integration.rs", op: "create", lines_added: 96, lines_removed: 0 })),
    w(wev("wkr-antigravity-01", "log", { level: "info", stream: "stdout", line: "burst scenario: 50 req burst allowed, 51st receives 429" })),
    w(wev("wkr-antigravity-01", "cost", { input_tokens: 19110, output_tokens: 4023, usd: 0.21, model: "default" })),
    w(wev("wkr-antigravity-01", "progress", { percent: 41, eta_ms: 380000, note: "2 of 5 scenarios written" })),
    // wkr-copilot-01 — reviewing 88%
    w(wev("wkr-copilot-01", "state_change", { state: "executing", from: "planning", note: "drafting docs" })),
    w(wev("wkr-copilot-01", "file_activity", { path: "docs/api/rate-limits.md", op: "create", lines_added: 74, lines_removed: 0 })),
    w(wev("wkr-copilot-01", "cost", { input_tokens: 9930, output_tokens: 3811, usd: 0.11, model: "gpt-5.2" })),
    w(wev("wkr-copilot-01", "progress", { percent: 88, note: "docs drafted, self-review" })),
    w(wev("wkr-copilot-01", "state_change", { state: "reviewing", from: "executing", note: "verifying examples" })),
    // wkr-claude-02 — queued behind deps
    w(wev("wkr-claude-02", "state_change", { state: "planning", from: "idle", note: "queued behind tests + docs" })),
    w(wev("wkr-claude-02", "dependency", { waiting_on: ["wkr-antigravity-01", "wkr-copilot-01"], reason: "security review runs last" })),
    w(wev("wkr-claude-02", "progress", { percent: 5, note: "reading limiter design" })),
  );
  await emitBatch(page, batch);
}

async function freshPage(browser, { width = 1600, height = 1000 } = {}) {
  const ctx = await browser.newContext({
    viewport: { width, height },
    deviceScaleFactor: 2,
    colorScheme: "dark",
  });
  const page = await ctx.newPage();
  await page.goto(BASE);
  await page.waitForFunction(() => typeof window.__viglaE2e === "object");
  await page.waitForTimeout(400);
  return page;
}

(async () => {
  const browser = await chromium.launch();

  // Scene 1: Operations Room, live fleet (hero) — overlay hidden
  {
    const page = await freshPage(browser);
    await page.addStyleTag({ content: HIDE_OVERLAY_CSS });
    await hydrateRunningFleet(page);
    await page.keyboard.press("Meta+2");
    await page.waitForTimeout(1500);
    const opsScreenshot = await page.screenshot({ path: path.join(OUT, "ops-room.png") });
    const socialDataUrl = await page.evaluate(async (source) => {
      const image = new Image();
      image.src = source;
      await image.decode();
      await document.fonts.ready;
      const canvas = document.createElement("canvas");
      canvas.width = 1280;
      canvas.height = 640;
      const context = canvas.getContext("2d");
      if (!context) throw new Error("2D canvas is unavailable");

      const background = context.createLinearGradient(0, 0, 1280, 640);
      background.addColorStop(0, "#06090f");
      background.addColorStop(0.58, "#09131c");
      background.addColorStop(1, "#071017");
      context.fillStyle = background;
      context.fillRect(0, 0, 1280, 640);

      context.strokeStyle = "rgba(56, 197, 180, 0.07)";
      context.lineWidth = 1;
      for (let x = 0; x <= 1280; x += 40) {
        context.beginPath();
        context.moveTo(x, 0);
        context.lineTo(x, 640);
        context.stroke();
      }
      for (let y = 0; y <= 640; y += 40) {
        context.beginPath();
        context.moveTo(0, y);
        context.lineTo(1280, y);
        context.stroke();
      }

      context.save();
      context.beginPath();
      context.roundRect(616, 84, 720, 456, 18);
      context.clip();
      context.drawImage(
        image,
        0,
        0,
        image.naturalWidth,
        image.naturalHeight,
        616,
        84,
        720,
        450,
      );
      const imageShade = context.createLinearGradient(616, 0, 800, 0);
      imageShade.addColorStop(0, "rgba(6, 9, 15, 0.72)");
      imageShade.addColorStop(1, "rgba(6, 9, 15, 0)");
      context.fillStyle = imageShade;
      context.fillRect(616, 84, 200, 456);
      context.restore();

      context.strokeStyle = "rgba(56, 197, 180, 0.42)";
      context.lineWidth = 2;
      context.beginPath();
      context.roundRect(616, 84, 720, 456, 18);
      context.stroke();

      context.fillStyle = "#38c5b4";
      context.beginPath();
      context.arc(76, 71, 9, 0, Math.PI * 2);
      context.fill();
      context.strokeStyle = "rgba(56, 197, 180, 0.55)";
      context.beginPath();
      context.arc(76, 71, 17, 0, Math.PI * 2);
      context.stroke();
      context.fillStyle = "#f4f7fa";
      context.font = "750 28px Inter, -apple-system, BlinkMacSystemFont, sans-serif";
      context.fillText("Vigla", 108, 81);

      context.fillStyle = "rgba(56, 197, 180, 0.13)";
      context.beginPath();
      context.roundRect(68, 137, 244, 38, 19);
      context.fill();
      context.fillStyle = "#67d7ca";
      context.font = "700 15px Inter, -apple-system, BlinkMacSystemFont, sans-serif";
      context.letterSpacing = "1.1px";
      context.fillText("OPEN SOURCE · LOCAL-FIRST", 88, 162);

      context.fillStyle = "#f4f7fa";
      context.font = "760 58px Inter, -apple-system, BlinkMacSystemFont, sans-serif";
      context.letterSpacing = "-2px";
      context.fillText("Supervise the merge.", 68, 250);
      context.fillText("Not every terminal.", 68, 318);

      context.fillStyle = "#aeb8c7";
      context.font = "450 22px Inter, -apple-system, BlinkMacSystemFont, sans-serif";
      context.letterSpacing = "0px";
      context.fillText("Cross-vendor coding agents.", 68, 377);
      context.fillText("One audited, reversible mission.", 68, 410);

      context.fillStyle = "#778497";
      context.font = "600 17px 'JetBrains Mono', ui-monospace, monospace";
      context.fillText("github.com/Kilbex/Vigla", 68, 558);
      return canvas.toDataURL("image/png");
    }, `data:image/png;base64,${opsScreenshot.toString("base64")}`);
    fs.writeFileSync(
      path.join(OUT, "social-preview.png"),
      Buffer.from(socialDataUrl.split(",", 2)[1], "base64"),
    );
    await page.close();
  }

  // Scene 2: Plan review overlay (element crop)
  {
    const page = await freshPage(browser, { width: 1600, height: 1400 });
    missionSeq = 0;
    await emitMission(page, env("mission.created", { spec: missionSpec({ confirm_plan: true }) }));
    await emitMission(page, env("mission.execution_started", {}));
    await emitMission(page, env("supervisor.decomposition", { tasks: TASKS }));
    await emitMission(
      page,
      env("plan.proposed", {
        tasks: TASKS,
        generation: 1,
        overview:
          "Five-task plan: build the limiter, wire it into the pipeline, prove it under load, document the contract, then security-review the bypass paths.",
        tech_stack: [
          { layer: "middleware", choice: "tower + token bucket", rationale: "Existing dependency; zero new crates.", is_new: false },
          { layer: "testing", choice: "integration load scenarios", rationale: "Catches burst/steady-state regressions.", is_new: true },
        ],
        envelope_fit: {
          scope: { fit: "within", note: "limited to API + docs" },
          reversibility: { fit: "within", note: "one revertable merge" },
          risk: { fit: "near_limit", note: "touches the public request path" },
          quality: { fit: "within", note: "tests required on every task" },
        },
      }),
    );
    await page.waitForTimeout(1500);
    const fit = page.getByRole("button", { name: /fit mind map/i });
    if (await fit.isVisible().catch(() => false)) {
      await fit.click();
      await page.waitForTimeout(500);
    }
    const card = page.locator(".mission-overlay__card");
    if (await card.isVisible().catch(() => false)) {
      await card.screenshot({ path: path.join(OUT, "plan-review.png") });
    } else {
      await page.screenshot({ path: path.join(OUT, "plan-review.png") });
    }
    await page.close();
  }

  // Scene 3: Mission inbox with completion verdict + finished fleet
  {
    const page = await freshPage(browser);
    missionSeq = 0;
    const batch3 = [
      m(env("mission.created", { spec: missionSpec({ worker_count: 3 }) })),
      m(env("supervisor.decomposition", { tasks: TASKS.slice(0, 3) })),
      m(env("mission.execution_started", {})),
    ];
    for (const [id, idx] of [
      ["wkr-claude-01", 0],
      ["wkr-codex-01", 1],
      ["wkr-antigravity-01", 2],
    ]) {
      batch3.push(m(env("worker.spawned", { worker_id: id, task_index: idx, task_title: TASKS[idx].title })));
    }
    // Finished fleet on the canvas behind the verdict rail.
    const doneWorkers = [
      ["wkr-claude-01", "claude-sonnet-5", 0.87, 18],
      ["wkr-codex-01", "gpt-5.2-codex", 0.54, 12],
      ["wkr-antigravity-01", "default", 0.21, 16],
    ];
    for (const [id, model, usd, passed] of doneWorkers) {
      batch3.push(
        w(wev(id, "state_change", { state: "executing", from: "planning", note: "working" })),
        w(wev(id, "test_result", { suite: "workspace", passed, failed: 0, skipped: 0, duration_ms: 5100 })),
        w(wev(id, "cost", { input_tokens: 21000, output_tokens: 6100, usd, model })),
        w(wev(id, "progress", { percent: 100, note: "submitted for audit" })),
        w(wev(id, "state_change", { state: "done", from: "executing", note: "accepted by supervisor" })),
      );
    }
    await emitBatch(page, batch3);
    await emitMission(page, env("supervisor.integrated", { worker_id: "wkr-claude-01", integration_sha: "8f31c2adeadbeef", snapshot_tag: "vigla/pre-merge/msn-e2e-0001" }));
    await emitMission(page, env("supervisor.integrated", { worker_id: "wkr-codex-01", integration_sha: "190af42deadbeef", snapshot_tag: "vigla/pre-merge/msn-e2e-0001" }));
    await emitMission(page, env("supervisor.integrated", { worker_id: "wkr-antigravity-01", integration_sha: "62bd903deadbeef", snapshot_tag: "vigla/pre-merge/msn-e2e-0001" }));
    await emitMission(
      page,
      env("supervisor.audit_completed", {
        tier: "standard",
        overall: 0.93,
        payload_json: JSON.stringify({
          overall: 0.93,
          test_pass: { ran: true, passed: 46, failed: 0, skipped: 1, score: 0.97 },
          scope: null,
          regression: null,
          lint: null,
          security_flags: [],
        }),
      }),
    );
    await emitMission(page, env("mission.completed", { summary: "Rate limiter shipped: middleware, pipeline wiring, 46 tests, docs.", files_changed: 7 }));
    await emitMission(page, env("mission.merge_resolved", { resolution: { type: "merged" } }));
    await emitMission(
      page,
      env("mission.completion_verdict_rendered", {
        payload_json: JSON.stringify({
          all_subtasks_accepted: true,
          integrated_test_pass: { passed: 46, failed: 0 },
          residual_risk: "low",
          doc_coverage: 1.0,
          unresolved_issues: [],
          recommendation: {
            kind: "accept",
            audit: {
              overall: 0.93,
              test_pass: { passed: 46, failed: 0 },
              scope: null,
              regression: null,
              lint: null,
              security_flags: [],
            },
            summary: "All subtasks accepted; residual risk low.",
          },
        }),
      }),
    );
    await page.addStyleTag({ content: HIDE_OVERLAY_CSS });
    await page.keyboard.press("Meta+3");
    await page.waitForTimeout(500);
    const row = page.locator(".mission-history-row").first();
    if (await row.isVisible().catch(() => false)) {
      await row.click();
      await page.waitForTimeout(800);
    }
    await page.screenshot({ path: path.join(OUT, "mission-inbox.png") });
    await page.close();
  }

  await browser.close();
  renderWebp(
    path.join(OUT, "ops-room.png"),
    path.join(OUT, "vigla-demo-poster.webp"),
    82,
  );
  renderWebp(
    path.join(OUT, "mission-inbox.png"),
    path.join(OUT, "mission-inbox.webp"),
    82,
  );
  console.log("done");
})();
