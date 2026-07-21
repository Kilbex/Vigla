import { describe, it, expect } from "vitest";
import type { Event, EventKind } from "../bindings";
import { applyEvent, emptyState } from "../store/ingest";

/// Synthesise a single canonical event.
function evt(workerId: string, seq: number, taskId: string, kind: EventKind): Event {
  return {
    schema_version: "1.0",
    worker_id: workerId,
    task_id: taskId,
    seq,
    ts: `2026-01-01T00:00:${String(seq % 60).padStart(2, "0")}.000Z`,
    ...kind,
  } as Event;
}

/// Build a representative event stream for `numWorkers` workers, each
/// running a claude-happy-shaped trajectory of `eventsPerWorker`
/// events.
function buildStream(numWorkers: number, eventsPerWorker: number): Event[] {
  const out: Event[] = [];
  for (let w = 0; w < numWorkers; w++) {
    const wid = `worker-${w}`;
    const tid = `task-${w}`;
    for (let i = 0; i < eventsPerWorker; i++) {
      const phase = i / eventsPerWorker;
      const kind: EventKind =
        i === 0
          ? { type: "state_change", payload: { state: "idle" } }
          : phase < 0.2
            ? { type: "state_change", payload: { state: "planning" } }
            : phase < 0.6
              ? {
                  type: "progress",
                  payload: { percent: phase * 100, eta_ms: 1000 },
                }
              : phase < 0.7
                ? {
                    type: "file_activity",
                    payload: {
                      path: `src/file-${i}.ts`,
                      op: "modified",
                      lines_added: 5,
                      lines_removed: 1,
                    },
                  }
                : phase < 0.8
                  ? {
                      type: "test_result",
                      payload: {
                        suite: "vitest",
                        passed: 10,
                        failed: 0,
                        skipped: 0,
                        duration_ms: 100,
                      },
                    }
                  : phase < 0.9
                    ? {
                        type: "cost",
                        payload: {
                          input_tokens: 100,
                          output_tokens: 50,
                          usd: 0.01,
                        },
                      }
                    : i === eventsPerWorker - 1
                      ? {
                          type: "completion",
                          payload: { summary: "done" },
                        }
                      : { type: "state_change", payload: { state: "executing" } };
      out.push(evt(wid, i, tid, kind));
    }
  }
  return out;
}

describe("ingest performance", () => {
  it("ingests 5 workers × 16 events in well under 5ms", () => {
    const stream = buildStream(5, 16);
    expect(stream.length).toBe(80);
    const s = emptyState();
    const t0 = performance.now();
    for (const e of stream) {
      applyEvent(s, e);
    }
    const elapsed = performance.now() - t0;
    expect(s.totalEvents).toBe(80);
    expect(elapsed).toBeLessThan(5);
    // Five workers should be in the order map.
    expect(s.workerOrder.length).toBe(5);
  });

  it("ingests 16 workers × 16 events in well under 20ms (Step-12 stretch)", () => {
    const stream = buildStream(16, 16);
    expect(stream.length).toBe(256);
    const s = emptyState();
    const t0 = performance.now();
    for (const e of stream) {
      applyEvent(s, e);
    }
    const elapsed = performance.now() - t0;
    expect(s.totalEvents).toBe(256);
    expect(elapsed).toBeLessThan(20);
  });

  it("100 events / worker scaled to 5 workers stays under 25ms", () => {
    const stream = buildStream(5, 100);
    const s = emptyState();
    const t0 = performance.now();
    for (const e of stream) {
      applyEvent(s, e);
    }
    const elapsed = performance.now() - t0;
    expect(elapsed).toBeLessThan(25);
  });

  it("Step-12 stretch: 16 workers × 64 events stays under 50ms", () => {
    const stream = buildStream(16, 64);
    expect(stream.length).toBe(1024);
    const s = emptyState();
    const t0 = performance.now();
    for (const e of stream) {
      applyEvent(s, e);
    }
    const elapsed = performance.now() - t0;
    expect(s.totalEvents).toBe(1024);
    expect(elapsed).toBeLessThan(50);
  });
});
