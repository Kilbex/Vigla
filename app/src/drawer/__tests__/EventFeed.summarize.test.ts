import { describe, it, expect } from "vitest";
import { summarize } from "../EventFeed";
import type { Event } from "../../bindings";

const envelope = {
  schema_version: "1.0",
  worker_id: "w-1",
  task_id: null,
  seq: 1,
  ts: "2026-01-01T00:00:00Z",
};

function makeEvent<T extends Event["type"], P>(type: T, payload: P): Event {
  return { ...envelope, type, payload } as unknown as Event;
}

describe("summarize · file_activity", () => {
  it("happy path renders op, path, and +adds/-removes", () => {
    const e = makeEvent("file_activity", {
      op: "modified",
      path: "src/foo.ts",
      lines_added: 12,
      lines_removed: 3,
    });
    expect(summarize(e)).toBe("modified src/foo.ts (+12/-3)");
  });

  it("missing op falls back to 'edit' and never prints 'undefined'", () => {
    const e = makeEvent("file_activity", {
      op: undefined,
      path: "src/foo.ts",
      lines_added: 12,
      lines_removed: 3,
    });
    const out = summarize(e);
    expect(out).toBe("edit src/foo.ts (+12/-3)");
    expect(out).not.toContain("undefined");
  });

  it("missing path falls back to '(unknown file)' and never prints 'undefined'", () => {
    const e = makeEvent("file_activity", {
      op: "modified",
      path: undefined,
      lines_added: 12,
      lines_removed: 3,
    });
    const out = summarize(e);
    expect(out).toBe("modified (unknown file) (+12/-3)");
    expect(out).not.toContain("undefined");
  });

  it("missing op, path, and line counts all fall back cleanly", () => {
    const e = makeEvent("file_activity", {
      op: undefined,
      path: undefined,
      lines_added: undefined,
      lines_removed: undefined,
    });
    const out = summarize(e);
    expect(out).toBe("edit (unknown file) (+0/-0)");
    expect(out).not.toContain("undefined");
  });
});

describe("summarize · cost", () => {
  it("happy path: 3 decimals, no '+' prefix, thousand-separators on tokens", () => {
    const e = makeEvent("cost", {
      usd: 0.241,
      input_tokens: 12450,
      output_tokens: 3200,
    });
    const out = summarize(e);
    expect(out).toContain("$0.241");
    expect(out).toContain("12,450 in");
    expect(out).toContain("3,200 out");
    expect(out.startsWith("+")).toBe(false);
  });

  it("tiny value uses 4 decimals", () => {
    const e = makeEvent("cost", {
      usd: 0.0023,
      input_tokens: 0,
      output_tokens: 0,
    });
    expect(summarize(e)).toContain("$0.0023");
  });

  it("null guards: undefined usd and token counts do not throw", () => {
    const e = makeEvent("cost", {
      usd: undefined,
      input_tokens: undefined,
      output_tokens: undefined,
    });
    const out = summarize(e);
    expect(out).toContain("$0.000");
    expect(out).toContain("0 in / 0 out");
  });
});
