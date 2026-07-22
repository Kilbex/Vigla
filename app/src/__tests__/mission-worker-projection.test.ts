import { beforeEach, describe, expect, it } from "vitest";
import type { MissionEvent } from "../bindings";
import {
  selectGlobalCounters,
  selectWorkersNeedingReview,
  useOpsStore,
} from "../store";

type MissionEventBody = MissionEvent extends infer E
  ? E extends MissionEvent
    ? Omit<E, "mission_id" | "seq" | "ts">
    : never
  : never;

function event(
  seq: number,
  kind: MissionEventBody,
  missionId = "mission-projection",
): MissionEvent {
  return {
    ...kind,
    mission_id: missionId,
    seq,
    ts: `2026-07-21T12:00:0${seq}.000Z`,
  } as MissionEvent;
}

describe("mission worker operations projection", () => {
  beforeEach(() => useOpsStore.getState().reset());

  it("projects spawn through integration and releases activeCount", () => {
    const ingest = useOpsStore.getState().ingestMissionEvent;
    ingest(
      event(1, {
        type: "worker.spawned",
        payload: {
          worker_id: "mock-1",
          task_index: 0,
          task_title: "Implement parser",
          vendor: "codex",
          model: "gpt-5.5",
        },
      } as MissionEventBody),
    );
    expect(useOpsStore.getState().activeCount).toBe(1);
    expect(useOpsStore.getState().workers["mock-1"]).toMatchObject({
      missionScoped: true,
      missionId: "mission-projection",
      vendor: "codex",
      model: "gpt-5.5",
      state: "executing",
    });

    ingest(
      event(2, {
        type: "worker.result_submitted",
        payload: {
          worker_id: "mock-1",
          files: ["src/parser.ts"],
          summary: "Parser implemented",
        },
      }),
    );
    expect(useOpsStore.getState().workers["mock-1"].state).toBe(
      "reviewing",
    );

    ingest(
      event(3, {
        type: "supervisor.integrated",
        payload: {
          worker_id: "mock-1",
          integration_sha: "1234567890abcdef",
          snapshot_tag: "vigla/snap/mission-projection/0",
        },
      }),
    );
    const state = useOpsStore.getState();
    expect(state.workers["mock-1"].state).toBe("done");
    expect(state.workers["mock-1"].missionTimeline.map((row) => row.label)).toEqual([
      "Started",
      "Result submitted",
      "Integrated",
    ]);
    expect(state.activeCount).toBe(0);
    expect(selectWorkersNeedingReview(state)).toEqual([]);
    expect(selectGlobalCounters(state).needsInput).toBe(0);
  });

  it("keeps ID inference as a compatibility fallback for legacy recordings", () => {
    useOpsStore.getState().ingestMissionEvent(
      event(1, {
        type: "worker.spawned",
        payload: {
          worker_id: "wkr-gemini-0001",
          task_index: 0,
          task_title: "Replay legacy event",
        },
      }),
    );

    expect(useOpsStore.getState().workers["wkr-gemini-0001"]).toMatchObject({
      vendor: "gemini",
      model: null,
    });
  });

  it("does not erase a known model when a legacy duplicate omits identity", () => {
    const ingest = useOpsStore.getState().ingestMissionEvent;
    ingest(
      event(1, {
        type: "worker.spawned",
        payload: {
          worker_id: "wkr-codex-0001",
          task_index: 0,
          task_title: "Current event",
          vendor: "codex",
          model: "gpt-5.5",
        },
      }),
    );
    ingest(
      event(2, {
        type: "worker.spawned",
        payload: {
          worker_id: "wkr-codex-0001",
          task_index: 0,
          task_title: "Legacy duplicate",
        },
      }),
    );

    expect(useOpsStore.getState().workers["wkr-codex-0001"]).toMatchObject({
      vendor: "codex",
      model: "gpt-5.5",
      currentTaskTitle: "Legacy duplicate",
    });
  });

  it("replaces mission-scoped workers when the next mission reuses worker ids", () => {
    const store = useOpsStore.getState();
    store.registerWorker("standalone-worker", "codex", "Keep running");
    store.selectWorker("standalone-worker");

    store.ingestMissionEvent(
      event(
        0,
        {
          type: "mission.created",
          payload: {
            spec: {
              title: "First mission",
              objective: "Run two tasks",
              target_ref: "main",
              tests: null,
              supervisor_model: null,
              worker_model: null,
              worker_count: 2,
              confirm_plan: null,
            },
          },
        },
        "mission-one",
      ),
    );
    for (const [seq, id] of [
      [1, "wkr-claude-0001"],
      [2, "wkr-claude-0002"],
    ] as const) {
      store.ingestMissionEvent(
        event(
          seq,
          {
            type: "worker.spawned",
            payload: { worker_id: id, task_index: seq - 1, task_title: id },
          },
          "mission-one",
        ),
      );
    }
    store.ingestMissionEvent(
      event(
        3,
        {
          type: "supervisor.integrated",
          payload: {
            worker_id: "wkr-claude-0001",
            integration_sha: "1111111111111111",
            snapshot_tag: "vigla/snap/mission-one/0",
          },
        },
        "mission-one",
      ),
    );

    store.ingestMissionEvent(
      event(
        0,
        {
          type: "mission.created",
          payload: {
            spec: {
              title: "Second mission",
              objective: "Run one task",
              target_ref: "main",
              tests: null,
              supervisor_model: null,
              worker_model: null,
              worker_count: 1,
              confirm_plan: null,
            },
          },
        },
        "mission-two",
      ),
    );
    store.ingestMissionEvent(
      event(
        1,
        {
          type: "worker.spawned",
          payload: {
            worker_id: "wkr-claude-0001",
            task_index: 0,
            task_title: "Only task",
          },
        },
        "mission-two",
      ),
    );

    const state = useOpsStore.getState();
    const missionWorkers = Object.values(state.workers).filter(
      (worker) => worker.missionScoped,
    );
    expect(missionWorkers).toHaveLength(1);
    expect(missionWorkers[0]).toMatchObject({
      id: "wkr-claude-0001",
      missionId: "mission-two",
      currentTaskTitle: "Only task",
      state: "executing",
    });
    expect(missionWorkers[0].missionTimeline.map((row) => row.label)).toEqual([
      "Started",
    ]);
    expect(state.workers["wkr-claude-0002"]).toBeUndefined();
    expect(state.workers["standalone-worker"]).toMatchObject({
      missionScoped: false,
      currentTaskTitle: "Keep running",
    });
    expect(state.selectedWorkerId).toBe("standalone-worker");
    expect(state.activeCount).toBe(2);
    expect(state.needsInputCount).toBe(0);
  });

  it("terminalizes every in-flight mission worker on abort", () => {
    const ingest = useOpsStore.getState().ingestMissionEvent;
    for (const [seq, id] of [
      [1, "wkr-codex-0001"],
      [2, "wkr-claude-0002"],
    ] as const) {
      ingest(
        event(seq, {
          type: "worker.spawned",
          payload: { worker_id: id, task_index: seq - 1, task_title: id },
        }),
      );
    }
    expect(useOpsStore.getState().activeCount).toBe(2);

    ingest(
      event(3, {
        type: "mission.aborted",
        payload: { reason: "operator stopped the mission" },
      }),
    );

    const state = useOpsStore.getState();
    expect(state.activeCount).toBe(0);
    expect(state.workers["wkr-codex-0001"].state).toBe("failed");
    expect(state.workers["wkr-claude-0002"].state).toBe("failed");
  });
});
