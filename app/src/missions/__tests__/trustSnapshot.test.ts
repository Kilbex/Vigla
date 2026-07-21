// @vitest-environment jsdom

import { beforeEach, describe, expect, it } from "vitest";
import type { ActiveMission } from "../types";
import {
  finalRollbackTagForMission,
  loadMissionTrustSnapshot,
  snapshotFromMission,
} from "../trustSnapshot";

describe("mission trust snapshots", () => {
  beforeEach(() => window.localStorage.clear());

  it("derives the persistent final-merge anchor from mission and target", () => {
    expect(finalRollbackTagForMission("msn-1", "release/v1")).toBe(
      "vigla/revert/msn-1/before/release/v1",
    );
  });

  it("stores the target ref and final-merge rollback anchor", () => {
    const mission = {
      id: "msn-1",
      spec: { title: "Ship", target_ref: "main" },
      lifecycle: "merged",
      startedAt: "2026-07-21T12:00:00Z",
      updatedAt: "2026-07-21T12:01:00Z",
      statusLine: "Merged",
      completionSummary: "Done",
      audit: null,
      auditPayloadJson: null,
      verdict: null,
      testsPassed: null,
      filesChanged: 0,
      workers: {},
      tasks: [],
      resolution: { type: "merged" },
    } as unknown as ActiveMission;

    const snapshot = snapshotFromMission(mission);
    expect(snapshot.targetRef).toBe("main");
    expect(snapshot.rollbackAnchor).toBe("vigla/revert/msn-1/before/main");
  });

  it("does not invent a final rollback anchor for a legacy snapshot", () => {
    window.localStorage.setItem(
      "vigla.missionTrustSnapshots.v1",
      JSON.stringify({
        order: ["legacy"],
        byId: {
          legacy: {
            missionId: "legacy",
            title: "Legacy",
            lifecycle: "merged",
            preMergeTag: "vigla/pre-merge/legacy/0",
          },
        },
      }),
    );

    const snapshot = loadMissionTrustSnapshot("legacy");
    expect(snapshot?.targetRef).toBe("");
    expect(snapshot?.rollbackAnchor).toBe("");
  });
});
