import type { Event, Vendor, WorkerState } from "../bindings";
import type {
  Alert,
  AlertKind,
  DependencyEdge,
  OpsState,
  WorkerSnapshot,
} from "./types";
import { MAX_EVENTS_PER_WORKER } from "./types";

export const MAX_ALERTS = 24;
const FLASH_MS = 1800;
const EMPTY_EDGES: DependencyEdge[] = [];

function toMs(ts: string): number {
  const ms = Date.parse(ts);
  return Number.isFinite(ms) ? ms : Date.now();
}

export function shortId(id: string): string {
  const missionWorker = /^wkr-([a-z][a-z0-9_-]*?)-0*([0-9]+)$/i.exec(id);
  if (missionWorker) {
    return `${vendorDisplayName(missionWorker[1])} · ${Number.parseInt(
      missionWorker[2],
      10,
    )}`;
  }
  return id.length > 8 ? id.slice(-8) : id;
}

function vendorDisplayName(value: string): string {
  switch (value.toLowerCase()) {
    case "claude":
      return "Claude";
    case "codex":
      return "Codex";
    case "gemini":
      return "Gemini";
    case "antigravity":
      return "Antigravity";
    case "kiro":
      return "Kiro";
    case "copilot":
      return "Copilot";
    case "opencode":
      return "OpenCode";
    case "mock":
      return "Mock";
    default:
      return value;
  }
}

export function fresh(
  workerId: string,
  spawnedAt: number,
  overrides?: {
    vendor?: Vendor;
    model?: string | null;
    currentTaskTitle?: string | null;
    missionScoped?: boolean;
    missionId?: string | null;
  },
): WorkerSnapshot {
  return {
    id: workerId,
    shortId: shortId(workerId),
    vendor: overrides?.vendor ?? "mock",
    model: overrides?.model ?? null,
    state: "idle",
    spawnedAt,
    currentTaskId: null,
    currentTaskTitle: overrides?.currentTaskTitle ?? null,
    progress: null,
    etaMs: null,
    progressNote: null,
    filesAdded: 0,
    filesModified: 0,
    filesDeleted: 0,
    linesAdded: 0,
    linesRemoved: 0,
    testsPassed: 0,
    testsFailed: 0,
    testsSkipped: 0,
    lastSuite: null,
    costUsd: 0,
    inputTokens: 0,
    outputTokens: 0,
    recentLog: null,
    blockedOn: null,
    failureSummary: null,
    completionSummary: null,
    flashUntil: 0,
    eventCount: 0,
    missionScoped: overrides?.missionScoped ?? false,
    missionId: overrides?.missionId ?? null,
    missionTimeline: [],
  };
}

export function pushAlert(state: OpsState, alert: Alert) {
  state.alerts = [alert, ...state.alerts].slice(0, MAX_ALERTS);
}

export function alertId(workerId: string, seq: number, kind: AlertKind): string {
  return `${workerId}-${seq}-${kind}`;
}


/**
 * Apply one event to the ops state. The function mutates in place;
 * callers using zustand's set() should clone the state first (the
 * store wrapper does that).
 *
 * Reducer matches `docs/event-schema.md` §4. Forward-compat: an
 * unrecognized event-kind discriminator is silently tolerated (we
 * only bump eventCount).
 */
export function applyEvent(state: OpsState, event: Event): void {
  const workerId = event.worker_id;
  if (!state.workers[workerId]) {
    state.workers[workerId] = fresh(workerId, toMs(event.ts));
    state.workerOrder = [...state.workerOrder, workerId];
    // P3 — `fresh()` starts the worker in `idle` (non-terminal).
    // Counted as active until a state_change moves it to done/failed.
    state.activeCount += 1;
    pushAlert(state, {
      id: alertId(workerId, event.seq, "started"),
      kind: "started",
      workerId,
      workerShortId: shortId(workerId),
      title: "spawned",
      detail: null,
      ts: toMs(event.ts),
    });
  }
  // Schema §6: detect regressions and gaps. Never reorder; mark.
  const lastSeq = state.lastSeqByWorker[workerId];
  if (lastSeq !== undefined) {
    if (event.seq <= lastSeq) {
      pushAlert(state, {
        id: alertId(workerId, event.seq, "seq_gap"),
        kind: "seq_gap",
        workerId,
        workerShortId: shortId(workerId),
        title: "seq regression",
        detail: `seq ${event.seq} ≤ last ${lastSeq}`,
        ts: toMs(event.ts),
      });
    } else if (event.seq > lastSeq + 1) {
      pushAlert(state, {
        id: alertId(workerId, event.seq, "seq_gap"),
        kind: "seq_gap",
        workerId,
        workerShortId: shortId(workerId),
        title: "seq gap",
        detail: `missing ${event.seq - lastSeq - 1} between ${lastSeq} and ${event.seq}`,
        ts: toMs(event.ts),
      });
    }
  }
  // Track the maximum seq seen — never go backwards even on regression.
  state.lastSeqByWorker[workerId] = Math.max(lastSeq ?? -1, event.seq);

  const worker = state.workers[workerId];
  worker.eventCount += 1;
  state.totalEvents += 1;

  // Append to bounded per-worker event log (powers the drawer feed
  // in Step 8). Drop oldest when over cap.
  //
  // Mutate in place rather than `[...existing, event]` / `.slice()` —
  // each call here ran in O(N) and allocated two new arrays per
  // event, accumulating GC pressure during long missions. The store
  // already produces a new object via Immer / shallow copy at the
  // slice level, so in-place push/shift on the inner array is safe.
  let bucket = state.workerEvents[workerId];
  if (!bucket) {
    bucket = [];
    state.workerEvents[workerId] = bucket;
  }
  bucket.push(event);
  while (bucket.length > MAX_EVENTS_PER_WORKER) {
    bucket.shift();
  }

  if (event.task_id) {
    worker.currentTaskId = event.task_id;
    // taskOwner tracks the most recent worker to emit for this task,
    // mirroring tasks[].workerId. Earlier code only set on first
    // occurrence, which left dependency edges pointing at the dead
    // tile when a task was retried by a different worker.
    state.taskOwner[event.task_id] = workerId;
    if (!state.tasks[event.task_id]) {
      state.tasks[event.task_id] = {
        id: event.task_id,
        title: "(task)",
        workerId,
        state: null,
        completedAt: null,
      };
    } else {
      state.tasks[event.task_id].workerId = workerId;
    }
  }

  switch (event.type) {
    case "state_change": {
      const next = event.payload.state as WorkerState;
      const prevState = worker.state;
      const wasTerminal = prevState === "done" || prevState === "failed";
      const nextTerminal = next === "done" || next === "failed";
      // P3 — keep activeCount / needsInputCount in lock-step with
      // the boundary crossing. `needsInputCount` mirrors
      // `computeNeedsReview`: done/failed workers whose
      // reviewStatus is undefined or "needs_review".
      if (!wasTerminal && nextTerminal) {
        state.activeCount -= 1;
        const rs = state.reviewStatus[workerId];
        if (rs === undefined || rs === "needs_review") {
          state.needsInputCount += 1;
        }
      } else if (wasTerminal && !nextTerminal) {
        state.activeCount += 1;
        const rs = state.reviewStatus[workerId];
        if (rs === undefined || rs === "needs_review") {
          state.needsInputCount -= 1;
        }
      }
      worker.state = next;
      if (event.payload.note) {
        worker.progressNote = event.payload.note;
      }
      if (event.task_id && state.tasks[event.task_id]) {
        state.tasks[event.task_id].state = next;
      }
      if (next === "blocked") {
        pushAlert(state, {
          id: alertId(workerId, event.seq, "blocked"),
          kind: "blocked",
          workerId,
          workerShortId: shortId(workerId),
          title: "blocked",
          detail: event.payload.note ?? null,
          ts: toMs(event.ts),
        });
      } else if (next === "executing" && event.payload.from === "blocked") {
        pushAlert(state, {
          id: alertId(workerId, event.seq, "unblocked"),
          kind: "unblocked",
          workerId,
          workerShortId: shortId(workerId),
          title: "resumed",
          detail: event.payload.note ?? null,
          ts: toMs(event.ts),
        });
      } else if (next === "done") {
        worker.flashUntil = toMs(event.ts) + FLASH_MS;
        if (event.task_id && state.tasks[event.task_id]) {
          state.tasks[event.task_id].completedAt = toMs(event.ts);
        }
      } else if (next === "failed") {
        // Failure alert is added on the `failure` event; state_change
        // failed is the visual-only signal.
      }
      break;
    }
    case "log":
      worker.recentLog = event.payload.line;
      break;
    case "progress":
      worker.progress = event.payload.percent;
      worker.etaMs = event.payload.eta_ms ?? null;
      if (event.payload.note) worker.progressNote = event.payload.note;
      break;
    case "file_activity": {
      const op = event.payload.op;
      if (op === "created") worker.filesAdded += 1;
      else if (op === "modified") worker.filesModified += 1;
      else if (op === "deleted") worker.filesDeleted += 1;
      worker.linesAdded += event.payload.lines_added ?? 0;
      worker.linesRemoved += event.payload.lines_removed ?? 0;
      break;
    }
    case "test_result":
      worker.testsPassed = event.payload.passed;
      worker.testsFailed = event.payload.failed;
      worker.testsSkipped = event.payload.skipped;
      worker.lastSuite = event.payload.suite;
      break;
    case "cost":
      worker.costUsd += event.payload.usd;
      worker.inputTokens += event.payload.input_tokens;
      worker.outputTokens += event.payload.output_tokens;
      if (event.payload.model) worker.model = event.payload.model;
      state.totalSpendUsd += event.payload.usd;
      break;
    case "dependency":
      worker.blockedOn = event.payload.waiting_on;
      break;
    case "completion": {
      worker.completionSummary = event.payload.summary;
      pushAlert(state, {
        id: alertId(workerId, event.seq, "completion"),
        kind: "completion",
        workerId,
        workerShortId: shortId(workerId),
        title: "step done",
        detail: event.payload.summary,
        ts: toMs(event.ts),
      });
      break;
    }
    case "failure": {
      worker.failureSummary = event.payload.error;
      pushAlert(state, {
        id: alertId(workerId, event.seq, "failure"),
        kind: "failure",
        workerId,
        workerShortId: shortId(workerId),
        title: "failed",
        detail: event.payload.error,
        ts: toMs(event.ts),
      });
      break;
    }
  }

  // Recompute derived dependency edges, but only replace the stored
  // ref if the content actually changed — otherwise the React Flow
  // canvas would re-route and reset edge animations on every event.
  const recomputed = computeDependencyEdges(state);
  if (!edgesEqual(state.derivedDependencyEdges, recomputed)) {
    state.derivedDependencyEdges = recomputed;
  }
}

function computeDependencyEdges(state: OpsState): DependencyEdge[] {
  const edges: DependencyEdge[] = [];
  for (const w of Object.values(state.workers)) {
    if (!w.blockedOn) continue;
    for (const upstreamTaskId of w.blockedOn) {
      const sourceWorkerId = state.taskOwner[upstreamTaskId];
      if (!sourceWorkerId || sourceWorkerId === w.id) continue;
      const upstream = state.tasks[upstreamTaskId];
      const stateLabel: DependencyEdge["state"] = upstream?.completedAt
        ? "done"
        : w.state === "blocked"
          ? "blocked"
          : "pending";
      edges.push({
        id: `${sourceWorkerId}->${w.id}`,
        source: sourceWorkerId,
        target: w.id,
        state: stateLabel,
      });
    }
  }
  return edges.length === 0 ? EMPTY_EDGES : edges;
}

function edgesEqual(a: DependencyEdge[], b: DependencyEdge[]): boolean {
  if (a === b) return true;
  if (a.length !== b.length) return false;
  for (let i = 0; i < a.length; i++) {
    const ae = a[i];
    const be = b[i];
    if (
      ae.id !== be.id ||
      ae.source !== be.source ||
      ae.target !== be.target ||
      ae.state !== be.state
    ) {
      return false;
    }
  }
  return true;
}

export function emptyState(): OpsState {
  return {
    workers: {},
    workerOrder: [],
    tasks: {},
    taskOwner: {},
    workerEvents: {},
    alerts: [],
    totalEvents: 0,
    totalSpendUsd: 0,
    activeCount: 0,
    needsInputCount: 0,
    selectedWorkerId: null,
    derivedDependencyEdges: EMPTY_EDGES,
    lastSeqByWorker: {},
    squads: {},
    squadOrder: [],
    workerSquad: {},
    reviewStatus: {},
    reviewFocusedWorkerId: null,
  };
}
