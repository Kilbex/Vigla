import { describe, it, expect, expectTypeOf } from "vitest";
import {
  SCHEMA_VERSION,
  type Artifact,
  type Cost,
  type Event,
  type FailureCategory,
  type HealthDto,
  type LogLevel,
  type LogStream,
  type WorkerInfo,
  type WorkerState,
} from "../bindings";

// Step 2 — TS-side smoke test for the generated event-schema bindings.
// Verifies discriminator narrowing on `Event`, parses one canonical
// fixture per event type, and asserts the inferred payload types match
// what the schema promises. TS strict mode catches `any` regressions.

describe("schema constants", () => {
  it("exposes SCHEMA_VERSION as a literal", () => {
    expect(SCHEMA_VERSION).toBe("2.0");
    // Compile-time: SCHEMA_VERSION is exported as `"2.0"` literal, not string
    expectTypeOf(SCHEMA_VERSION).toEqualTypeOf<"2.0">();
  });
});

describe("HealthDto", () => {
  it("matches the host crate's command return shape", () => {
    const sample: HealthDto = { version: "0.0.1", uptime_ms: 42 };
    expectTypeOf(sample.version).toEqualTypeOf<string>();
    expectTypeOf(sample.uptime_ms).toEqualTypeOf<number>();
  });
});

// Fixture envelope for terser test bodies.
const baseEnvelope = {
  schema_version: "1.0",
  worker_id: "0190a7e0-2c3a-7a01-9f00-0000000000a1",
  task_id: "0190a7e0-2c3a-7a01-9f00-0000000000b7",
  seq: 1,
  ts: "2026-05-08T19:43:01.221Z",
} as const;

function parse(obj: unknown): Event {
  // Round-trip through JSON to mirror the wire path.
  return JSON.parse(JSON.stringify(obj)) as Event;
}

describe("Event discriminator narrowing", () => {
  it("state_change", () => {
    const evt = parse({
      ...baseEnvelope,
      type: "state_change",
      payload: { state: "executing", from: "planning", note: "patch 2/4" },
    });
    expect(evt.type).toBe("state_change");
    if (evt.type === "state_change") {
      expectTypeOf(evt.payload.state).toEqualTypeOf<WorkerState>();
      expect(evt.payload.state).toBe("executing");
      expect(evt.payload.from).toBe("planning");
    }
  });

  it("log", () => {
    const evt = parse({
      ...baseEnvelope,
      seq: 2,
      type: "log",
      payload: { level: "info", stream: "stdout", line: "hello", tag: "fs" },
    });
    if (evt.type === "log") {
      expectTypeOf(evt.payload.level).toEqualTypeOf<LogLevel>();
      expectTypeOf(evt.payload.stream).toEqualTypeOf<LogStream>();
      expect(evt.payload.line).toBe("hello");
      expect(evt.payload.tag).toBe("fs");
    } else {
      throw new Error("expected log");
    }
  });

  it("progress", () => {
    const evt = parse({
      ...baseEnvelope,
      seq: 3,
      type: "progress",
      payload: { percent: 62.5, eta_ms: 18000, note: "patching tests" },
    });
    if (evt.type === "progress") {
      expectTypeOf(evt.payload.percent).toEqualTypeOf<number>();
      expect(evt.payload.percent).toBe(62.5);
      expect(evt.payload.eta_ms).toBe(18000);
    }
  });

  it("file_activity", () => {
    const evt = parse({
      ...baseEnvelope,
      seq: 4,
      type: "file_activity",
      payload: {
        path: "src/fetcher.ts",
        op: "modified",
        lines_added: 12,
        lines_removed: 4,
      },
    });
    if (evt.type === "file_activity") {
      expect(evt.payload.path).toBe("src/fetcher.ts");
      expect(evt.payload.op).toBe("modified");
    }
  });

  it("test_result", () => {
    const evt = parse({
      ...baseEnvelope,
      seq: 5,
      type: "test_result",
      payload: {
        suite: "vitest",
        passed: 18,
        failed: 1,
        skipped: 0,
        duration_ms: 1230,
        failures: [
          {
            name: "fetcher > retries on 503",
            message: "expected 3, got 2",
            file: "src/fetcher.test.ts",
            line: 42,
          },
        ],
      },
    });
    if (evt.type === "test_result") {
      expect(evt.payload.failed).toBe(1);
      expect(evt.payload.failures?.[0]?.line).toBe(42);
    }
  });

  it("cost", () => {
    const evt = parse({
      ...baseEnvelope,
      seq: 6,
      type: "cost",
      payload: {
        input_tokens: 4210,
        output_tokens: 980,
        usd: 0.0186,
        cache_read_tokens: 11200,
        model: "claude-opus-4-7",
      },
    });
    if (evt.type === "cost") {
      const c: Cost = evt.payload;
      expect(c.input_tokens).toBe(4210);
      expect(c.usd).toBeCloseTo(0.0186);
    }
  });

  it("dependency", () => {
    const evt = parse({
      ...baseEnvelope,
      seq: 7,
      type: "dependency",
      payload: {
        waiting_on: ["0190a7e0-2c3a-7a01-9f00-0000000000c3"],
        reason: "needs schema migration from codex-1",
      },
    });
    if (evt.type === "dependency") {
      expect(evt.payload.waiting_on).toHaveLength(1);
    }
  });

  it("completion", () => {
    const evt = parse({
      ...baseEnvelope,
      seq: 8,
      type: "completion",
      payload: {
        summary: "done",
        artifacts: [{ kind: "file", ref: "src/x.ts", label: "patched" }],
        duration_ms: 198400,
      },
    });
    if (evt.type === "completion") {
      const a: Artifact | undefined = evt.payload.artifacts?.[0];
      expect(a?.ref).toBe("src/x.ts");
      expect(a?.kind).toBe("file");
    }
  });

  it("failure", () => {
    const evt = parse({
      ...baseEnvelope,
      seq: 9,
      type: "failure",
      payload: {
        error: "exit 1",
        retryable: true,
        suggestion: "review",
        exit_code: 1,
        category: "task_logic",
      },
    });
    if (evt.type === "failure") {
      const cat: FailureCategory | null | undefined = evt.payload.category;
      expect(cat).toBe("task_logic");
      expect(evt.payload.retryable).toBe(true);
    }
  });
});

describe("identity types", () => {
  it("WorkerInfo carries all envelope fields", () => {
    const w: WorkerInfo = {
      id: "wid",
      name: "claude-1",
      vendor: "claude",
      cli_binary: "/usr/local/bin/claude",
      cli_version: "1.4.2",
      cwd: "/tmp",
      model: "claude-opus-4-7",
      spawned_at: "2026-05-08T19:42:13.000Z",
      ended_at: null,
    };
    expect(w.vendor).toBe("claude");
  });
});
