// T5 — direct tests for describeArbiterDecided + buildCard's
// arbiter.decided branch. Pins the three decision-kind titles and
// the bound-present escalation branch, plus a malformed JSON
// regression.
//
// The substring match `decision_json.includes('"kind":"scrub"')`
// at ingest.ts:669 is fragile (a future rename to e.g. `decision_kind`
// would silently turn every scrub into an accept). These tests
// catch that class of regression.

import { beforeEach, describe, expect, it, vi } from "vitest";

vi.mock("../bindings", () => ({
  commands: {
    missionEventVisibility: vi.fn(),
    surfaceInboxNotification: vi.fn(),
  },
}));

import { commands } from "../bindings";
import { _setBannerEmitter, _setInboxAppender, applyMissionEvent } from "../missions/ingest";
import { emptyMissionsState } from "../missions/types";
import type { MissionEvent } from "../bindings";
import type { InboxCard } from "../inbox/types";
import type { MissionsState } from "../missions/types";
import { _resetVisibilityCache } from "../inbox/visibility-client";

const MID = "demo-arbiter";

function ts(seq: number): string {
  return `2026-05-21T00:00:00.${String(seq).padStart(3, "0")}Z`;
}

function ev(seq: number, type: MissionEvent["type"], payload: unknown): MissionEvent {
  return { mission_id: MID, seq, ts: ts(seq), type, payload } as MissionEvent;
}

function makeAppender(
  ref: { current: MissionsState },
): (missionId: string, card: InboxCard) => void {
  return (missionId: string, card: InboxCard) => {
    const active = ref.current.active;
    if (!active || active.id !== missionId) return;
    const without = active.inbox.filter((c) => c.id !== card.id);
    const merged = [...without, card].sort((a, b) => a.seq - b.seq);
    ref.current = {
      ...ref.current,
      active: { ...active, inbox: merged },
    };
  };
}

const createdEvent: MissionEvent = ev(0, "mission.created", {
  spec: {
    title: "T",
    objective: "O",
    target_ref: "main",
    tests: null,
    supervisor_model: null,
    worker_model: null,
    worker_count: null,
    confirm_plan: null,
    scope_paths: [],
  },
});

async function runArbiterDecided(
  decisionJson: string,
  bound: "scope" | "reversibility" | "risk" | "quality" | null,
  visibility: { inbox_kind: InboxCard["kind"]; severity: InboxCard["severity"] },
): Promise<InboxCard | undefined> {
  (commands.missionEventVisibility as ReturnType<typeof vi.fn>).mockImplementation(
    async (kind: { type: string }) => {
      if (kind.type === "arbiter.decided") {
        return { kind: "inbox", ...visibility };
      }
      return { kind: "internal" };
    },
  );

  let state = emptyMissionsState();
  const ref: { current: MissionsState } = { current: state };
  _setInboxAppender(makeAppender(ref));

  state = applyMissionEvent(state, createdEvent);
  ref.current = state;

  const arbiter: MissionEvent = ev(10, "arbiter.decided", {
    worker_id: "mock-1",
    decision_json: decisionJson,
    audit_overall: 0.42,
    bound,
  });
  state = applyMissionEvent(state, arbiter);
  ref.current = state;

  // Allow async visibility lookup + dispatch to flush.
  await Promise.resolve();
  await Promise.resolve();
  await Promise.resolve();

  return ref.current.active?.inbox.find((c) => c.seq === 10);
}

beforeEach(() => {
  _resetVisibilityCache();
  vi.clearAllMocks();
  (commands.missionEventVisibility as ReturnType<typeof vi.fn>).mockResolvedValue({
    kind: "internal",
  });
  _setInboxAppender(null);
  _setBannerEmitter(null);
});

describe("describeArbiterDecided", () => {
  it("escalation with bound produces Escalation card with bound kind in title", async () => {
    const card = await runArbiterDecided(
      '{"kind":"escalate","bound":"quality"}',
      "quality",
      { inbox_kind: "escalation", severity: "action_required" },
    );
    expect(card).toBeDefined();
    expect(card!.title).toContain("Escalation");
    expect(card!.title.toLowerCase()).toContain("quality");
    expect(card!.severity).toBe("action_required");
    expect(card!.bound).toBe("quality");
  });

  it("scrub decision produces a Scrub-titled card", async () => {
    const card = await runArbiterDecided(
      '{"kind":"scrub","reason":"out_of_scope","retained_artifacts":[]}',
      null,
      { inbox_kind: "escalation", severity: "warning" },
    );
    expect(card).toBeDefined();
    expect(card!.title).toContain("Scrub");
    expect(card!.bound).toBeNull();
  });

  it("accept decision produces an Accepted-titled card", async () => {
    const card = await runArbiterDecided(
      '{"kind":"accept"}',
      null,
      { inbox_kind: "completion", severity: "info" },
    );
    expect(card).toBeDefined();
    expect(card!.title).toContain("Accepted");
    expect(card!.bound).toBeNull();
  });

  it("falls back to Accepted-style card when decision_json is malformed", async () => {
    // The current implementation does a string-includes check for
    // `"kind":"scrub"`; any unparseable / unrecognised payload
    // falls through to the default Accept branch without
    // throwing. This pins that defensive behaviour.
    const card = await runArbiterDecided(
      "not json at all",
      null,
      { inbox_kind: "completion", severity: "info" },
    );
    expect(card).toBeDefined();
    expect(card!.title).toContain("Accepted");
  });

  it("scope bound surfaces 'scope' in escalation title", async () => {
    const card = await runArbiterDecided(
      '{"kind":"escalate","bound":"scope"}',
      "scope",
      { inbox_kind: "escalation", severity: "action_required" },
    );
    expect(card).toBeDefined();
    expect(card!.title.toLowerCase()).toContain("scope");
    expect(card!.bound).toBe("scope");
  });
});
