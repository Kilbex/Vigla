import { describe, it, expect } from "vitest";
import {
  buildMindMap,
  MIND_MAP_NODE_DIMENSIONS,
  type MindMap,
  type PlanTask,
} from "../plan-mind-map";
import type { EnvelopeFit, TechChoice } from "../types";

const spec = { title: "Add OAuth callback", objective: "implement /auth/callback" };

function task(index: number, title: string, deps: number[] = []): PlanTask {
  return { index, title, description: "", depends_on: deps };
}

function assertNoBlankLabels(mm: MindMap): void {
  expect(
    mm.nodes.filter((node) => !String(node.data.label ?? "").trim()),
  ).toHaveLength(0);
}

function assertNodesInsideBounds(mm: MindMap): void {
  for (const node of mm.nodes) {
    expect(node.position.x).toBeGreaterThanOrEqual(0);
    expect(node.position.y).toBeGreaterThanOrEqual(0);
    expect(node.position.x + node.dimensions.width).toBeLessThanOrEqual(
      mm.bounds.width,
    );
    expect(node.position.y + node.dimensions.height).toBeLessThanOrEqual(
      mm.bounds.height,
    );
  }
}

function assertNoNodeOverlap(mm: MindMap): void {
  const overlaps: string[] = [];
  for (let i = 0; i < mm.nodes.length; i += 1) {
    for (let j = i + 1; j < mm.nodes.length; j += 1) {
      const a = mm.nodes[i];
      const b = mm.nodes[j];
      const intersects =
        a.position.x < b.position.x + b.dimensions.width &&
        a.position.x + a.dimensions.width > b.position.x &&
        a.position.y < b.position.y + b.dimensions.height &&
        a.position.y + a.dimensions.height > b.position.y;
      if (intersects) overlaps.push(`${a.id}/${b.id}`);
    }
  }
  expect(overlaps).toEqual([]);
}

describe("buildMindMap", () => {
  it("empty plan produces only the root node", () => {
    const mm = buildMindMap(spec, { tasks: [], generation: 0 });
    expect(mm.nodes.find((n) => n.id === "root")).toBeTruthy();
    // No tech stack, no waves, no tasks → only the root remains.
    expect(mm.nodes.filter((n) => n.type === "wave")).toHaveLength(0);
    expect(mm.nodes.filter((n) => n.type === "task")).toHaveLength(0);
    assertNoBlankLabels(mm);
    assertNodesInsideBounds(mm);
    assertNoNodeOverlap(mm);
  });

  it("single task plan produces one wave with one task", () => {
    const mm = buildMindMap(spec, {
      tasks: [task(0, "A")],
      generation: 0,
    });
    expect(mm.nodes.filter((n) => n.type === "wave")).toHaveLength(1);
    expect(mm.nodes.filter((n) => n.type === "task")).toHaveLength(1);
    expect(mm.edges.find((e) => e.source === "wave-0" && e.target === "task-0")).toBeTruthy();
    assertNoBlankLabels(mm);
    assertNodesInsideBounds(mm);
    assertNoNodeOverlap(mm);
  });

  it("linear 4-task chain produces 4 waves of 1 module each", () => {
    const tasks = [
      task(0, "A"),
      task(1, "B", [0]),
      task(2, "C", [1]),
      task(3, "D", [2]),
    ];
    const mm = buildMindMap(spec, { tasks, generation: 0 });
    const waves = mm.nodes.filter((n) => n.type === "wave");
    expect(waves).toHaveLength(4);
    expect(
      mm.edges.find(
        (e) => e.source === "task-0" && e.target === "task-1",
      ),
    ).toBeTruthy();
  });

  it("diamond (1→2, 1→3, 2→4, 3→4) produces 3 waves", () => {
    const tasks = [
      task(0, "root"),
      task(1, "left", [0]),
      task(2, "right", [0]),
      task(3, "join", [1, 2]),
    ];
    const mm = buildMindMap(spec, { tasks, generation: 0 });
    const waves = mm.nodes.filter((n) => n.type === "wave");
    expect(waves).toHaveLength(3);
    const edgeIds = mm.edges
      .filter((e) => e.id.startsWith("task-"))
      .map((e) => `${e.source}->${e.target}`)
      .sort();
    expect(edgeIds).toEqual(
      ["task-0->task-1", "task-0->task-2", "task-1->task-3", "task-2->task-3"].sort(),
    );
    for (const edge of mm.edges.filter(
      (candidate) => candidate.data?.kind === "dependency",
    )) {
      expect(edge.layout).toBe(false);
    }
  });

  it("tech_stack rows produce TechLeaf nodes with is_new flag", () => {
    const ts: TechChoice[] = [
      { layer: "framework", choice: "Tauri 2", rationale: "exists", is_new: false },
      { layer: "migrations", choice: "sqlx-cli", rationale: "new", is_new: true },
    ];
    const mm = buildMindMap(spec, { tasks: [], generation: 0, tech_stack: ts });
    const techLeaves = mm.nodes.filter((n) => n.type === "tech-leaf");
    expect(techLeaves).toHaveLength(2);
    expect(techLeaves.map((n) => n.data.label)).toEqual([
      "framework: Tauri 2",
      "migrations: sqlx-cli",
    ]);
    const newLeaves = techLeaves.filter((n) => n.data.is_new === true);
    expect(newLeaves).toHaveLength(1);
    assertNoBlankLabels(mm);
    assertNoNodeOverlap(mm);
  });

  it("envelope flag set when any bound is non-Within", () => {
    const ef: EnvelopeFit = {
      scope: { fit: "within", note: "" },
      reversibility: { fit: "exceeds", note: "migration" },
      risk: { fit: "within", note: "" },
      quality: { fit: "within", note: "" },
    };
    const mm = buildMindMap(spec, {
      tasks: [task(0, "A")],
      generation: 0,
      envelope_fit: ef,
    });
    const root = mm.nodes.find((n) => n.id === "root");
    expect(root?.data.envelope_flag).toBe(true);
    expect(root?.data.envelope_status).toBe("exceeds");
    expect(root?.data.branch).toBe("risk");
  });

  it("envelope flag cleared when all bounds are Within", () => {
    const ef: EnvelopeFit = {
      scope: { fit: "within", note: "" },
      reversibility: { fit: "within", note: "" },
      risk: { fit: "within", note: "" },
      quality: { fit: "within", note: "" },
    };
    const mm = buildMindMap(spec, {
      tasks: [],
      generation: 0,
      envelope_fit: ef,
    });
    const root = mm.nodes.find((n) => n.id === "root");
    expect(root?.data.envelope_flag).toBe(false);
  });

  it("uses CSS-matched dimensions and top-left positions", () => {
    const mm = buildMindMap(spec, {
      tasks: [
        task(0, "Prepare state"),
        task(1, "Implement callback", [0]),
        task(2, "Review changes", [1]),
      ],
      generation: 0,
    });
    for (const node of mm.nodes) {
      expect(node.dimensions).toEqual(MIND_MAP_NODE_DIMENSIONS[node.type]);
    }
    assertNodesInsideBounds(mm);
    assertNoNodeOverlap(mm);
  });

  it("renders long, parallel, and role-colored task plans with tooltip detail", () => {
    const tasks: PlanTask[] = Array.from({ length: 12 }, (_, index) => ({
      index,
      title:
        index === 4
          ? "Coordinate an unusually long callback migration title that must truncate but remain inspectable"
          : `Task ${index + 1}`,
      description: `Task ${index + 1} description`,
      role: index % 5 === 0 ? "tester" : index % 4 === 0 ? "reviewer" : "implementer",
      depends_on: index < 4 ? [] : [index - 4],
      criteria_summary: "tests pass",
      scope_paths: [`src/module-${index}.ts`, `tests/module-${index}.spec.ts`],
    }));
    const mm = buildMindMap(spec, { tasks, generation: 0 });
    expect(mm.nodes.filter((n) => n.type === "task")).toHaveLength(12);
    expect(mm.nodes.some((n) => n.data.branch === "test")).toBe(true);
    expect(mm.nodes.some((n) => n.data.branch === "review")).toBe(true);
    expect(
      mm.nodes
        .filter((n) => n.type === "task")
        .every((n) => String(n.data.tooltip).includes("Criteria: tests pass")),
    ).toBe(true);
    assertNoBlankLabels(mm);
    assertNodesInsideBounds(mm);
    assertNoNodeOverlap(mm);
  });

  it("keeps tech-stack-only plans readable", () => {
    const mm = buildMindMap(spec, {
      tasks: [],
      generation: 0,
      tech_stack: [
        {
          layer: "runtime",
          choice: "Node 22",
          rationale: "existing toolchain",
          is_new: false,
        },
      ],
    });
    expect(mm.nodes.map((n) => n.type).sort()).toEqual(
      ["root", "tech-leaf", "tech-root"].sort(),
    );
    assertNoBlankLabels(mm);
    assertNodesInsideBounds(mm);
    assertNoNodeOverlap(mm);
  });

  it("falls back to a final wave when dependencies form a cycle", () => {
    const mm = buildMindMap(spec, {
      tasks: [task(0, "A", [1]), task(1, "B", [0])],
      generation: 0,
    });
    expect(mm.nodes.filter((n) => n.type === "task")).toHaveLength(2);
    expect(mm.nodes.filter((n) => n.type === "wave")).toHaveLength(1);
    assertNoBlankLabels(mm);
    assertNoNodeOverlap(mm);
  });
});
