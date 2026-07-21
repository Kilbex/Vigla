import { describe, expect, it } from "vitest";
import {
  applyInboxAction,
  selectInbox,
} from "../inbox/InboxState";
import { emptyInboxState, type InboxCard } from "../inbox/types";

function card(overrides: Partial<InboxCard> = {}): InboxCard {
  return {
    id: "mid:1:w-1",
    missionId: "mid",
    seq: 1,
    surfacedAt: "2026-05-21T00:00:00.000Z",
    kind: "escalation",
    severity: "action_required",
    title: "Quality bound tripped",
    detail: "audit overall 0.4 < 0.7 floor",
    bound: "quality",
    resolved: false,
    ...overrides,
  };
}

describe("applyInboxAction — upsert", () => {
  it("appends a new card", () => {
    const after = applyInboxAction(emptyInboxState(), {
      type: "upsert",
      card: card(),
    });
    expect(after.cards).toHaveLength(1);
    expect(after.cards[0].id).toBe("mid:1:w-1");
  });

  it("replaces an existing card by id (latest wins)", () => {
    const s1 = applyInboxAction(emptyInboxState(), {
      type: "upsert",
      card: card({ title: "first" }),
    });
    const s2 = applyInboxAction(s1, {
      type: "upsert",
      card: card({ title: "second", seq: 2 }),
    });
    expect(s2.cards).toHaveLength(1);
    expect(s2.cards[0].title).toBe("second");
    expect(s2.cards[0].seq).toBe(2);
  });

  it("keeps cards in stable order by seq ascending", () => {
    let s = emptyInboxState();
    s = applyInboxAction(s, { type: "upsert", card: card({ id: "a", seq: 3 }) });
    s = applyInboxAction(s, { type: "upsert", card: card({ id: "b", seq: 1 }) });
    s = applyInboxAction(s, { type: "upsert", card: card({ id: "c", seq: 2 }) });
    expect(s.cards.map((c) => c.id)).toEqual(["b", "c", "a"]);
  });
});

describe("applyInboxAction — resolve", () => {
  it("marks the card resolved without removing it", () => {
    const s1 = applyInboxAction(emptyInboxState(), {
      type: "upsert",
      card: card(),
    });
    const s2 = applyInboxAction(s1, { type: "resolve", id: "mid:1:w-1" });
    expect(s2.cards).toHaveLength(1);
    expect(s2.cards[0].resolved).toBe(true);
  });

  it("is a no-op for unknown id", () => {
    const s1 = applyInboxAction(emptyInboxState(), {
      type: "upsert",
      card: card(),
    });
    const s2 = applyInboxAction(s1, { type: "resolve", id: "ghost" });
    expect(s2.cards).toEqual(s1.cards);
  });
});

describe("applyInboxAction — clear_for_mission", () => {
  it("removes only cards for the given mission", () => {
    let s = emptyInboxState();
    s = applyInboxAction(s, {
      type: "upsert",
      card: card({ id: "m1:1", missionId: "m1" }),
    });
    s = applyInboxAction(s, {
      type: "upsert",
      card: card({ id: "m2:1", missionId: "m2", seq: 1 }),
    });
    const after = applyInboxAction(s, {
      type: "clear_for_mission",
      missionId: "m1",
    });
    expect(after.cards.map((c) => c.id)).toEqual(["m2:1"]);
  });
});

describe("selectInbox", () => {
  it("returns the state slice unchanged", () => {
    const s = applyInboxAction(emptyInboxState(), {
      type: "upsert",
      card: card(),
    });
    expect(selectInbox(s)).toBe(s);
  });
});

describe("immutability", () => {
  it("never mutates the input state", () => {
    const s = applyInboxAction(emptyInboxState(), {
      type: "upsert",
      card: card(),
    });
    const before = JSON.stringify(s);
    applyInboxAction(s, { type: "upsert", card: card({ id: "other" }) });
    expect(JSON.stringify(s)).toBe(before);
  });
});
