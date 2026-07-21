import { describe, expect, it } from "vitest";
import type { Event, EventKind } from "../bindings";
import {
  computeStationNodes,
  type StationNodeCache,
} from "../operations/OperationsRoom";
import { emptyState } from "../store/ingest";
import { applyToOpsState } from "../store/ops-state";

/// Regression coverage for the OperationsRoom node-cache.
///
/// The store re-allocates the `workers` dict on every event but clones
/// only the touched worker's entry (see `applyToOpsState`). Without a
/// cache, OperationsRoom's memo rebuilt all N StationNode wrappers per
/// event, defeating React Flow's ability to short-circuit unchanged
/// tiles. These tests pin the surgical invariant: an event mutates
/// exactly the one node it should, and a burst allocates O(events) not
/// O(events × workers).

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
          : phase < 0.5
            ? {
                type: "progress",
                payload: { percent: phase * 100, eta_ms: 1000 },
              }
            : { type: "state_change", payload: { state: "executing" } };
      out.push(evt(wid, i, tid, kind));
    }
  }
  return out;
}

function countFreshAllocs(prev: StationNodeCache, next: StationNodeCache): number {
  let n = 0;
  for (const [id, node] of next) {
    if (prev.get(id) !== node) n++;
  }
  return n;
}

describe("OperationsRoom — node-cache stability", () => {
  it("only the touched worker gets a new StationNode ref per event", () => {
    let state = emptyState();
    let cache: StationNodeCache = new Map();

    // Seed three workers, each with one event so the cache is populated.
    for (const wid of ["worker-0", "worker-1", "worker-2"]) {
      state = applyToOpsState(
        state,
        evt(wid, 0, `task-${wid}`, {
          type: "state_change",
          payload: { state: "idle" },
        }),
      );
      ({ nextCache: cache } = computeStationNodes(
        cache,
        state.workerOrder,
        state.workers,
      ));
    }

    const before = new Map(cache);
    state = applyToOpsState(
      state,
      evt("worker-1", 1, "task-worker-1", {
        type: "progress",
        payload: { percent: 50, eta_ms: 500 },
      }),
    );
    const { nextCache: after } = computeStationNodes(
      cache,
      state.workerOrder,
      state.workers,
    );

    expect(after.get("worker-0")).toBe(before.get("worker-0"));
    expect(after.get("worker-1")).not.toBe(before.get("worker-1"));
    expect(after.get("worker-2")).toBe(before.get("worker-2"));
  });

  it("a 16-worker × 64-event burst allocates exactly one node per event", () => {
    const stream = buildStream(16, 64);
    expect(stream.length).toBe(1024);

    let state = emptyState();
    let cache: StationNodeCache = new Map();
    let totalAllocs = 0;

    for (const e of stream) {
      state = applyToOpsState(state, e);
      const { nextCache } = computeStationNodes(
        cache,
        state.workerOrder,
        state.workers,
      );
      totalAllocs += countFreshAllocs(cache, nextCache);
      cache = nextCache;
    }

    // Each event mutates exactly its target worker's snapshot ref, so
    // each event produces exactly one fresh StationNode wrapper. The
    // pre-fix path allocated 16 × 1024 = 16,384 wrappers — this
    // assertion would have caught it loudly.
    expect(totalAllocs).toBe(1024);
    expect(state.workerOrder.length).toBe(16);
    expect(cache.size).toBe(16);
  });

  it("position changes for the same worker still invalidate the cache", () => {
    // If a future change makes layoutFor depend on worker count or
    // anything other than index, an existing tile that visually moves
    // must allocate a fresh Node so React Flow re-positions it. The
    // cache equality check on x/y guarantees this.
    let state = emptyState();
    state = applyToOpsState(
      state,
      evt("worker-0", 0, "task-0", {
        type: "state_change",
        payload: { state: "idle" },
      }),
    );

    const { nodes: firstNodes, nextCache: firstCache } = computeStationNodes(
      new Map(),
      state.workerOrder,
      state.workers,
    );
    expect(firstNodes).toHaveLength(1);

    // Re-run with a manually shifted workerOrder so worker-0's index — and
    // therefore its computed layout position — changes. Snapshot ref is
    // identical, so an index-blind cache would (wrongly) reuse the node.
    const shifted = ["other-worker", "worker-0"] as const;
    const workersWithGhost = {
      ...state.workers,
      "other-worker": {
        ...state.workers["worker-0"],
        id: "other-worker",
      },
    };
    const { nextCache: secondCache } = computeStationNodes(
      firstCache,
      shifted,
      workersWithGhost,
    );

    expect(secondCache.get("worker-0")).not.toBe(firstCache.get("worker-0"));
  });
});
