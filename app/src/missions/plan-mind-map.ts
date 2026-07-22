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
import {
  PLAN_CONTENT_LIMITS,
  sanitizePlanDetail,
  sanitizePlanLabel,
} from "./plan-content";

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
  truncation_note?: string;
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
  source_task_count?: number;
  generation: number;
  overview?: string | null;
  tech_stack?: TechChoice[] | null;
  source_tech_stack_count?: number;
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
  const rawTasks = Array.isArray(plan.tasks) ? plan.tasks : [];
  const {
    tasks,
    omittedTasks,
    omittedDependencies,
    omittedScopePaths,
  } = prepareTasks(rawTasks);
  const rawTechStack = Array.isArray(plan.tech_stack) ? plan.tech_stack : [];
  const sourceTaskCount = boundedSourceCount(
    plan.source_task_count,
    rawTasks.length,
  );
  const sourceTechStackCount = boundedSourceCount(
    plan.source_tech_stack_count,
    rawTechStack.length,
  );
  const techStack = rawTechStack
    .slice(0, PLAN_CONTENT_LIMITS.techItems)
    .flatMap((item) => {
      if (!item || typeof item !== "object") return [];
      return [
        {
          layer: sanitizePlanLabel(item.layer) || "layer",
          choice: sanitizePlanLabel(item.choice) || "choice",
          rationale: sanitizePlanDetail(item.rationale),
          is_new: item.is_new === true,
        },
      ];
    });
  const omittedTechItems = Math.max(
    0,
    sourceTechStackCount - techStack.length,
  );
  const truncationNote = summarizeTruncation(
    omittedTasks + Math.max(0, sourceTaskCount - rawTasks.length),
    omittedTechItems,
    omittedDependencies,
    omittedScopePaths,
  );
  const taskByIndex = new Map(tasks.map((task) => [task.index, task]));

  const envelope = summarizeEnvelope(plan.envelope_fit);
  const title = sanitizePlanLabel(spec.title) || "Mission";
  const objective = sanitizePlanDetail(spec.objective);

  // Root.
  nodes.push({
    id: "root",
    type: "root",
    position: { x: 0, y: 0 },
    dimensions: MIND_MAP_NODE_DIMENSIONS.root,
    data: {
      label: title,
      subtitle: "Mission",
      objective,
      branch: envelope.status === "exceeds" ? "risk" : "root",
      tooltip: [title, objective, envelope.summary, truncationNote]
        .filter(Boolean)
        .join("\n\n"),
      envelope_flag: envelope.status !== "within" && envelope.status !== "unknown",
      envelope_status: envelope.status,
      envelope_label: envelope.label,
      envelope_summary: envelope.summary,
      truncation_note: truncationNote,
    },
  });

  // Tech-stack subtree.
  if (techStack.length > 0) {
    nodes.push({
      id: "tech-root",
      type: "tech-root",
      position: { x: 0, y: 0 },
      dimensions: MIND_MAP_NODE_DIMENSIONS["tech-root"],
      data: {
        label: "Tech stack",
        subtitle: `${techStack.length} item${
          techStack.length === 1 ? "" : "s"
        }`,
        branch: "tech",
        tooltip: "Technology choices proposed for this mission.",
        task_count: techStack.length,
      },
    });
    edges.push(hierarchyEdge("root->tech-root", "root", "tech-root", "tech"));
    techStack.forEach((t, idx) => {
      const id = `tech-${idx}`;
      const layer = t.layer;
      const choice = t.choice;
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
  for (const t of tasks) {
    indegree[t.index] = 0;
    dependents[t.index] = [];
  }
  for (const t of tasks) {
    for (const dep of t.depends_on ?? []) {
      if (!taskByIndex.has(dep)) continue;
      indegree[t.index] = (indegree[t.index] ?? 0) + 1;
      dependents[dep] = [...(dependents[dep] ?? []), t.index];
    }
  }
  const waves: number[][] = [];
  let frontier = tasks
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
  const unscheduled = tasks
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
  for (const t of tasks) {
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

function boundedSourceCount(value: unknown, minimum: number): number {
  return Number.isSafeInteger(value) && Number(value) >= minimum
    ? Number(value)
    : minimum;
}

function prepareTasks(rawTasks: readonly PlanTask[]): {
  tasks: PlanTask[];
  omittedTasks: number;
  omittedDependencies: number;
  omittedScopePaths: number;
} {
  const staged: Array<{ task: PlanTask; rawDependencies: readonly number[] }> = [];
  const seen = new Set<number>();
  let omittedScopePaths = 0;

  for (const raw of rawTasks.slice(0, PLAN_CONTENT_LIMITS.tasks)) {
    if (
      !raw ||
      typeof raw !== "object" ||
      !Number.isSafeInteger(raw.index) ||
      raw.index < 0 ||
      seen.has(raw.index)
    ) {
      continue;
    }
    seen.add(raw.index);
    const rawCriteria =
      raw.criteria &&
      typeof raw.criteria === "object" &&
      !Array.isArray(raw.criteria)
        ? raw.criteria
        : null;
    const rawScopePaths = Array.isArray(raw.scope_paths)
      ? raw.scope_paths
      : [];
    omittedScopePaths += Math.max(
      0,
      rawScopePaths.length - PLAN_CONTENT_LIMITS.scopePathsPerTask,
    );
    staged.push({
      task: {
        index: raw.index,
        title:
          sanitizePlanLabel(raw.title) || `Task ${staged.length + 1}`,
        description: sanitizePlanDetail(raw.description),
        role: sanitizePlanLabel(raw.role),
        criteria: rawCriteria
          ? {
              min_audit_overall: Number.isFinite(
                rawCriteria.min_audit_overall,
              )
                ? Math.min(
                    1,
                    Math.max(0, Number(rawCriteria.min_audit_overall)),
                  )
                : undefined,
              require_tests_pass: rawCriteria.require_tests_pass === true,
              forbid_new_security_flags:
                rawCriteria.forbid_new_security_flags === true,
              summary: sanitizePlanDetail(rawCriteria.summary),
            }
          : undefined,
        criteria_summary: sanitizePlanDetail(raw.criteria_summary),
        scope_paths: rawScopePaths
          .slice(0, PLAN_CONTENT_LIMITS.scopePathsPerTask)
          .map(sanitizePlanLabel)
          .filter(Boolean),
        depends_on: [],
      },
      rawDependencies: Array.isArray(raw.depends_on) ? raw.depends_on : [],
    });
  }

  const visibleIndexes = new Set(staged.map(({ task }) => task.index));
  let acceptedDependencies = 0;
  let omittedDependencies = 0;
  let remainingDependencyInputs = PLAN_CONTENT_LIMITS.dependencyInputs;
  for (const entry of staged) {
    const unique = new Set<number>();
    const inspectedCount = Math.min(
      entry.rawDependencies.length,
      remainingDependencyInputs,
    );
    omittedDependencies += entry.rawDependencies.length - inspectedCount;
    remainingDependencyInputs -= inspectedCount;
    for (let index = 0; index < inspectedCount; index += 1) {
      const dependency = entry.rawDependencies[index];
      if (
        !Number.isSafeInteger(dependency) ||
        !visibleIndexes.has(dependency) ||
        unique.has(dependency)
      ) {
        continue;
      }
      unique.add(dependency);
      if (acceptedDependencies < PLAN_CONTENT_LIMITS.dependencyEdges) {
        entry.task.depends_on?.push(dependency);
        acceptedDependencies += 1;
      } else {
        omittedDependencies += 1;
      }
    }
  }

  return {
    tasks: staged.map(({ task }) => task),
    omittedTasks: Math.max(0, rawTasks.length - staged.length),
    omittedDependencies,
    omittedScopePaths,
  };
}

function summarizeTruncation(
  tasks: number,
  stackItems: number,
  dependencies: number,
  scopePaths: number,
): string | undefined {
  const parts: string[] = [];
  if (tasks > 0) parts.push(`${tasks} task${tasks === 1 ? "" : "s"}`);
  if (stackItems > 0) {
    parts.push(`${stackItems} stack item${stackItems === 1 ? "" : "s"}`);
  }
  if (dependencies > 0) {
    parts.push(
      `${dependencies} dependenc${dependencies === 1 ? "y" : "ies"}`,
    );
  }
  if (scopePaths > 0) {
    parts.push(`${scopePaths} scope path${scopePaths === 1 ? "" : "s"}`);
  }
  if (parts.length === 0) return undefined;
  const summary =
    parts.length === 1
      ? parts[0]
      : parts.length === 2
        ? `${parts[0]} and ${parts[1]}`
        : `${parts.slice(0, -1).join(", ")}, and ${parts[parts.length - 1]}`;
  return `${summary} omitted for a responsive preview.`;
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
      const note = sanitizePlanDetail(bound.note);
      return note ? `${label}: ${note}` : label;
    })
    .join("\n");
}
