// Plan mind-map visual quality gates.
//
// These tests hydrate the real plan-preview surface through the same
// mission-event mock path as plan-mode.spec.ts. They assert the
// user-visible map is not clipped at desktop, tablet, or mobile widths
// and that every rendered node has a readable label or tooltip-backed
// detail.

import { test, expect, emitMission } from "./fixtures";

const MID = "msn-e2e-plan-map-0001";

interface VisualTask {
  index: number;
  title: string;
  description: string;
  role: "implementer" | "tester" | "reviewer";
  depends_on: number[];
  criteria: {
    summary: string;
    require_tests_pass: boolean;
    forbid_new_security_flags: boolean;
  };
  scope_paths: string[];
}

function envelope(seq: number, type: string, payload: unknown): Record<string, unknown> {
  return {
    mission_id: MID,
    seq,
    ts: new Date(2026, 4, 25, 9, seq, 0).toISOString(),
    type,
    payload,
  };
}

function missionSpec() {
  return {
    title: "Refactor mission planning surface",
    objective:
      "Make the proposed plan map readable, bounded, and comparable to a polished logic chart.",
    target_ref: "main",
    tests: null,
    supervisor_model: "claude",
    worker_model: null,
    worker_count: 3,
    confirm_plan: true,
    scope_paths: ["app/src/missions", "tests/e2e"],
  };
}

function visualTasks(): VisualTask[] {
  return [
    {
      index: 0,
      title: "Correct Dagre layout coordinate conversion",
      description: "Convert graph centers into React Flow top-left positions.",
      role: "implementer",
      depends_on: [],
      criteria: {
        summary: "nodes remain inside the map bounds",
        require_tests_pass: true,
        forbid_new_security_flags: true,
      },
      scope_paths: ["app/src/missions/plan-mind-map.ts"],
    },
    {
      index: 1,
      title: "Render custom map nodes with stable dimensions",
      description: "Replace default nodes with branch-colored custom nodes.",
      role: "implementer",
      depends_on: [0],
      criteria: {
        summary: "all nodes have non-empty labels",
        require_tests_pass: true,
        forbid_new_security_flags: true,
      },
      scope_paths: ["app/src/missions/PlanMindMap.tsx", "app/src/index.css"],
    },
    {
      index: 2,
      title: "Verify mobile and tablet plan review ergonomics",
      description: "Confirm fit-to-width behavior and constrained pan/zoom.",
      role: "tester",
      depends_on: [1],
      criteria: {
        summary: "desktop, tablet, and mobile screenshots have no clipping",
        require_tests_pass: true,
        forbid_new_security_flags: true,
      },
      scope_paths: ["tests/e2e/plan-mind-map-visual.spec.ts"],
    },
    {
      index: 3,
      title: "Review envelope risk markers and dependency edge contrast",
      description: "Keep cross-dependencies subtle and risk markers visible.",
      role: "reviewer",
      depends_on: [1],
      criteria: {
        summary: "risk and dependency affordances remain secondary",
        require_tests_pass: true,
        forbid_new_security_flags: true,
      },
      scope_paths: ["app/src/index.css"],
    },
    {
      index: 4,
      title:
        "Document the screenshot-backed quality gate for long labels and branch overflow",
      description: "Ensure long labels truncate visually while exposing details in title text.",
      role: "implementer",
      depends_on: [2, 3],
      criteria: {
        summary: "long title has tooltip-backed detail",
        require_tests_pass: true,
        forbid_new_security_flags: true,
      },
      scope_paths: ["tests/e2e"],
    },
  ];
}

async function hydratePlan(page: import("@playwright/test").Page): Promise<void> {
  const tasks = visualTasks();
  await emitMission(page, envelope(1, "mission.created", { spec: missionSpec() }));
  await emitMission(page, envelope(2, "mission.execution_started", null));
  await emitMission(page, envelope(3, "supervisor.decomposition", { tasks }));
  await emitMission(
    page,
    envelope(4, "plan.proposed", {
      tasks,
      generation: 1,
      overview: "A five-task visual quality pass for the mission planning surface.",
      tech_stack: [
        {
          layer: "layout",
          choice: "Dagre + React Flow",
          rationale: "Existing dependency; deterministic graph placement.",
          is_new: false,
        },
        {
          layer: "visual-gates",
          choice: "Playwright screenshot checks",
          rationale: "Catches clipping and blank-label regressions.",
          is_new: true,
        },
      ],
      envelope_fit: {
        scope: { fit: "within", note: "limited to planning UI" },
        reversibility: { fit: "within", note: "CSS and React component only" },
        risk: { fit: "near_limit", note: "changes a user-facing planning surface" },
        quality: { fit: "within", note: "unit and visual coverage" },
      },
    }),
  );
}

async function assertMapLayout(page: import("@playwright/test").Page) {
  const map = page.getByTestId("plan-mind-map");
  await expect(map).toBeVisible();
  await expect(page.getByRole("button", { name: /fit mind map/i })).toBeVisible();
  await page.getByRole("button", { name: /fit mind map/i }).click();
  await page.waitForTimeout(250);

  const metrics = await map.evaluate((element) => {
    const mapRect = element.getBoundingClientRect();
    const nodes = Array.from(element.querySelectorAll<HTMLElement>(".plan-map-node"));
    const nodeRects = nodes.map((node) => {
      const rect = node.getBoundingClientRect();
      return {
        kind: node.dataset.kind ?? "",
        branch: node.dataset.branch ?? "",
        text: node.textContent?.trim() ?? "",
        title: node.getAttribute("title") ?? "",
        left: rect.left,
        right: rect.right,
        top: rect.top,
        bottom: rect.bottom,
      };
    });
    const clipped = nodeRects.filter(
      (node) =>
        node.left < mapRect.left - 3 ||
        node.right > mapRect.right + 3 ||
        node.top < mapRect.top - 3 ||
        node.bottom > mapRect.bottom + 3,
    );
    return {
      nodeCount: nodes.length,
      taskCount: nodeRects.filter((node) => node.kind === "task").length,
      techLeafCount: nodeRects.filter((node) => node.kind === "tech-leaf").length,
      branches: Array.from(new Set(nodeRects.map((node) => node.branch))).sort(),
      blankNodes: nodeRects.filter((node) => node.text.length === 0),
      taskTitlesWithoutTooltip: nodeRects.filter(
        (node) => node.kind === "task" && node.title.length === 0,
      ),
      clipped,
    };
  });

  expect(metrics.nodeCount).toBeGreaterThanOrEqual(9);
  expect(metrics.taskCount).toBe(5);
  expect(metrics.techLeafCount).toBe(2);
  expect(metrics.branches).toEqual(
    expect.arrayContaining(["execution", "review", "tech", "test"]),
  );
  expect(metrics.blankNodes).toHaveLength(0);
  expect(metrics.taskTitlesWithoutTooltip).toHaveLength(0);
  expect(metrics.clipped).toHaveLength(0);
}

for (const viewport of [
  { name: "desktop", width: 1365, height: 900 },
  { name: "tablet", width: 768, height: 900 },
  { name: "mobile", width: 390, height: 844 },
]) {
  test(`renders an unclipped custom plan mind map on ${viewport.name}`, async ({
    e2ePage,
  }, testInfo) => {
    await e2ePage.setViewportSize({
      width: viewport.width,
      height: viewport.height,
    });
    await hydratePlan(e2ePage);
    await assertMapLayout(e2ePage);

    await testInfo.attach(`plan-mind-map-${viewport.name}`, {
      body: await e2ePage.getByTestId("plan-mind-map").screenshot(),
      contentType: "image/png",
    });
  });
}
