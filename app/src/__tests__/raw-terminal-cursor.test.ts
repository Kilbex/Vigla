import { describe, expect, it } from "vitest";
import type { Event } from "../bindings";
import { eventsAfterSeq } from "../drawer/terminal-cursor";

// Event factory just needs `seq` to differ; the rest is filler.
function evt(seq: number): Event {
  return {
    schema_version: "1.0",
    worker_id: "w1",
    task_id: null,
    seq,
    ts: "2026-01-01T00:00:00.000Z",
    type: "log",
    payload: { level: "info", stream: "stdout", line: `line ${seq}` },
  } as Event;
}

describe("eventsAfterSeq (RawTerminal write cursor)", () => {
  it("returns everything when lastSeen is null (first write)", () => {
    const events = [evt(0), evt(1), evt(2)];
    const got = eventsAfterSeq(events, null);
    expect(got.map((e) => e.seq)).toEqual([0, 1, 2]);
  });

  it("returns only fresh events (lastSeen=1)", () => {
    const events = [evt(0), evt(1), evt(2), evt(3)];
    const got = eventsAfterSeq(events, 1);
    expect(got.map((e) => e.seq)).toEqual([2, 3]);
  });

  it("returns nothing when no new events", () => {
    const events = [evt(0), evt(1)];
    const got = eventsAfterSeq(events, 1);
    expect(got).toEqual([]);
  });

  it("survives the log rotation that broke the index-based cursor (regression for C3)", () => {
    // Simulate the bounded log: store rotates oldest events out at
    // MAX_EVENTS_PER_WORKER. The array length stays at the cap, but
    // its content shifts forward. An index-based cursor frozen at
    // length=cap would write nothing here; a seq-based cursor still
    // writes the new events.
    const cap = 500;
    const before: Event[] = [];
    for (let s = 0; s < cap; s++) before.push(evt(s));
    // Cursor caught up after an earlier render.
    let lastSeen: number | null = before[cap - 1].seq;
    expect(lastSeen).toBe(cap - 1);

    // Two new events arrive; oldest two are dropped (rotation).
    const after = before.slice(2).concat([evt(cap), evt(cap + 1)]);
    expect(after.length).toBe(cap);
    expect(after[0].seq).toBe(2);
    expect(after[after.length - 1].seq).toBe(cap + 1);

    const fresh = eventsAfterSeq(after, lastSeen);
    expect(fresh.map((e) => e.seq)).toEqual([cap, cap + 1]);
    lastSeen = after[after.length - 1].seq;
    expect(lastSeen).toBe(cap + 1);
  });

  it("walks the tail in O(new) on a long log", () => {
    // Sanity: lots of stale events plus one new at the end.
    const events: Event[] = [];
    for (let s = 0; s < 1000; s++) events.push(evt(s));
    const got = eventsAfterSeq(events, 998);
    expect(got.map((e) => e.seq)).toEqual([999]);
  });
});
