import type { MissionEvent, Vendor, WorkerState } from "../bindings";
import { initialReplayState } from "../replay/state";
import {
  alertId,
  emptyState,
  fresh,
  MAX_ALERTS,
  shortId,
} from "./ingest";
import { applyToOpsState } from "./ops-state";
import type { StoreSliceContext } from "./context";
import type { OpsState, OpsStore } from "./types";

type WorkerActions = Pick<
  OpsStore,
  | "ingest"
  | "reset"
  | "selectWorker"
  | "registerWorker"
  | "registerMissionWorker"
  | "updateMissionWorker"
  | "finishMissionWorkers"
  | "ingestMissionEvent"
  | "setWorkerModel"
>;

export function createWorkerSlice({ set, get }: StoreSliceContext): WorkerActions {
  return {
    ingest: (event) =>
      set((prev) => {
        if (prev.replay.mode === "replay") {
          // Shadow into liveSnapshot so exitReplay can restore the
          // live ops room with everything that happened during replay.
          if (!prev.liveSnapshot) {
            // Should never happen — enterReplay always seeds liveSnapshot.
            // If it does happen, a future regression in any action that
            // routes through here (e.g. registerWorker) is dropping
            // live events silently. Surface the dropped event rather
            // than absorb it.
            console.warn(
              "[store] ingest: replay mode but liveSnapshot is null — event dropped",
              event.worker_id,
            );
            return prev;
          }
          return {
            liveSnapshot: applyToOpsState(prev.liveSnapshot, event),
          };
        }
        // Live mode — apply to visible state.
        return applyToOpsState(prev, event);
      }),

    reset: () =>
      set({ ...emptyState(), liveSnapshot: null, replay: initialReplayState }),

    selectWorker: (workerId) => set({ selectedWorkerId: workerId }),

    registerWorker: (id, vendor, taskTitle, model) =>
      set((prev) => {
        const isReplay = prev.replay.mode === "replay";
        const target: OpsState | null = isReplay ? prev.liveSnapshot : prev;
        if (!target) {
          // Should never happen — enterReplay always seeds liveSnapshot.
          // Surface the dropped registration so a future regression
          // doesn't silently revive the vendor: "mock" identity bug.
          console.warn(
            "[store] registerWorker: replay mode but liveSnapshot is null — registration dropped",
            id,
          );
          return prev;
        }

        const existing = target.workers[id];

        if (existing) {
          // Patch path: vendor + currentTaskTitle only.
          // Preserve every event-derived field. No alert push.
          const patched = {
            ...existing,
            vendor,
            model: model ?? existing.model,
            currentTaskTitle: taskTitle,
          };
          const newWorkers = { ...target.workers, [id]: patched };
          return isReplay
            ? { liveSnapshot: { ...prev.liveSnapshot!, workers: newWorkers } }
            : { workers: newWorkers };
        }

        return registerNewWorker(prev, target, isReplay, id, vendor, taskTitle, model);
      }),

    registerMissionWorker: (id, vendor, taskTitle, missionId) =>
      set((prev) => {
        const isReplay = prev.replay.mode === "replay";
        const target: OpsState | null = isReplay ? prev.liveSnapshot : prev;
        if (!target) return prev;
        const existing = target.workers[id];
        if (existing) {
          const workers = {
            ...target.workers,
            [id]: {
              ...existing,
              vendor,
              currentTaskTitle: taskTitle,
              missionScoped: true,
              missionId,
            },
          };
          return isReplay
            ? { liveSnapshot: { ...prev.liveSnapshot!, workers } }
            : { workers };
        }
        return registerNewWorker(
          prev,
          target,
          isReplay,
          id,
          vendor,
          taskTitle,
          undefined,
          { missionScoped: true, missionId },
        );
      }),

    updateMissionWorker: (id, update) =>
      set((prev) => {
        const isReplay = prev.replay.mode === "replay";
        const target: OpsState | null = isReplay ? prev.liveSnapshot : prev;
        const existing = target?.workers[id];
        if (!target || !existing || !existing.missionScoped) return prev;
        const nextState = update.state ?? existing.state;
        const wasTerminal = isTerminalWorkerState(existing.state);
        const nextTerminal = isTerminalWorkerState(nextState);
        let activeCount = target.activeCount;
        let needsInputCount = target.needsInputCount;
        const needsReview =
          target.reviewStatus[id] === undefined ||
          target.reviewStatus[id] === "needs_review";
        if (!wasTerminal && nextTerminal) {
          activeCount -= 1;
          if (needsReview) needsInputCount += 1;
        } else if (wasTerminal && !nextTerminal) {
          activeCount += 1;
          if (needsReview) needsInputCount -= 1;
        }
        const worker = {
          ...existing,
          state: nextState,
          progressNote: update.note ?? existing.progressNote,
          completionSummary:
            update.completionSummary ?? existing.completionSummary,
          failureSummary: update.failureSummary ?? existing.failureSummary,
          filesModified: update.filesChanged ?? existing.filesModified,
          missionTimeline: [...existing.missionTimeline, update.timeline].slice(-100),
          eventCount: existing.eventCount + 1,
          flashUntil:
            nextState === "done" && !wasTerminal ? Date.now() + 1800 : existing.flashUntil,
        };
        const workers = { ...target.workers, [id]: worker };
        const patch = {
          workers,
          activeCount,
          needsInputCount,
          totalEvents: target.totalEvents + 1,
        };
        return isReplay
          ? { liveSnapshot: { ...prev.liveSnapshot!, ...patch } }
          : patch;
      }),

    finishMissionWorkers: (missionId, state, timeline) => {
      const current = get();
      const target =
        current.replay.mode === "replay" ? current.liveSnapshot : current;
      const ids = Object.values(target?.workers ?? {})
        .filter(
          (worker) =>
            worker.missionScoped &&
            worker.missionId === missionId &&
            !isTerminalWorkerState(worker.state),
        )
        .map((worker) => worker.id);
      for (const id of ids) {
        get().updateMissionWorker(id, {
          state,
          note: timeline.detail ?? timeline.label,
          ...(state === "done"
            ? { completionSummary: timeline.detail ?? timeline.label }
            : { failureSummary: timeline.detail ?? timeline.label }),
          timeline,
        });
      }
    },

    ingestMissionEvent: (event) => {
      ingestMissionWorkerProjection(event, get());
    },

    setWorkerModel: (id, model) =>
      set((prev) => {
        const isReplay = prev.replay.mode === "replay";
        const target: OpsState | null = isReplay ? prev.liveSnapshot : prev;
        const existing = target?.workers[id];
        if (!target || !existing) return prev;
        const newWorkers = {
          ...target.workers,
          [id]: { ...existing, model },
        };
        return isReplay
          ? { liveSnapshot: { ...prev.liveSnapshot!, workers: newWorkers } }
          : { workers: newWorkers };
      }),
  };
}

function registerNewWorker(
  prev: OpsStore,
  target: OpsState,
  isReplay: boolean,
  id: string,
  vendor: Vendor,
  taskTitle: string | null,
  model: string | null | undefined,
  mission?: { missionScoped: boolean; missionId: string },
) {
  const spawnedAt = Date.now();
  const newSnapshot = fresh(id, spawnedAt, {
    vendor,
    model,
    currentTaskTitle: taskTitle,
    missionScoped: mission?.missionScoped,
    missionId: mission?.missionId,
  });
  const startedAlert = {
    id: alertId(id, 0, "started" as const),
    kind: "started" as const,
    workerId: id,
    workerShortId: shortId(id),
    title: "spawned",
    detail: null,
    ts: spawnedAt,
  };
  const newWorkers = { ...target.workers, [id]: newSnapshot };
  const newOrder = [...target.workerOrder, id];
  const newAlerts = [startedAlert, ...target.alerts].slice(0, MAX_ALERTS);

  if (isReplay) {
    return {
      liveSnapshot: {
        ...prev.liveSnapshot!,
        workers: newWorkers,
        workerOrder: newOrder,
        alerts: newAlerts,
        // P3 — `fresh()` starts in `idle` (non-terminal); count it.
        activeCount: prev.liveSnapshot!.activeCount + 1,
      },
    };
  }
  return {
    workers: newWorkers,
    workerOrder: newOrder,
    alerts: newAlerts,
    activeCount: prev.activeCount + 1,
  };
}

function isTerminalWorkerState(state: WorkerState): boolean {
  return state === "done" || state === "failed";
}

function vendorFromWorkerId(workerId: string): Vendor {
  const match = /^wkr-([a-z][a-z0-9_-]*?)-0*[0-9]+$/i.exec(workerId);
  const vendor = match?.[1]?.toLowerCase();
  switch (vendor) {
    case "claude":
    case "codex":
    case "gemini":
    case "antigravity":
    case "kiro":
    case "copilot":
    case "opencode":
    case "mock":
      return vendor;
    default:
      return "mock";
  }
}

function timeline(event: MissionEvent, label: string, detail?: string | null) {
  return { ts: event.ts, label, detail: detail ?? null };
}

function ingestMissionWorkerProjection(event: MissionEvent, store: OpsStore): void {
  switch (event.type) {
    case "worker.spawned": {
      const id = event.payload.worker_id;
      store.registerMissionWorker(
        id,
        vendorFromWorkerId(id),
        event.payload.task_title,
        event.mission_id,
      );
      store.updateMissionWorker(id, {
        state: "executing",
        note: "Worker started",
        timeline: timeline(event, "Started", event.payload.task_title),
      });
      break;
    }
    case "worker.progress":
      store.updateMissionWorker(event.payload.worker_id, {
        state: "executing",
        note: event.payload.note,
        timeline: timeline(event, "Progress", event.payload.note),
      });
      break;
    case "worker.result_submitted":
      store.updateMissionWorker(event.payload.worker_id, {
        state: "reviewing",
        note: "Result submitted for review",
        completionSummary: event.payload.summary,
        filesChanged: event.payload.files.length,
        timeline: timeline(event, "Result submitted", event.payload.summary),
      });
      break;
    case "supervisor.review_started":
      store.updateMissionWorker(event.payload.worker_id, {
        state: "reviewing",
        note: "Supervisor review in progress",
        timeline: timeline(event, "Review started"),
      });
      break;
    case "supervisor.integrated":
      store.updateMissionWorker(event.payload.worker_id, {
        state: "done",
        note: "Integrated into the mission branch",
        completionSummary: `Integrated at ${event.payload.integration_sha.slice(0, 12)}`,
        timeline: timeline(
          event,
          "Integrated",
          event.payload.integration_sha.slice(0, 12),
        ),
      });
      break;
    case "supervisor.post_integration_audit_completed":
      store.updateMissionWorker(event.payload.worker_id, {
        timeline: timeline(
          event,
          "Post-integration audit",
          `score ${event.payload.overall.toFixed(2)}`,
        ),
      });
      break;
    case "arbiter.decided": {
      let kind: string | undefined;
      try {
        kind = (JSON.parse(event.payload.decision_json) as { kind?: string }).kind;
      } catch {
        kind = undefined;
      }
      const terminal = kind === "scrub" || kind === "escalate";
      store.updateMissionWorker(event.payload.worker_id, {
        ...(terminal
          ? {
              state: "failed" as const,
              failureSummary: `Arbiter decision: ${kind}`,
            }
          : {}),
        note: kind ? `Arbiter: ${kind}` : "Arbiter decision recorded",
        timeline: timeline(
          event,
          "Arbiter decision",
          kind ?? "unrecognized decision",
        ),
      });
      break;
    }
    case "mission.completed":
      store.finishMissionWorkers(
        event.mission_id,
        "done",
        timeline(event, "Mission complete", event.payload.summary),
      );
      break;
    case "mission.attention_ready":
      store.finishMissionWorkers(
        event.mission_id,
        "failed",
        timeline(event, "Mission needs attention"),
      );
      break;
    case "mission.aborted":
      store.finishMissionWorkers(
        event.mission_id,
        "failed",
        timeline(event, "Mission aborted", event.payload.reason),
      );
      break;
  }
}
