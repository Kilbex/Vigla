import { describe, it, expect, vi, beforeEach } from "vitest";
import { render } from "@testing-library/react";
import type { Event } from "../bindings";

// xterm.js's Terminal cannot construct cleanly under jsdom (it tries
// to read DPR / canvas state via `term.open()`). For audit-r5 polish
// we only care about whether `clear()` is called when workerId
// changes and whether `writeln()` runs after the reset — so stub the
// xterm classes with bare doubles that record method calls.
const clearCalls: number[] = [];
const writelnCalls: string[] = [];

vi.mock("@xterm/xterm", () => {
  return {
    Terminal: class {
      loadAddon() {}
      open() {}
      writeln(line: string) {
        writelnCalls.push(line);
      }
      clear() {
        clearCalls.push(clearCalls.length);
      }
      dispose() {}
    },
  };
});

vi.mock("@xterm/addon-fit", () => ({
  FitAddon: class {
    fit() {}
  },
}));
vi.mock("@xterm/addon-search", () => ({
  SearchAddon: class {},
}));
vi.mock("@xterm/addon-web-links", () => ({
  WebLinksAddon: class {},
}));
// xterm's CSS import is a no-op in jsdom but the loader must resolve.
vi.mock("@xterm/xterm/css/xterm.css", () => ({}));

import RawTerminal from "../drawer/RawTerminal";

function logEvt(workerId: string, seq: number): Event {
  return {
    schema_version: "1.0",
    worker_id: workerId,
    task_id: null,
    seq,
    ts: "2026-05-10T00:00:00.000Z",
    type: "log",
    payload: { level: "info", stream: "stdout", line: `s${seq}` },
  } as Event;
}

beforeEach(() => {
  clearCalls.length = 0;
  writelnCalls.length = 0;
});

describe("RawTerminal — worker switch reset (audit r5 polish)", () => {
  it("clears the terminal and resets the seq cursor when workerId changes", () => {
    const eventsA = [logEvt("A", 0), logEvt("A", 1), logEvt("A", 2)];
    const { rerender } = render(
      <RawTerminal events={eventsA} workerId="A" />,
    );
    // Worker A's three events were written. (Mount also calls clear()
    // once via the workerId-keyed effect — that's expected.)
    expect(writelnCalls.length).toBeGreaterThanOrEqual(3);
    const clearOnMount = clearCalls.length;
    const writelnAfterA = writelnCalls.length;

    // Switch to worker B with seqs lower than A's last seq (2).
    const eventsB = [logEvt("B", 0), logEvt("B", 1)];
    rerender(<RawTerminal events={eventsB} workerId="B" />);

    // clear() fires once more when workerId changes.
    expect(clearCalls.length).toBe(clearOnMount + 1);
    // Worker B's events render even though their seqs (0, 1) are
    // BELOW worker A's last seq (2). Without the fix, lastSeqRef
    // would still be 2 and eventsAfterSeq would filter B's seqs out.
    expect(writelnCalls.length).toBeGreaterThan(writelnAfterA);
  });

  it("does NOT clear the terminal when only the events array changes (same worker)", () => {
    const events = [logEvt("A", 0), logEvt("A", 1)];
    const { rerender } = render(
      <RawTerminal events={events} workerId="A" />,
    );
    const clearAfterMount = clearCalls.length;

    // More events for the same worker — the seq cursor must NOT reset.
    const moreEvents = [...events, logEvt("A", 2), logEvt("A", 3)];
    rerender(<RawTerminal events={moreEvents} workerId="A" />);

    expect(clearCalls.length).toBe(clearAfterMount);
    // The new events were appended, not re-written from scratch.
    // Total writelns = events.length on initial render (worker A
    // seq 0,1) + 2 new events on rerender (seq 2, 3) = 4.
    expect(writelnCalls.length).toBe(4);
  });
});
