// QC-3 — Mind-map projection.
//
// Pure builder, no React. Given a mission spec and the PlanProposed
// payload, returns the React-Flow-shaped { nodes, edges } that
// PlanMindMap.tsx renders. Wave layering is Kahn's algorithm in TS —
// identical in shape to the orchestrator's
// task_graph::scheduler::ready() topology run to completion.
//
// Why this lives FE-side: the FE already has the task list and the
// envelope_fit on every PlanProposed event. Recomputing the waves
// in TS avoids a wire type (`MissionPlan` / `Wave` / `MindMap`)
// that would only serve the mind-map view.

import dagre from "@dagrejs/dagre";
import type { EnvelopeFit, TechChoice } from "./types";

export interface PlanTask {
  index: number;
  title: string;
  description?: string | null;
  depends_on?: number[];
  role?: "implementer" | "tester" | "reviewer" | string;
  criteria?: {
    min_audit_overall?: number | null;
    require_tests_pass?: boolean | null;
    forbid_new_security_flags?: boolean | null;
    summary?: string | null;
  };
  criteria_summary?: string | null;
  scope_paths?: string[];
}

export type MindMapNodeType =
  | "root"
  | "tech-root"
  | "tech-leaf"
  | "wave"
  | "task";

export type MindMapBranch =
  | "root"
  | "execution"
  | "test"
  | "review"
  | "tech"
  | "risk";

export type EnvelopeStatus = "within" | "near_limit" | "exceeds" | "unknown";

export interface MindMapNodeDimensions {
  width: number;
  height: number;
}

export interface MindMapNodeData extends Record<string, unknown> {
  label: string;
  branch: MindMapBranch;
  tooltip: string;
  subtitle?: string;
  objective?: string;
  envelope_status?: EnvelopeStatus;
  envelope_label?: string;
  envelope_summary?: string;
  task_count?: number;
  role?: string;
  scope_summary?: string;
  dependency_count?: number;
  description?: string;
  criteria_summary?: string;
  layer?: string;
  choice?: string;
  is_new?: boolean;
}

export interface MindMapNode {
  id: string;
  type: MindMapNodeType;
  position: { x: number; y: number };
  dimensions: MindMapNodeDimensions;
  data: MindMapNodeData;
}

export interface MindMapEdge {
  id: string;
  source: string;
  target: string;
  type?: string;
  className?: string;
  animated?: boolean;
  layout?: boolean;
  data?: {
    kind: "hierarchy" | "dependency";
    branch: MindMapBranch;
  };
}

export interface MindMap {
  nodes: MindMapNode[];
  edges: MindMapEdge[];
  bounds: { x: number; y: number; width: number; height: number };
}

export interface PlanPayload {
  tasks: PlanTask[];
  generation: number;
  overview?: string | null;
  tech_stack?: TechChoice[] | null;
  envelope_fit?: EnvelopeFit | null;
}

export interface PlanSpec {
  title: string;
  objective: string;
}

export const MIND_MAP_NODE_DIMENSIONS: Record<
  MindMapNodeType,
  MindMapNodeDimensions
> = {
  root: { width: 260, height: 96 },
  "tech-root": { width: 180, height: 54 },
  "tech-leaf": { width: 220, height: 74 },
  wave: { width: 160, height: 52 },
  task: { width: 240, height: 92 },
};

const MAP_PADDING = 40;

/**
 * Build the mind-map projection of a proposed plan.
 *
 * Layout: dagre LR. Waves derive from Kahn's algorithm over the
 * task `depends_on` edges; the same task ordering the orchestrator
 * uses to dispatch workers in parallel.
 */
export function buildMindMap(spec: PlanSpec, plan: PlanPayload): MindMap {
  const nodes: MindMapNode[] = [];
  const edges: MindMapEdge[] = [];
  const taskByIndex = new Map(plan.tasks.map((task) => [task.index, task]));

  const envelope = summarizeEnvelope(plan.envelope_fit);

  // Root.
  nodes.push({
    id: "root",
    type: "root",
    position: { x: 0, y: 0 },
    dimensions: MIND_MAP_NODE_DIMENSIONS.root,
    data: {
      label: spec.title || "Mission",
      subtitle: "Mission",
      objective: spec.objective,
      branch: envelope.status === "exceeds" ? "risk" : "root",
      tooltip: [spec.title, spec.objective, envelope.summary]
        .filter(Boolean)
        .join("\n\n"),
      envelope_flag: envelope.status !== "within" && envelope.status !== "unknown",
      envelope_status: envelope.status,
      envelope_label: envelope.label,
      envelope_summary: envelope.summary,
    },
  });

  // Tech-stack subtree.
  if (plan.tech_stack && plan.tech_stack.length > 0) {
    nodes.push({
      id: "tech-root",
      type: "tech-root",
      position: { x: 0, y: 0 },
      dimensions: MIND_MAP_NODE_DIMENSIONS["tech-root"],
      data: {
        label: "Tech stack",
        subtitle: `${plan.tech_stack.length} item${
          plan.tech_stack.length === 1 ? "" : "s"
        }`,
        branch: "tech",
        tooltip: "Technology choices proposed for this mission.",
        task_count: plan.tech_stack.length,
      },
    });
    edges.push(hierarchyEdge("root->tech-root", "root", "tech-root", "tech"));
    plan.tech_stack.forEach((t, idx) => {
      const id = `tech-${idx}`;
      const layer = t.layer?.trim() || "layer";
      const choice = t.choice?.trim() || "choice";
      nodes.push({
        id,
        type: "tech-leaf",
        position: { x: 0, y: 0 },
        dimensions: MIND_MAP_NODE_DIMENSIONS["tech-leaf"],
        data: {
          label: `${layer}: ${choice}`,
          subtitle: t.is_new ? "New stack element" : "Existing stack",
          branch: "tech",
          tooltip: [layer, choice, t.rationale].filter(Boolean).join("\n"),
          layer,
          choice,
          is_new: t.is_new === true,
        },
      });
      edges.push(hierarchyEdge(`tech-root->${id}`, "tech-root", id, "tech"));
    });
  }

  // Waves (Kahn's algorithm over depends_on).
  const indegree: Record<number, number> = {};
  const dependents: Record<number, number[]> = {};
  for (const t of plan.tasks) {
    indegree[t.index] = 0;
    dependents[t.index] = [];
  }
  for (const t of plan.tasks) {
    for (const dep of t.depends_on ?? []) {
      if (!taskByIndex.has(dep)) continue;
      indegree[t.index] = (indegree[t.index] ?? 0) + 1;
      dependents[dep] = [...(dependents[dep] ?? []), t.index];
    }
  }
  const waves: number[][] = [];
  let frontier = plan.tasks
    .filter((t) => (indegree[t.index] ?? 0) === 0)
    .map((t) => t.index);
  while (frontier.length > 0) {
    waves.push([...frontier].sort((a, b) => a - b));
    const next: number[] = [];
    for (const idx of frontier) {
      for (const child of dependents[idx] ?? []) {
        indegree[child] -= 1;
        if (indegree[child] === 0) next.push(child);
      }
    }
    frontier = next;
  }
  const scheduled = new Set(waves.flat());
  const unscheduled = plan.tasks
    .filter((t) => !scheduled.has(t.index))
    .map((t) => t.index)
    .sort((a, b) => a - b);
  if (unscheduled.length > 0) {
    waves.push(unscheduled);
  }

  // Wave + task nodes.
  waves.forEach((batch, waveIdx) => {
    const waveId = `wave-${waveIdx}`;
    nodes.push({
      id: waveId,
      type: "wave",
      position: { x: 0, y: 0 },
      dimensions: MIND_MAP_NODE_DIMENSIONS.wave,
      data: {
        label: `Wave ${waveIdx + 1}`,
        subtitle: `${batch.length} task${batch.length === 1 ? "" : "s"}`,
        branch: "execution",
        tooltip: `${batch.length} task${batch.length === 1 ? "" : "s"} can run in this wave.`,
        task_count: batch.length,
      },
    });
    edges.push(hierarchyEdge(`root->${waveId}`, "root", waveId, "execution"));
    for (const taskIdx of batch) {
      const task = taskByIndex.get(taskIdx);
      if (!task) continue;
      const taskId = `task-${taskIdx}`;
      const branch = branchForRole(task.role);
      const scopeSummary = summarizeScope(task.scope_paths);
      const criteriaSummary =
        task.criteria_summary ?? summarizeCriteria(task.criteria);
      const dependencyCount = (task.depends_on ?? []).filter((dep) =>
        taskByIndex.has(dep),
      ).length;
      const title = task.title?.trim() || `Task ${task.index + 1}`;
      nodes.push({
        id: taskId,
        type: "task",
        position: { x: 0, y: 0 },
        dimensions: MIND_MAP_NODE_DIMENSIONS.task,
        data: {
          label: title,
          subtitle: `Task ${task.index + 1}`,
          branch,
          tooltip: buildTaskTooltip(
            task,
            scopeSummary,
            criteriaSummary,
            dependencyCount,
          ),
          index: task.index,
          role: task.role,
          scope_summary: scopeSummary,
          dependency_count: dependencyCount,
          description: task.description ?? undefined,
          criteria_summary: criteriaSummary,
        },
      });
      edges.push(hierarchyEdge(`${waveId}->${taskId}`, waveId, taskId, branch));
    }
  });

  // Dependency edges between tasks (drawn on top of the wave
  // grouping so the FE can render them as light, secondary lines).
  for (const t of plan.tasks) {
    for (const dep of t.depends_on ?? []) {
      if (!taskByIndex.has(dep)) continue;
      edges.push({
        id: `task-${dep}->task-${t.index}`,
        source: `task-${dep}`,
        target: `task-${t.index}`,
        type: "smoothstep",
        className: "plan-mind-map__edge plan-mind-map__edge--dependency",
        layout: false,
        data: { kind: "dependency", branch: branchForRole(t.role) },
      });
    }
  }

  const bounds = layoutInPlace(nodes, edges);
  return { nodes, edges, bounds };
}

function hierarchyEdge(
  id: string,
  source: string,
  target: string,
  branch: MindMapBranch,
): MindMapEdge {
  return {
    id,
    source,
    target,
    type: "smoothstep",
    className: `plan-mind-map__edge plan-mind-map__edge--hierarchy plan-mind-map__edge--${branch}`,
    data: { kind: "hierarchy", branch },
  };
}

function layoutInPlace(
  nodes: MindMapNode[],
  edges: MindMapEdge[],
): MindMap["bounds"] {
  const g = new dagre.graphlib.Graph();
  g.setGraph({
    rankdir: "LR",
    nodesep: 34,
    ranksep: 74,
    marginx: MAP_PADDING,
    marginy: MAP_PADDING,
  });
  g.setDefaultEdgeLabel(() => ({}));
  for (const n of nodes) {
    // Dagre mutates node labels with x/y/order/rank during layout.
    // Clone the size object so same-type nodes do not share mutated
    // coordinates through MIND_MAP_NODE_DIMENSIONS.
    g.setNode(n.id, {
      width: n.dimensions.width,
      height: n.dimensions.height,
    });
  }
  for (const e of edges.filter((edge) => edge.layout !== false)) {
    g.setEdge(e.source, e.target);
  }
  dagre.layout(g);
  let minX = Number.POSITIVE_INFINITY;
  let minY = Number.POSITIVE_INFINITY;
  for (const n of nodes) {
    const pos = g.node(n.id);
    if (pos) {
      // Dagre returns node centers; React Flow positions are top-left.
      n.position = {
        x: pos.x - n.dimensions.width / 2,
        y: pos.y - n.dimensions.height / 2,
      };
      minX = Math.min(minX, n.position.x);
      minY = Math.min(minY, n.position.y);
    }
  }
  if (!Number.isFinite(minX) || !Number.isFinite(minY)) {
    return { x: 0, y: 0, width: 0, height: 0 };
  }
  const dx = MAP_PADDING - minX;
  const dy = MAP_PADDING - minY;
  let maxX = 0;
  let maxY = 0;
  for (const n of nodes) {
    n.position = {
      x: Math.round(n.position.x + dx),
      y: Math.round(n.position.y + dy),
    };
    maxX = Math.max(maxX, n.position.x + n.dimensions.width);
    maxY = Math.max(maxY, n.position.y + n.dimensions.height);
  }
  return {
    x: 0,
    y: 0,
    width: Math.ceil(maxX + MAP_PADDING),
    height: Math.ceil(maxY + MAP_PADDING),
  };
}

function branchForRole(role: PlanTask["role"]): MindMapBranch {
  switch (role) {
    case "tester":
      return "test";
    case "reviewer":
      return "review";
    default:
      return "execution";
  }
}

function summarizeScope(paths: string[] | undefined): string | undefined {
  if (!paths || paths.length === 0) return undefined;
  if (paths.length === 1) return paths[0];
  return `${paths[0]} +${paths.length - 1}`;
}

function summarizeCriteria(criteria: PlanTask["criteria"]): string | undefined {
  if (!criteria) return undefined;
  if (criteria.summary) return criteria.summary;
  const parts: string[] = [];
  if (typeof criteria.min_audit_overall === "number") {
    parts.push(`audit >= ${Math.round(criteria.min_audit_overall * 100)}%`);
  }
  if (criteria.require_tests_pass === true) parts.push("tests must pass");
  if (criteria.forbid_new_security_flags === true) {
    parts.push("no new security flags");
  }
  return parts.length > 0 ? parts.join(", ") : undefined;
}

function buildTaskTooltip(
  task: PlanTask,
  scopeSummary: string | undefined,
  criteriaSummary: string | undefined,
  dependencyCount: number,
): string {
  return [
    task.title?.trim() || `Task ${task.index + 1}`,
    task.description,
    task.role ? `Role: ${task.role}` : null,
    scopeSummary ? `Scope: ${(task.scope_paths ?? []).join(", ")}` : null,
    dependencyCount > 0 ? `Depends on ${dependencyCount} task(s)` : null,
    criteriaSummary ? `Criteria: ${criteriaSummary}` : null,
  ]
    .filter(Boolean)
    .join("\n");
}

function summarizeEnvelope(envelopeFit: EnvelopeFit | null | undefined): {
  status: EnvelopeStatus;
  label: string;
  summary: string;
} {
  if (!envelopeFit) {
    return {
      status: "unknown",
      label: "No envelope",
      summary: "No envelope-fit assessment was supplied.",
    };
  }
  const entries = Object.entries(envelopeFit);
  const exceeds = entries.filter(([, bound]) => bound.fit === "exceeds");
  const nearLimit = entries.filter(([, bound]) => bound.fit === "near_limit");
  if (exceeds.length > 0) {
    return {
      status: "exceeds",
      label: "Exceeds bound",
      summary: summarizeBounds(exceeds),
    };
  }
  if (nearLimit.length > 0) {
    return {
      status: "near_limit",
      label: "Near limit",
      summary: summarizeBounds(nearLimit),
    };
  }
  return {
    status: "within",
    label: "Within bounds",
    summary: "All authority bounds are within the mission envelope.",
  };
}

function summarizeBounds(
  entries: Array<[string, EnvelopeFit[keyof EnvelopeFit]]>,
): string {
  return entries
    .map(([key, bound]) => {
      const label = key.charAt(0).toUpperCase() + key.slice(1);
      return bound.note ? `${label}: ${bound.note}` : label;
    })
    .join("\n");
}
