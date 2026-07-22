// @vitest-environment jsdom
import { beforeEach, describe, expect, it, vi } from "vitest";
import type { EventVisibility, MissionEvent } from "../bindings";

const { missionEventVisibility } = vi.hoisted(() => ({
  missionEventVisibility: vi.fn<(...args: unknown[]) => Promise<EventVisibility>>(),
}));

vi.mock("../bindings", async () => {
  const actual = await vi.importActual<Record<string, unknown>>("../bindings");
  return {
    ...actual,
    commands: { missionEventVisibility },
  };
});

import { _resetVisibilityCache, fetchVisibility } from "../inbox/visibility-client";

function arbiterDecidedEvent(
  bound: string | null,
  decisionKind: "accept" | "extend" | "scrub" | "escalate",
  seq: number,
  reworkKind?: string,
): MissionEvent {
  return {
    mission_id: "mid",
    seq,
    ts: `2026-05-21T00:00:0${seq}.000Z`,
    type: "arbiter.decided",
    payload: {
      worker_id: "mock-1",
      decision_json: JSON.stringify({
        kind: decisionKind,
        ...(reworkKind ? { rework_kind: { kind: reworkKind } } : {}),
      }),
      audit_overall: decisionKind === "accept" ? 0.85 : 0.4,
      bound,
    },
  } as unknown as MissionEvent;
}

const acceptVerdict: EventVisibility = {
  kind: "inbox",
  inbox_kind: "completion",
  severity: "info",
} as unknown as EventVisibility;

const scrubVerdict: EventVisibility = {
  kind: "inbox",
  inbox_kind: "escalation",
  severity: "warning",
} as unknown as EventVisibility;

const extendVerdict: EventVisibility = { kind: "internal" } as unknown as EventVisibility;

const escalateVerdict: EventVisibility = {
  kind: "inbox",
  inbox_kind: "escalation",
  severity: "action_required",
} as unknown as EventVisibility;

describe("fetchVisibility cache key — arbiter.decided", () => {
  beforeEach(() => {
    missionEventVisibility.mockReset();
    _resetVisibilityCache();
  });

  it("does not collapse Accept and Scrub under the same cache key", async () => {
    // First: Accept resolves; cache stores Accept's verdict.
    missionEventVisibility.mockResolvedValueOnce(acceptVerdict);
    const v1 = await fetchVisibility(arbiterDecidedEvent(null, "accept", 1));
    expect(v1).toEqual(acceptVerdict);

    // Second: Scrub should NOT reuse Accept's cached verdict — the
    // visibility client must round-trip again with a different
    // discriminator.
    missionEventVisibility.mockResolvedValueOnce(scrubVerdict);
    const v2 = await fetchVisibility(arbiterDecidedEvent(null, "scrub", 2));
    expect(v2).toEqual(scrubVerdict);

    expect(missionEventVisibility).toHaveBeenCalledTimes(2);
  });

  it("does not collapse Accept and Extend under the same cache key", async () => {
    missionEventVisibility.mockResolvedValueOnce(acceptVerdict);
    await fetchVisibility(arbiterDecidedEvent(null, "accept", 1));

    missionEventVisibility.mockResolvedValueOnce(extendVerdict);
    const v2 = await fetchVisibility(arbiterDecidedEvent(null, "extend", 2));
    expect(v2).toEqual(extendVerdict);

    expect(missionEventVisibility).toHaveBeenCalledTimes(2);
  });

  it("distinguishes Escalate by bound (Scope vs Quality)", async () => {
    missionEventVisibility.mockResolvedValueOnce(escalateVerdict);
    await fetchVisibility(arbiterDecidedEvent("scope", "escalate", 1));

    missionEventVisibility.mockResolvedValueOnce(escalateVerdict);
    await fetchVisibility(arbiterDecidedEvent("quality", "escalate", 2));

    expect(missionEventVisibility).toHaveBeenCalledTimes(2);
  });

  it("does not collapse ordinary Extend and terminal MarkUnachievable", async () => {
    missionEventVisibility.mockResolvedValueOnce(extendVerdict);
    expect(
      await fetchVisibility(
        arbiterDecidedEvent(null, "extend", 1, "revise"),
      ),
    ).toEqual(extendVerdict);

    missionEventVisibility.mockResolvedValueOnce(scrubVerdict);
    expect(
      await fetchVisibility(
        arbiterDecidedEvent(null, "extend", 2, "mark_unachievable"),
      ),
    ).toEqual(scrubVerdict);

    expect(missionEventVisibility).toHaveBeenCalledTimes(2);
  });

  it("reuses cached verdict for a repeat of the same (bound, decision) tuple", async () => {
    missionEventVisibility.mockResolvedValueOnce(acceptVerdict);
    const v1 = await fetchVisibility(arbiterDecidedEvent(null, "accept", 1));
    const v2 = await fetchVisibility(arbiterDecidedEvent(null, "accept", 2));
    expect(v1).toEqual(acceptVerdict);
    expect(v2).toEqual(acceptVerdict);
    expect(missionEventVisibility).toHaveBeenCalledTimes(1);
  });

  it("coalesces concurrent requests for the same visibility key", async () => {
    let resolveVisibility!: (value: EventVisibility) => void;
    missionEventVisibility.mockImplementationOnce(
      () =>
        new Promise((resolve) => {
          resolveVisibility = resolve;
        }),
    );

    const first = fetchVisibility(arbiterDecidedEvent(null, "accept", 1));
    const second = fetchVisibility(arbiterDecidedEvent(null, "accept", 2));

    expect(missionEventVisibility).toHaveBeenCalledTimes(1);
    resolveVisibility(acceptVerdict);
    await expect(Promise.all([first, second])).resolves.toEqual([
      acceptVerdict,
      acceptVerdict,
    ]);
  });
});

function completionVerdictEvent(
  recommendationKind: "accept" | "extend" | "scrub",
  seq: number,
): MissionEvent {
  return {
    mission_id: "mid",
    seq,
    ts: `2026-05-21T00:00:0${seq}.000Z`,
    type: "mission.completion_verdict_rendered",
    payload: {
      payload_json: JSON.stringify({
        recommendation: { kind: recommendationKind },
      }),
    },
  } as unknown as MissionEvent;
}

describe("fetchVisibility cache key — mission.completion_verdict_rendered", () => {
  beforeEach(() => {
    missionEventVisibility.mockReset();
    _resetVisibilityCache();
  });

  it("does not collapse Accept and Scrub under the same cache key", async () => {
    missionEventVisibility.mockResolvedValueOnce(acceptVerdict);
    expect(await fetchVisibility(completionVerdictEvent("accept", 1))).toEqual(acceptVerdict);

    missionEventVisibility.mockResolvedValueOnce(scrubVerdict);
    expect(await fetchVisibility(completionVerdictEvent("scrub", 2))).toEqual(scrubVerdict);
    expect(missionEventVisibility).toHaveBeenCalledTimes(2);
  });

  it("does not collapse Accept and Extend under the same cache key", async () => {
    missionEventVisibility.mockResolvedValueOnce(acceptVerdict);
    expect(await fetchVisibility(completionVerdictEvent("accept", 1))).toEqual(acceptVerdict);

    missionEventVisibility.mockResolvedValueOnce(escalateVerdict);
    expect(await fetchVisibility(completionVerdictEvent("extend", 2))).toEqual(escalateVerdict);
    expect(missionEventVisibility).toHaveBeenCalledTimes(2);
  });

  it("reuses a cached verdict for repeated recommendations of the same kind", async () => {
    missionEventVisibility.mockResolvedValueOnce(scrubVerdict);
    expect(await fetchVisibility(completionVerdictEvent("scrub", 1))).toEqual(scrubVerdict);
    expect(await fetchVisibility(completionVerdictEvent("scrub", 2))).toEqual(scrubVerdict);
    expect(missionEventVisibility).toHaveBeenCalledTimes(1);
  });

  it("preserves payload-derived visibility when policy IPC is degraded", async () => {
    missionEventVisibility.mockRejectedValueOnce(new Error("ipc down"));

    expect(await fetchVisibility(completionVerdictEvent("scrub", 1))).toEqual(scrubVerdict);
  });
});

function recoveryDecidedEvent(
  actionJson: string,
  seq: number,
): MissionEvent {
  return {
    mission_id: "mid",
    seq,
    ts: `2026-05-21T00:01:0${seq}.000Z`,
    type: "supervisor.recovery_decided",
    payload: {
      worker_id: "mock-1",
      class_json: JSON.stringify({ kind: "missing_file" }),
      action_json: actionJson,
    },
  } as unknown as MissionEvent;
}

const internalVerdict: EventVisibility = {
  kind: "internal",
} as unknown as EventVisibility;

const recoveryEscalateVerdict: EventVisibility = {
  kind: "inbox",
  inbox_kind: "escalation",
  severity: "action_required",
} as unknown as EventVisibility;

describe("fetchVisibility cache key — supervisor.recovery_decided", () => {
  beforeEach(() => {
    missionEventVisibility.mockReset();
    _resetVisibilityCache();
  });

  it("does not collapse Retry and Escalate actions under the same cache key", async () => {
    // The Rust RecoveryAction enum is externally tagged, so the wire
    // shape is `{"retry":{...}}` and `{"escalate":{...}}`. Two distinct
    // action kinds must round-trip through the visibility command
    // independently — a shared cache slot would alias them once a
    // future visibility rule surfaces Escalate differently.
    missionEventVisibility.mockResolvedValueOnce(internalVerdict);
    const v1 = await fetchVisibility(
      recoveryDecidedEvent(JSON.stringify({ retry: { attempt: 1, max: 2 } }), 1),
    );
    expect(v1).toEqual(internalVerdict);

    missionEventVisibility.mockResolvedValueOnce(recoveryEscalateVerdict);
    const v2 = await fetchVisibility(
      recoveryDecidedEvent(
        JSON.stringify({
          escalate: { bound: "scope", evidence: { summary: "x", payload_json: null } },
        }),
        2,
      ),
    );
    expect(v2).toEqual(recoveryEscalateVerdict);

    expect(missionEventVisibility).toHaveBeenCalledTimes(2);
  });

  it("reuses cached verdict for a repeat of the same action kind", async () => {
    missionEventVisibility.mockResolvedValueOnce(internalVerdict);
    const v1 = await fetchVisibility(
      recoveryDecidedEvent(JSON.stringify({ retry: { attempt: 1, max: 2 } }), 1),
    );
    // A second Retry — different `attempt` payload but same variant
    // tag — must hit the cache and NOT re-cross the IPC boundary.
    const v2 = await fetchVisibility(
      recoveryDecidedEvent(JSON.stringify({ retry: { attempt: 2, max: 2 } }), 2),
    );
    expect(v1).toEqual(internalVerdict);
    expect(v2).toEqual(internalVerdict);
    expect(missionEventVisibility).toHaveBeenCalledTimes(1);
  });

  it("falls back to a safe default when action_json is malformed", async () => {
    // Malformed JSON must not crash the inbox pipeline — the cache
    // key collapses to a single `unknown` bucket so we still return
    // a real verdict.
    missionEventVisibility.mockResolvedValueOnce(internalVerdict);
    const verdict = await fetchVisibility(recoveryDecidedEvent("not-json", 1));
    expect(verdict).toEqual(internalVerdict);

    // A repeat malformed event hits the cache rather than re-crossing
    // the IPC boundary — verifying the fallback key is stable.
    const verdict2 = await fetchVisibility(recoveryDecidedEvent("also-not-json", 2));
    expect(verdict2).toEqual(internalVerdict);
    expect(missionEventVisibility).toHaveBeenCalledTimes(1);
  });
});

describe("fetchVisibility fallback verdict on IPC failure", () => {
  beforeEach(() => {
    missionEventVisibility.mockReset();
    _resetVisibilityCache();
  });

  it("falls back to a Warning Escalation for arbiter.decided when IPC throws", async () => {
    missionEventVisibility.mockRejectedValueOnce(new Error("ipc down"));
    const verdict = await fetchVisibility(arbiterDecidedEvent("quality", "escalate", 1));
    expect(verdict).toEqual({
      kind: "inbox",
      inbox_kind: "escalation",
      severity: "warning",
    });
  });

  it("does not cache a degraded fallback after IPC recovers", async () => {
    missionEventVisibility
      .mockRejectedValueOnce(new Error("transient IPC outage"))
      .mockResolvedValueOnce(escalateVerdict);
    const event = arbiterDecidedEvent("quality", "escalate", 1);

    expect(await fetchVisibility(event)).toEqual({
      kind: "inbox",
      inbox_kind: "escalation",
      severity: "warning",
    });
    expect(await fetchVisibility(event)).toEqual(escalateVerdict);
    expect(missionEventVisibility).toHaveBeenCalledTimes(2);
  });

  it("keeps a reverted mission visible when policy IPC is degraded", async () => {
    missionEventVisibility.mockRejectedValueOnce(new Error("ipc down"));
    const event = {
      mission_id: "mid",
      seq: 9,
      ts: "2026-05-21T00:00:09.000Z",
      type: "mission.reverted",
      payload: {
        restored_sha: "abc123",
        pre_merge_tag: "vigla/revert/mid/before/main",
      },
    } as unknown as MissionEvent;

    expect(await fetchVisibility(event)).toEqual({
      kind: "inbox",
      inbox_kind: "completion",
      severity: "info",
    });
  });

  it("keeps a quota-paused mission visible when policy IPC is degraded", async () => {
    missionEventVisibility.mockRejectedValueOnce(new Error("ipc down"));
    const event = {
      mission_id: "mid",
      seq: 10,
      ts: "2026-05-21T00:00:10.000Z",
      type: "mission.paused",
      payload: {
        vendor: "claude",
        reset_at: "2026-05-21T01:00:00.000Z",
        reason: "quota exhausted",
      },
    } as unknown as MissionEvent;

    expect(await fetchVisibility(event)).toEqual({
      kind: "inbox",
      inbox_kind: "completion",
      severity: "info",
    });
  });

  it("keeps an invalid decomposition actionable when policy IPC is degraded", async () => {
    missionEventVisibility.mockRejectedValueOnce(new Error("ipc down"));
    const event = {
      mission_id: "mid",
      seq: 11,
      ts: "2026-05-21T00:00:11.000Z",
      type: "supervisor.decomposition_rejected",
      payload: { reason: "empty decomposition" },
    } as unknown as MissionEvent;

    expect(await fetchVisibility(event)).toEqual({
      kind: "inbox",
      inbox_kind: "escalation",
      severity: "action_required",
    });
  });

  it("falls back to Internal for unknown event types when IPC throws", async () => {
    missionEventVisibility.mockRejectedValueOnce(new Error("ipc down"));
    const evt = {
      mission_id: "mid",
      seq: 1,
      ts: "2026-05-21T00:00:00.000Z",
      type: "supervisor.unknown_kind",
      payload: {},
    } as unknown as MissionEvent;
    const verdict = await fetchVisibility(evt);
    expect((verdict as { kind: string }).kind).toBe("internal");
  });

  it("falls back to Internal for plan.rejected when IPC throws (QC-3)", async () => {
    missionEventVisibility.mockRejectedValueOnce(new Error("ipc down"));
    const evt = {
      mission_id: "mid",
      seq: 1,
      ts: "2026-05-21T00:00:00.000Z",
      type: "plan.rejected",
      payload: { generation: 0, reason: "scope too broad" },
    } as unknown as MissionEvent;
    const verdict = await fetchVisibility(evt);
    // The user already saw the reject overlay; no inbox card —
    // the subsequent mission.aborted event will surface the
    // escalation card with the reason embedded.
    expect((verdict as { kind: string }).kind).toBe("internal");
  });
});
