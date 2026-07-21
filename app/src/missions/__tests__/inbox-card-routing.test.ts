// @vitest-environment jsdom
// P1-7 — regression coverage for the inbox card title produced by
// `describeArbiterDecided` when the supervisor escalates before the
// worker registration event lands (unknown / missing / empty
// `worker_id`) and when the upstream `audit_overall` is malformed.
// Also pins the buildCard contract: when the visibility verdict
// classifies an arbiter.decided as inbox_kind=escalation, the
// resulting `card.kind` must be `"escalation"` (not "completion").
//
// Drives the public surface (`applyMissionEvent` + the inbox
// appender side-channel) for the buildCard integration test, and
// calls the freshly-exported `describeArbiterDecided` directly for
// the per-branch title / detail assertions.

import { beforeEach, describe, expect, it, vi } from "vitest";

vi.mock("../../bindings", () => ({
  commands: {
    missionEventVisibility: vi.fn(),
    surfaceInboxNotification: vi.fn(),
  },
}));

import { commands } from "../../bindings";
import {
  _setBannerEmitter,
  _setInboxAppender,
  applyMissionEvent,
  describeArbiterDecided,
} from "../ingest";
import { emptyMissionsState } from "../types";
import type { MissionEvent } from "../../bindings";
import type { InboxCard } from "../../inbox/types";
import type { ActiveMission, MissionsState, MissionWorker } from "../types";
import { _resetVisibilityCache } from "../../inbox/visibility-client";

const MID = "demo-arbiter-routing";

function ts(seq: number): string {
  return `2026-05-26T00:00:00.${String(seq).padStart(3, "0")}Z`;
}

function ev(seq: number, type: MissionEvent["type"], payload: unknown): MissionEvent {
  return { mission_id: MID, seq, ts: ts(seq), type, payload } as MissionEvent;
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

function freshActive(workers: Record<string, MissionWorker> = {}): ActiveMission {
  // Build an ActiveMission shell suitable for direct calls to
  // describeArbiterDecided. We only care about the `workers` map
  // for these tests; every other field is filled with the same
  // defaults the reducer hands out on `mission.created`.
  return {
    id: MID,
    spec: {
      title: "T",
      objective: "O",
      target_ref: "main",
      tests: null,
      supervisor_model: null,
      worker_model: null,
      worker_count: null,
      confirm_plan: null,
    } as ActiveMission["spec"],
    lifecycle: "executing",
    startedAt: ts(0),
    updatedAt: ts(0),
    statusLine: "",
    progressPercent: 0,
    tasks: [],
    workers,
    testsPassed: null,
    completionSummary: null,
    filesChanged: 0,
    resolution: null,
    abortReason: null,
    attention: [],
    planGeneration: 0,
    planOverview: null,
    planTechStack: null,
    planEnvelopeFit: null,
    supervisorActivity: null,
    lastExtensionDirective: null,
    lastExtensionAt: null,
    audit: null,
    auditPayloadJson: null,
    verdict: null,
    inbox: [],
  };
}

function makeWorker(id: string, taskTitle: string): MissionWorker {
  return {
    id,
    taskIndex: 0,
    taskTitle,
    status: "spawned",
    latestProgress: null,
    submittedFiles: [],
  };
}

// `audit_overall` is intentionally typed `unknown` so callers can pass
// `Number.NaN`, `null`, the sentinel `OMIT` (drop the key entirely), or
// a real number — the function under test must defend against
// malformed wire payloads.
const OMIT = Symbol("omit");
function arbiterDecided(
  seq: number,
  worker_id: string | undefined,
  bound: "scope" | "reversibility" | "risk" | "quality" | null,
  audit_overall: unknown = 0.84,
  decision_json = '{"kind":"escalate"}',
): MissionEvent {
  const payload: Record<string, unknown> = {
    decision_json,
    bound,
  };
  if (audit_overall !== OMIT) payload.audit_overall = audit_overall;
  if (worker_id !== undefined) payload.worker_id = worker_id;
  return ev(seq, "arbiter.decided", payload);
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

describe("describeArbiterDecided — worker title fallbacks (P1-7)", () => {
  it("known worker_id keeps the original taskTitle in the escalation title", () => {
    const active = freshActive({ "wkr-1": makeWorker("wkr-1", "Implement") });
    const out = describeArbiterDecided(
      arbiterDecided(1, "wkr-1", "scope") as Extract<
        MissionEvent,
        { type: "arbiter.decided" }
      >,
      active,
    );
    expect(out.title).toBe('Escalation: scope bound on "Implement"');
    expect(out.detail).toBe("Audit 0.84; see decision payload");
    expect(out.bound).toBe("scope");
  });

  it("unknown worker_id falls back to a short worker-id label", () => {
    const active = freshActive();
    const out = describeArbiterDecided(
      arbiterDecided(2, "abc12345xyz98765", "reversibility") as Extract<
        MissionEvent,
        { type: "arbiter.decided" }
      >,
      active,
    );
    // last-8 of "abc12345xyz98765" → "xyz98765"
    expect(out.title).toContain("worker xyz98765");
    expect(out.title).not.toContain("undefined");
    expect(out.bound).toBe("reversibility");
  });

  it("empty worker_id falls back to the mission-level framing", () => {
    const active = freshActive();
    const out = describeArbiterDecided(
      arbiterDecided(3, "", "scope") as Extract<
        MissionEvent,
        { type: "arbiter.decided" }
      >,
      active,
    );
    expect(out.title).toBe('Escalation: scope bound on "this mission"');
    expect(out.title).not.toContain("undefined");
  });

  it("missing worker_id field uses the same mission-level fallback", () => {
    const active = freshActive();
    const out = describeArbiterDecided(
      arbiterDecided(4, undefined, "scope") as Extract<
        MissionEvent,
        { type: "arbiter.decided" }
      >,
      active,
    );
    expect(out.title).toBe('Escalation: scope bound on "this mission"');
    expect(out.title).not.toContain("undefined");
  });

  it("non-numeric audit_overall (NaN) renders detail as 'see decision payload'", () => {
    const active = freshActive({ "wkr-1": makeWorker("wkr-1", "Implement") });
    const out = describeArbiterDecided(
      arbiterDecided(5, "wkr-1", "risk", Number.NaN) as Extract<
        MissionEvent,
        { type: "arbiter.decided" }
      >,
      active,
    );
    expect(out.detail).toBe("see decision payload");
    expect(out.detail).not.toContain("NaN");
  });

  it("missing audit_overall (key omitted) renders detail as 'see decision payload'", () => {
    const active = freshActive({ "wkr-1": makeWorker("wkr-1", "Implement") });
    const out = describeArbiterDecided(
      arbiterDecided(6, "wkr-1", "quality", OMIT) as Extract<
        MissionEvent,
        { type: "arbiter.decided" }
      >,
      active,
    );
    expect(out.detail).toBe("see decision payload");
    expect(out.detail).not.toContain("undefined");
    expect(out.detail).not.toContain("NaN");
  });

  it("never produces the literal 'undefined' substring across the bound/worker matrix", () => {
    // Parametrize over every AuthorityBound (including null for the
    // accept/scrub branches) and a known + unknown worker, asserting
    // no branch's title or detail leaks the substring "undefined".
    const bounds: Array<"scope" | "reversibility" | "risk" | "quality" | null> = [
      "scope",
      "reversibility",
      "risk",
      "quality",
      null,
    ];
    const workerCases: Array<{ id: string | undefined; map: Record<string, MissionWorker> }> = [
      { id: "wkr-1", map: { "wkr-1": makeWorker("wkr-1", "Implement") } },
      { id: "phantom-id-xyz98765", map: {} },
      { id: "", map: {} },
      { id: undefined, map: {} },
    ];
    for (const bound of bounds) {
      for (const wc of workerCases) {
        const out = describeArbiterDecided(
          arbiterDecided(7, wc.id, bound) as Extract<
            MissionEvent,
            { type: "arbiter.decided" }
          >,
          freshActive(wc.map),
        );
        expect(out.title.includes("undefined"), `title for ${bound}/${wc.id}`).toBe(
          false,
        );
        expect(
          out.detail?.includes("undefined") === true,
          `detail for ${bound}/${wc.id}`,
        ).toBe(false);
      }
    }
  });
});

describe("buildCard — visibility verdict pins inbox_kind onto card.kind", () => {
  it("arbiter.decided with verdict.inbox_kind=escalation writes card.kind='escalation'", async () => {
    // Drive through the public `applyMissionEvent` surface so the
    // integration is honest: the visibility verdict mock returns
    // `inbox_kind:"escalation"` and the resulting card MUST carry
    // `kind:"escalation"` — regressions where buildCard collapses
    // to "completion" (the bug the user observed) would surface here.
    (commands.missionEventVisibility as ReturnType<typeof vi.fn>).mockResolvedValue({
      kind: "inbox",
      inbox_kind: "escalation",
      severity: "action_required",
    });

    let state: MissionsState = emptyMissionsState();
    const ref: { current: MissionsState } = { current: state };
    _setInboxAppender((missionId: string, card: InboxCard) => {
      const active = ref.current.active;
      if (!active || active.id !== missionId) return;
      const merged = [...active.inbox.filter((c) => c.id !== card.id), card];
      ref.current = {
        ...ref.current,
        active: { ...active, inbox: merged },
      };
    });

    state = applyMissionEvent(state, createdEvent);
    ref.current = state;
    state = applyMissionEvent(
      state,
      arbiterDecided(10, "wkr-known", "scope", 0.42),
    );
    ref.current = state;

    // Flush the fire-and-forget visibility promise chain.
    await Promise.resolve();
    await Promise.resolve();
    await Promise.resolve();

    const card = ref.current.active?.inbox.find((c) => c.seq === 10);
    expect(card).toBeDefined();
    expect(card!.kind).toBe("escalation");
    expect(card!.severity).toBe("action_required");
    expect(card!.bound).toBe("scope");
    // And confirm the safety net + per-branch fix held: no literal
    // "undefined" even though wkr-known was never registered.
    expect(card!.title.includes("undefined")).toBe(false);
  });
});
