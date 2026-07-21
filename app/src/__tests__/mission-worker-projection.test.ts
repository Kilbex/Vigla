import { beforeEach, describe, expect, it } from "vitest";
import type { MissionEvent } from "../bindings";
import { useOpsStore } from "../store";

type MissionEventBody = MissionEvent extends infer E
  ? E extends MissionEvent
    ? Omit<E, "mission_id" | "seq" | "ts">
    : never
  : never;

function event(
  seq: number,
  kind: MissionEventBody,
): MissionEvent {
  return {
    ...kind,
    mission_id: "mission-projection",
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
          worker_id: "wkr-claude-0001",
          task_index: 0,
          task_title: "Implement parser",
        },
      }),
    );
    expect(useOpsStore.getState().activeCount).toBe(1);
    expect(useOpsStore.getState().workers["wkr-claude-0001"]).toMatchObject({
      missionScoped: true,
      missionId: "mission-projection",
      state: "executing",
    });

    ingest(
      event(2, {
        type: "worker.result_submitted",
        payload: {
          worker_id: "wkr-claude-0001",
          files: ["src/parser.ts"],
          summary: "Parser implemented",
        },
      }),
    );
    expect(useOpsStore.getState().workers["wkr-claude-0001"].state).toBe(
      "reviewing",
    );

    ingest(
      event(3, {
        type: "supervisor.integrated",
        payload: {
          worker_id: "wkr-claude-0001",
          integration_sha: "1234567890abcdef",
          snapshot_tag: "vigla/snap/mission-projection/0",
        },
      }),
    );
    const state = useOpsStore.getState();
    expect(state.workers["wkr-claude-0001"].state).toBe("done");
    expect(state.workers["wkr-claude-0001"].missionTimeline.map((row) => row.label)).toEqual([
      "Started",
      "Result submitted",
      "Integrated",
    ]);
    expect(state.activeCount).toBe(0);
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
