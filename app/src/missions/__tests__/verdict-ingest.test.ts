// S10 — verifies that the `mission.completion_verdict_rendered`
// wire event lands on the active mission slice as a deserialised
// CompletionVerdict.

import { describe, expect, it } from "vitest";
import { applyMissionEvent } from "../ingest";
import { emptyMissionsState } from "../types";
import type { MissionEvent } from "../../bindings";

const MID = "demo-verdict";

const SPEC = {
  title: "t",
  objective: "o",
  target_ref: "main",
  tests: null,
  supervisor_model: null,
  worker_model: null,
  worker_count: null,
  confirm_plan: null,
  scope_paths: [],
};

function created(seq: number): MissionEvent {
  return {
    mission_id: MID,
    seq,
    ts: `2026-05-31T00:00:0${seq}.000Z`,
    type: "mission.created",
    payload: { spec: SPEC },
  } as unknown as MissionEvent;
}

function verdictEvent(seq: number, payload_json: string): MissionEvent {
  return {
    mission_id: MID,
    seq,
    ts: `2026-05-31T00:00:0${seq}.000Z`,
    type: "mission.completion_verdict_rendered",
    payload: { payload_json },
  } as unknown as MissionEvent;
}

describe("completion verdict ingest", () => {
  it("starts with verdict=null on mission.created", () => {
    const state = applyMissionEvent(emptyMissionsState(), created(0));
    expect(state.active?.verdict).toBeNull();
  });

  it("populates ActiveMission.verdict on mission.completion_verdict_rendered", () => {
    const verdict = {
      all_subtasks_accepted: true,
      integrated_test_pass: {
        ran: true,
        passed: 12,
        failed: 0,
        skipped: 0,
        score: 1.0,
      },
      residual_risk: "low",
      doc_coverage: 0.92,
      unresolved_issues: [],
      recommendation: { kind: "accept", audit: {}, summary: "ok" },
    };
    let state = applyMissionEvent(emptyMissionsState(), created(0));
    state = applyMissionEvent(state, verdictEvent(1, JSON.stringify(verdict)));
    expect(state.active?.verdict).not.toBeNull();
    expect(state.active?.verdict?.residual_risk).toBe("low");
    expect(state.active?.verdict?.all_subtasks_accepted).toBe(true);
  });

  it("leaves verdict=null on malformed payload_json (defensive)", () => {
    let state = applyMissionEvent(emptyMissionsState(), created(0));
    state = applyMissionEvent(state, verdictEvent(1, "not json"));
    expect(state.active?.verdict).toBeNull();
  });
});
