// Row 4 (R6) — verifies that a vendor-quota pause surfaces on the
// active mission instead of being silently dropped. Before this,
// `mission.paused` fell through to the reducer's forward-compat
// no-op, so a rate-limited mission kept ticking as if still running
// (looked hung). The reducer must now set lifecycle=paused and add a
// `mission_paused` attention item carrying the resume timestamp, and
// `mission.resumed` must clear it back to executing.

import { describe, expect, it } from "vitest";
import { applyMissionEvent } from "../ingest";
import { emptyMissionsState } from "../types";
import type { MissionEvent } from "../../bindings";

const MID = "demo-quota";
const RESUME_AT = 1_716_000_000_000;

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
    ts: `2026-06-01T00:00:0${seq}.000Z`,
    type: "mission.created",
    payload: { spec: SPEC },
  } as unknown as MissionEvent;
}

function paused(seq: number, reasonJson: string, resumeAtMs: number): MissionEvent {
  return {
    mission_id: MID,
    seq,
    ts: `2026-06-01T00:00:0${seq}.000Z`,
    type: "mission.paused",
    payload: { reason_json: reasonJson, estimated_resume_at_ms: resumeAtMs },
  } as unknown as MissionEvent;
}

function resumed(seq: number, vendor: string): MissionEvent {
  return {
    mission_id: MID,
    seq,
    ts: `2026-06-01T00:00:0${seq}.000Z`,
    type: "mission.resumed",
    payload: { vendor },
  } as unknown as MissionEvent;
}

const QUOTA_REASON = JSON.stringify({ WaitingForQuota: { vendor: "claude" } });

describe("quota-pause ingest", () => {
  it("sets lifecycle=paused and adds a mission_paused attention item with the resume timestamp", () => {
    let state = applyMissionEvent(emptyMissionsState(), created(0));
    state = applyMissionEvent(state, paused(1, QUOTA_REASON, RESUME_AT));

    expect(state.active?.lifecycle).toBe("paused");
    const item = state.active?.attention.find((a) => a.kind === "mission_paused");
    expect(item).toBeDefined();
    expect(item?.severity).toBe("soft");
    expect(item?.resumeAtMs).toBe(RESUME_AT);
    expect(typeof item?.summary).toBe("string");
    expect(item?.summary.length).toBeGreaterThan(0);
  });

  it("clears the paused attention item and returns to executing on mission.resumed", () => {
    let state = applyMissionEvent(emptyMissionsState(), created(0));
    state = applyMissionEvent(state, paused(1, QUOTA_REASON, RESUME_AT));
    state = applyMissionEvent(state, resumed(2, "claude"));

    expect(state.active?.lifecycle).toBe("executing");
    expect(
      state.active?.attention.find((a) => a.kind === "mission_paused"),
    ).toBeUndefined();
  });

  it("still pauses (does not throw) when reason_json is malformed", () => {
    let state = applyMissionEvent(emptyMissionsState(), created(0));
    state = applyMissionEvent(state, paused(1, "not json", RESUME_AT));

    expect(state.active?.lifecycle).toBe("paused");
    const item = state.active?.attention.find((a) => a.kind === "mission_paused");
    expect(item?.resumeAtMs).toBe(RESUME_AT);
  });
});
