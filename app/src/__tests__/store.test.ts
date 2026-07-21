import { describe, it, expect, beforeEach } from "vitest";
import type { Event, EventKind } from "../bindings";
import { applyEvent, emptyState } from "../store/ingest";
import {
  selectGlobalCounters,
  selectWorkersNeedingReview,
  useOpsStore,
} from "../store";
import { computeNeedsReview } from "../store/review";
import { initialReplayState } from "../replay/state";

function evt(
  workerId: string,
  seq: number,
  ts: string,
  taskId: string | null,
  kind: EventKind,
): Event {
  return {
    schema_version: "1.0",
    worker_id: workerId,
    task_id: taskId,
    seq,
    ts,
    ...kind,
  } as Event;
}

describe("ingest", () => {
  it("registers a new worker on first event", () => {
    const s = emptyState();
    applyEvent(
      s,
      evt("w1", 0, "2026-01-01T00:00:00.000Z", null, {
        type: "state_change",
        payload: { state: "idle" },
      }),
    );
    expect(s.workerOrder).toEqual(["w1"]);
    expect(s.workers["w1"]?.state).toBe("idle");
    expect(s.totalEvents).toBe(1);
    // 1 alert: started
    expect(s.alerts.length).toBeGreaterThanOrEqual(1);
  });

  it("aggregates progress, file activity, tests, cost", () => {
    const s = emptyState();
    const base = (seq: number, kind: EventKind) =>
      evt("w1", seq, `2026-01-01T00:00:0${seq}.000Z`, "t1", kind);

    applyEvent(s, base(0, { type: "state_change", payload: { state: "executing" } }));
    applyEvent(s, base(1, { type: "progress", payload: { percent: 33, eta_ms: 1000 } }));
    applyEvent(
      s,
      base(2, {
        type: "file_activity",
        payload: { path: "a.ts", op: "created", lines_added: 5 },
      }),
    );
    applyEvent(
      s,
      base(3, {
        type: "file_activity",
        payload: { path: "a.ts", op: "modified", lines_added: 2, lines_removed: 1 },
      }),
    );
    applyEvent(
      s,
      base(4, {
        type: "test_result",
        payload: { suite: "vitest", passed: 5, failed: 0, skipped: 0, duration_ms: 100 },
      }),
    );
    applyEvent(
      s,
      base(5, {
        type: "cost",
        payload: {
          input_tokens: 100,
          output_tokens: 50,
          usd: 0.01,
          model: "claude-sonnet-4-5",
        },
      }),
    );

    const w = s.workers["w1"];
    expect(w.state).toBe("executing");
    expect(w.progress).toBe(33);
    expect(w.etaMs).toBe(1000);
    expect(w.filesAdded).toBe(1);
    expect(w.filesModified).toBe(1);
    expect(w.linesAdded).toBe(7);
    expect(w.linesRemoved).toBe(1);
    expect(w.testsPassed).toBe(5);
    expect(w.costUsd).toBeCloseTo(0.01);
    expect(w.inputTokens).toBe(100);
    expect(w.outputTokens).toBe(50);
    expect(w.model).toBe("claude-sonnet-4-5");
    expect(s.totalSpendUsd).toBeCloseTo(0.01);
  });

  it("emits a completion alert when state goes done", () => {
    const s = emptyState();
    applyEvent(
      s,
      evt("w1", 0, "2026-01-01T00:00:00.000Z", "t1", {
        type: "completion",
        payload: { summary: "all good" },
      }),
    );
    applyEvent(
      s,
      evt("w1", 1, "2026-01-01T00:00:01.000Z", "t1", {
        type: "state_change",
        payload: { state: "done", from: "reviewing" },
      }),
    );
    const completion = s.alerts.find((a) => a.kind === "completion");
    expect(completion?.detail).toBe("all good");
    expect(s.workers["w1"].flashUntil).toBeGreaterThan(0);
  });

  it("emits failure alert and records summary", () => {
    const s = emptyState();
    applyEvent(
      s,
      evt("w1", 0, "2026-01-01T00:00:00.000Z", "t1", {
        type: "failure",
        payload: { error: "boom", retryable: false },
      }),
    );
    expect(s.workers["w1"].failureSummary).toBe("boom");
    expect(s.alerts.find((a) => a.kind === "failure")?.detail).toBe("boom");
  });

  it("tracks blocked dependencies for edge derivation", () => {
    const s = emptyState();
    // Worker A owns task t1, then completes it.
    applyEvent(
      s,
      evt("a", 0, "2026-01-01T00:00:00.000Z", "t1", {
        type: "state_change",
        payload: { state: "executing" },
      }),
    );
    applyEvent(
      s,
      evt("a", 1, "2026-01-01T00:00:01.000Z", "t1", {
        type: "state_change",
        payload: { state: "done", from: "executing" },
      }),
    );
    // Worker B is now waiting on t1.
    applyEvent(
      s,
      evt("b", 0, "2026-01-01T00:00:02.000Z", "t2", {
        type: "dependency",
        payload: { waiting_on: ["t1"], reason: "needs t1" },
      }),
    );
    expect(s.taskOwner["t1"]).toBe("a");
    expect(s.workers["b"].blockedOn).toEqual(["t1"]);
  });

  it("taskOwner follows the latest worker on retry, redirecting blocked-on edges", () => {
    // Regression: worker A picked task t1 and failed; worker C
    // retried the same task. Earlier code never updated taskOwner
    // after the first event, so dependency edges still pointed at
    // dead tile A even though tasks[t1].workerId had moved to C.
    const s = emptyState();
    applyEvent(
      s,
      evt("a", 0, "2026-01-01T00:00:00.000Z", "t1", {
        type: "state_change",
        payload: { state: "executing" },
      }),
    );
    applyEvent(
      s,
      evt("a", 1, "2026-01-01T00:00:01.000Z", "t1", {
        type: "failure",
        payload: { error: "boom", retryable: true },
      }),
    );
    applyEvent(
      s,
      evt("b", 0, "2026-01-01T00:00:02.000Z", "t2", {
        type: "dependency",
        payload: { waiting_on: ["t1"], reason: "needs t1" },
      }),
    );
    // Sanity: edge currently points a -> b.
    expect(s.taskOwner["t1"]).toBe("a");
    expect(
      s.derivedDependencyEdges.find((e) => e.target === "b")?.source,
    ).toBe("a");

    // Worker C retries t1.
    applyEvent(
      s,
      evt("c", 0, "2026-01-01T00:00:03.000Z", "t1", {
        type: "state_change",
        payload: { state: "executing" },
      }),
    );

    expect(s.taskOwner["t1"]).toBe("c");
    expect(s.tasks["t1"].workerId).toBe("c");
    expect(
      s.derivedDependencyEdges.find((e) => e.target === "b")?.source,
    ).toBe("c");
  });

  it("derivedDependencyEdges keeps stable identity when nothing edge-relevant changed", () => {
    // Set up an actual edge: A owns t1, B is blocked on t1.
    const s = emptyState();
    applyEvent(
      s,
      evt("a", 0, "2026-01-01T00:00:00.000Z", "t1", {
        type: "state_change",
        payload: { state: "executing" },
      }),
    );
    applyEvent(
      s,
      evt("b", 0, "2026-01-01T00:00:01.000Z", "t2", {
        type: "dependency",
        payload: { waiting_on: ["t1"], reason: "needs t1" },
      }),
    );
    expect(s.derivedDependencyEdges.length).toBe(1);
    const edgesRef = s.derivedDependencyEdges;

    // A pure log event from a third worker shouldn't change the edge
    // graph; the derived array's identity must stay stable so React
    // Flow doesn't reset edge animations on every event.
    applyEvent(
      s,
      evt("c", 0, "2026-01-01T00:00:02.000Z", null, {
        type: "log",
        payload: { level: "info", stream: "stdout", line: "noise" },
      }),
    );
    expect(s.derivedDependencyEdges).toBe(edgesRef);

    // A state_change on the upstream that completes the dep DOES
    // change content, so the ref must change.
    applyEvent(
      s,
      evt("a", 1, "2026-01-01T00:00:03.000Z", "t1", {
        type: "state_change",
        payload: { state: "done", from: "executing" },
      }),
    );
    expect(s.derivedDependencyEdges).not.toBe(edgesRef);
    expect(s.derivedDependencyEdges[0].state).toBe("done");
  });

  it("detects seq gaps and seq regressions per worker (schema §6)", () => {
    const s = emptyState();
    const at = (seq: number) =>
      evt("w1", seq, `2026-01-01T00:00:0${seq % 10}.000Z`, null, {
        type: "log",
        payload: { level: "info", stream: "stdout", line: `s${seq}` },
      });

    applyEvent(s, at(0));
    applyEvent(s, at(1));
    expect(s.alerts.filter((a) => a.kind === "seq_gap").length).toBe(0);

    // Skip seq 2 — gap.
    applyEvent(s, at(3));
    const gapAlerts = s.alerts.filter((a) => a.kind === "seq_gap");
    expect(gapAlerts.length).toBe(1);
    expect(gapAlerts[0].detail).toMatch(/missing/);

    // Re-emit seq 2 — regression (seq <= last seen).
    applyEvent(s, at(2));
    const regressionAlerts = s.alerts.filter((a) => a.kind === "seq_gap");
    expect(regressionAlerts.length).toBe(2);
    expect(regressionAlerts[0].detail).toMatch(/regression|≤/);
  });

  it("forward-compat: unknown event types only bump counters", () => {
    const s = emptyState();
    applyEvent(s, {
      schema_version: "1.1",
      worker_id: "w1",
      task_id: null,
      seq: 0,
      ts: "2026-01-01T00:00:00.000Z",
      // Cast around the strict union — simulating a future event type
      // arriving from a newer producer.
      type: "future_event" as never,
      payload: { foo: 1 } as never,
    } as Event);
    expect(s.workers["w1"]).toBeDefined();
    expect(s.totalEvents).toBe(1);
  });
});

// ---------------------------------------------------------------------
// Step 17 polish — replay/live coexistence (Fix #2)
// ---------------------------------------------------------------------

function resetStore() {
  // Mirror what `reset()` does, but bypass the action to start each
  // test from a known-empty state without depending on `reset()` itself.
  useOpsStore.setState({
    ...emptyState(),
    replay: initialReplayState,
    liveSnapshot: null,
  });
}

describe("replay live coexistence", () => {
  it("routes ingest to liveSnapshot during replay", () => {
    resetStore();
    // Pretend a worker existed live, then enter replay.
    useOpsStore.getState().enterReplay([]);
    // Now ingest a live event for a NEW worker while in replay.
    useOpsStore.getState().ingest(
      evt("w-live", 0, "2026-05-09T00:00:00.000Z", null, {
        type: "state_change",
        payload: { state: "executing" },
      }),
    );
    const state = useOpsStore.getState();
    // Visible state must NOT see the new worker.
    expect(state.workers["w-live"]).toBeUndefined();
    expect(state.workerOrder).not.toContain("w-live");
    // liveSnapshot must have it.
    expect(state.liveSnapshot).not.toBeNull();
    expect(state.liveSnapshot!.workers["w-live"]?.state).toBe("executing");
    expect(state.liveSnapshot!.workerOrder).toContain("w-live");
  });

  it("enterReplay called during replay preserves liveSnapshot and only updates sessions", () => {
    resetStore();
    // Seed a live worker into visible state via direct ingest.
    useOpsStore.getState().ingest(
      evt("w-pre", 0, "2026-05-09T00:00:00.000Z", null, {
        type: "state_change",
        payload: { state: "executing" },
      }),
    );
    expect(useOpsStore.getState().workers["w-pre"]).toBeDefined();

    // Enter replay first time — snapshot live.
    useOpsStore.getState().enterReplay([]);
    expect(useOpsStore.getState().liveSnapshot).not.toBeNull();
    expect(useOpsStore.getState().liveSnapshot!.workers["w-pre"]).toBeDefined();
    expect(useOpsStore.getState().workers["w-pre"]).toBeUndefined();

    // Ingest a live event for a different worker during replay.
    useOpsStore.getState().ingest(
      evt("w-during", 0, "2026-05-09T00:00:01.000Z", null, {
        type: "state_change",
        payload: { state: "idle" },
      }),
    );
    expect(useOpsStore.getState().liveSnapshot!.workers["w-during"]).toBeDefined();

    // Second enterReplay (the ReplayPanel.tsx:46 path that populates sessions).
    const sessions = [
      {
        id: "w-pre",
        name: "pre",
        vendor: "mock" as const,
        cli_binary: "",
        cli_version: null,
        cwd: "",
        model: null,
        spawned_at: "2026-05-09T00:00:00.000Z",
        ended_at: null,
      },
    ];
    useOpsStore.getState().enterReplay(sessions);

    // liveSnapshot must still hold BOTH workers (not be re-snapshotted from empty visible).
    const live = useOpsStore.getState().liveSnapshot;
    expect(live).not.toBeNull();
    expect(live!.workers["w-pre"]).toBeDefined();
    expect(live!.workers["w-during"]).toBeDefined();
    // Sessions list must reflect the second call.
    expect(useOpsStore.getState().replay.sessions).toEqual(sessions);
    // Replay state-machine fields must be preserved (not reset).
    expect(useOpsStore.getState().replay.mode).toBe("replay");
  });

  it("enterReplay then exitReplay round-trips with new live events", () => {
    resetStore();
    // Spawn worker A live.
    useOpsStore.getState().ingest(
      evt("w-a", 0, "2026-05-09T00:00:00.000Z", null, {
        type: "state_change",
        payload: { state: "executing" },
      }),
    );
    // Enter replay.
    useOpsStore.getState().enterReplay([]);
    // Two events arrive during replay: one updates A, one introduces B.
    useOpsStore.getState().ingest(
      evt("w-a", 1, "2026-05-09T00:00:01.000Z", null, {
        type: "state_change",
        payload: { state: "done" },
      }),
    );
    useOpsStore.getState().ingest(
      evt("w-b", 0, "2026-05-09T00:00:02.000Z", null, {
        type: "state_change",
        payload: { state: "executing" },
      }),
    );
    // Exit replay — visible should now reflect both A's update and B's existence.
    useOpsStore.getState().exitReplay();
    const state = useOpsStore.getState();
    expect(state.workers["w-a"]?.state).toBe("done");
    expect(state.workers["w-b"]?.state).toBe("executing");
    expect(state.workerOrder).toEqual(["w-a", "w-b"]);
    expect(state.liveSnapshot).toBeNull();          // cleared after exit
    expect(state.replay.mode).toBe("live");
  });

  it("reset during replay clears both visible and liveSnapshot", () => {
    resetStore();
    useOpsStore.getState().ingest(
      evt("w-a", 0, "2026-05-09T00:00:00.000Z", null, {
        type: "state_change",
        payload: { state: "executing" },
      }),
    );
    useOpsStore.getState().enterReplay([]);
    useOpsStore.getState().ingest(
      evt("w-b", 0, "2026-05-09T00:00:01.000Z", null, {
        type: "state_change",
        payload: { state: "executing" },
      }),
    );
    expect(useOpsStore.getState().liveSnapshot).not.toBeNull();

    useOpsStore.getState().reset();
    const state = useOpsStore.getState();
    expect(state.workerOrder).toEqual([]);
    expect(Object.keys(state.workers)).toEqual([]);
    expect(state.liveSnapshot).toBeNull();
    expect(state.replay.mode).toBe("live");
  });
});

// ---------------------------------------------------------------------
// Audit round 4 (2026-05-09) — store ref-aliasing immutability.
// applyEvent mutates `workers[wid]` and `tasks[tid]` entries in place
// for performance; the wrapping ingest paths (`applyToOpsState` for
// live, `projectReplay` for replay) must therefore deep-enough-clone
// any entry applyEvent might touch — otherwise a Zustand commit will
// leave prior snapshots' refs pointing at mutated objects.
// ---------------------------------------------------------------------

describe("store ref-aliasing immutability", () => {
  it("setReplayPosition forward step does not mutate prior worker refs", () => {
    resetStore();
    const events: Event[] = [
      evt("w1", 0, "2026-05-09T00:00:00.000Z", null, {
        type: "state_change",
        payload: { state: "idle" },
      }),
      evt("w1", 1, "2026-05-09T00:00:01.000Z", null, {
        type: "state_change",
        payload: { state: "executing", from: "idle" },
      }),
      evt("w1", 2, "2026-05-09T00:00:02.000Z", null, {
        type: "state_change",
        payload: { state: "done", from: "executing" },
      }),
    ];
    useOpsStore.getState().enterReplay([]);
    useOpsStore.getState().beginReplay("w1");
    useOpsStore.getState().appendReplayPage(events);
    useOpsStore.getState().finishReplay();
    // finishReplay sets position to events.length (3).

    // Backward to position 1 — only the first event applied.
    useOpsStore.getState().setReplayPosition(1);
    const refAtPos1 = useOpsStore.getState().workers["w1"];
    expect(refAtPos1.state).toBe("idle");

    // Forward to position 2 — second event applied. Without per-entry
    // cloning, the local `clone` in projectReplay would have shared the
    // worker ref with refAtPos1 and applyEvent would have mutated its
    // .state to "executing" in place.
    useOpsStore.getState().setReplayPosition(2);
    expect(refAtPos1.state).toBe("idle");
    const refAtPos2 = useOpsStore.getState().workers["w1"];
    expect(refAtPos2.state).toBe("executing");
    expect(refAtPos2).not.toBe(refAtPos1);
  });

  it("setReplayPosition forward step does not mutate prior task refs", () => {
    resetStore();
    const events: Event[] = [
      evt("w1", 0, "2026-05-09T00:00:00.000Z", "t1", {
        type: "state_change",
        payload: { state: "idle" },
      }),
      evt("w1", 1, "2026-05-09T00:00:01.000Z", "t1", {
        type: "state_change",
        payload: { state: "executing", from: "idle" },
      }),
      evt("w1", 2, "2026-05-09T00:00:02.000Z", "t1", {
        type: "state_change",
        payload: { state: "done", from: "executing" },
      }),
    ];
    useOpsStore.getState().enterReplay([]);
    useOpsStore.getState().beginReplay("w1");
    useOpsStore.getState().appendReplayPage(events);
    useOpsStore.getState().finishReplay();

    useOpsStore.getState().setReplayPosition(1);
    const taskAtPos1 = useOpsStore.getState().tasks["t1"];
    expect(taskAtPos1.state).toBe("idle");

    useOpsStore.getState().setReplayPosition(2);
    expect(taskAtPos1.state).toBe("idle");
    expect(useOpsStore.getState().tasks["t1"].state).toBe("executing");
  });

  it("ingest in live mode does not mutate prior worker refs", () => {
    resetStore();
    useOpsStore.getState().ingest(
      evt("w1", 0, "2026-05-09T00:00:00.000Z", null, {
        type: "state_change",
        payload: { state: "idle" },
      }),
    );
    const refBefore = useOpsStore.getState().workers["w1"];
    expect(refBefore.state).toBe("idle");
    expect(refBefore.eventCount).toBe(1);

    useOpsStore.getState().ingest(
      evt("w1", 1, "2026-05-09T00:00:01.000Z", null, {
        type: "state_change",
        payload: { state: "executing", from: "idle" },
      }),
    );
    // refBefore is the OUTGOING snapshot's worker; its fields must be
    // frozen at the moment of capture, not retroactively mutated.
    expect(refBefore.state).toBe("idle");
    expect(refBefore.eventCount).toBe(1);
    // Fresh ref has the new values.
    expect(useOpsStore.getState().workers["w1"].state).toBe("executing");
    expect(useOpsStore.getState().workers["w1"].eventCount).toBe(2);
  });

  it("ingest in live mode does not mutate prior task refs", () => {
    resetStore();
    useOpsStore.getState().ingest(
      evt("w1", 0, "2026-05-09T00:00:00.000Z", "t1", {
        type: "state_change",
        payload: { state: "idle" },
      }),
    );
    const taskBefore = useOpsStore.getState().tasks["t1"];
    expect(taskBefore.state).toBe("idle");

    useOpsStore.getState().ingest(
      evt("w1", 1, "2026-05-09T00:00:01.000Z", "t1", {
        type: "state_change",
        payload: { state: "executing", from: "idle" },
      }),
    );
    // applyEvent mutates state.tasks[tid].state in place; without a
    // post-clone in applyToOpsState, taskBefore.state would now read
    // "executing".
    expect(taskBefore.state).toBe("idle");
    expect(useOpsStore.getState().tasks["t1"].state).toBe("executing");
  });
});

describe("registerWorker", () => {
  it("creates a worker with vendor and title", () => {
    resetStore();
    useOpsStore.getState().registerWorker("w-real", "claude", "fix the bug");
    const state = useOpsStore.getState();
    expect(state.workers["w-real"]).toBeDefined();
    expect(state.workers["w-real"].vendor).toBe("claude");
    expect(state.workers["w-real"].model).toBeNull();
    expect(state.workers["w-real"].currentTaskTitle).toBe("fix the bug");
    expect(state.workerOrder).toContain("w-real");
    // One "spawned" alert pushed.
    const startedAlerts = state.alerts.filter(
      (a) => a.kind === "started" && a.workerId === "w-real",
    );
    expect(startedAlerts.length).toBe(1);
  });

  it("registerWorker can seed and patch a model", () => {
    resetStore();
    useOpsStore
      .getState()
      .registerWorker("w-model", "claude", "task", "claude-sonnet-4-5");
    expect(useOpsStore.getState().workers["w-model"].model).toBe(
      "claude-sonnet-4-5",
    );
    useOpsStore.getState().setWorkerModel("w-model", "claude-opus-4-7");
    expect(useOpsStore.getState().workers["w-model"].model).toBe(
      "claude-opus-4-7",
    );
  });

  it("patches existing worker if event arrived first", () => {
    resetStore();
    // Simulate the race: applyEvent's first-event branch runs first.
    useOpsStore.getState().ingest(
      evt("w-race", 0, "2026-05-09T00:00:00.000Z", null, {
        type: "state_change",
        payload: { state: "executing" },
      }),
    );
    // Sanity: exists with default vendor.
    expect(useOpsStore.getState().workers["w-race"].vendor).toBe("mock");
    expect(useOpsStore.getState().workers["w-race"].state).toBe("executing");
    expect(useOpsStore.getState().workers["w-race"].eventCount).toBe(1);

    // Now IPC reply arrives — registerWorker patches.
    useOpsStore.getState().registerWorker("w-race", "codex", "race title");
    const w = useOpsStore.getState().workers["w-race"];
    // Identity overwritten...
    expect(w.vendor).toBe("codex");
    expect(w.currentTaskTitle).toBe("race title");
    // ...event-derived fields preserved.
    expect(w.state).toBe("executing");
    expect(w.eventCount).toBe(1);

    // No duplicate spawned alert (applyEvent's first-event branch
    // already pushed one; registerWorker patch must NOT push another).
    const startedAlerts = useOpsStore
      .getState()
      .alerts.filter((a) => a.kind === "started" && a.workerId === "w-race");
    expect(startedAlerts.length).toBe(1);
  });

  it("during replay writes to liveSnapshot not visible", () => {
    resetStore();
    useOpsStore.getState().enterReplay([]);
    useOpsStore.getState().registerWorker("w-replay-spawn", "claude", "started during replay");

    const state = useOpsStore.getState();
    // Visible state is the replay projection — must NOT have it.
    expect(state.workers["w-replay-spawn"]).toBeUndefined();
    // liveSnapshot must have it.
    expect(state.liveSnapshot!.workers["w-replay-spawn"]?.vendor).toBe("claude");
    expect(state.liveSnapshot!.workers["w-replay-spawn"]?.currentTaskTitle).toBe(
      "started during replay",
    );

    // Exit replay — visible should now have the registered worker.
    useOpsStore.getState().exitReplay();
    const after = useOpsStore.getState();
    expect(after.workers["w-replay-spawn"]?.vendor).toBe("claude");
    expect(after.workers["w-replay-spawn"]?.currentTaskTitle).toBe(
      "started during replay",
    );
  });
});

describe("Batch 3 — review selector + focus", () => {
  beforeEach(() => {
    useOpsStore.getState().reset();
  });

  function seedDone(workerId: string) {
    useOpsStore.getState().registerWorker(workerId, "claude", "task");
    useOpsStore.setState((prev) => ({
      workers: {
        ...prev.workers,
        [workerId]: { ...prev.workers[workerId], state: "done" },
      },
      // P3 — bypassing applyEvent's state_change handler, so
      // maintain the derived counters manually.
      activeCount: prev.activeCount - 1,
      needsInputCount: prev.needsInputCount + 1,
    }));
  }

  it("selectWorkersNeedingReview excludes accepted/rejected/parked workers", () => {
    seedDone("w-a");
    seedDone("w-b");
    seedDone("w-c");
    seedDone("w-d");
    useOpsStore.getState().setReviewStatus("w-b", "accepted");
    useOpsStore.getState().setReviewStatus("w-c", "rejected");
    useOpsStore.getState().setReviewStatus("w-d", "parked");
    const queue = useOpsStore.getState().workerOrder.filter((wid) => {
      const s = useOpsStore.getState().reviewStatus[wid];
      return s === undefined || s === "needs_review";
    });
    // We just sanity-check the underlying state matches the
    // computed selector — import to avoid TS circular issue.
    expect(selectWorkersNeedingReview(useOpsStore.getState())).toEqual(["w-a"]);
    expect(queue).toEqual(["w-a"]);
  });

  it("selectWorkersNeedingReview keeps explicit needs_review", () => {
    seedDone("w-x");
    useOpsStore.getState().setReviewStatus("w-x", "needs_review");
    expect(selectWorkersNeedingReview(useOpsStore.getState())).toEqual(["w-x"]);
  });

  it("selectGlobalCounters.needsInput tracks the queue", () => {
    seedDone("w-1");
    seedDone("w-2");
    expect(selectGlobalCounters(useOpsStore.getState()).needsInput).toBe(2);
    useOpsStore.getState().setReviewStatus("w-1", "accepted");
    expect(selectGlobalCounters(useOpsStore.getState()).needsInput).toBe(1);
  });

  it("setReviewFocus stores the focused worker id", () => {
    seedDone("w-f");
    useOpsStore.getState().setReviewFocus("w-f");
    expect(useOpsStore.getState().reviewFocusedWorkerId).toBe("w-f");
    useOpsStore.getState().setReviewFocus(null);
    expect(useOpsStore.getState().reviewFocusedWorkerId).toBeNull();
  });

  it("accept on focused worker auto-advances focus", () => {
    seedDone("w-a");
    seedDone("w-b");
    seedDone("w-c");
    useOpsStore.getState().setReviewFocus("w-a");
    useOpsStore.getState().setReviewStatus("w-a", "accepted");
    // Focus should move to the next queue member (w-b).
    expect(useOpsStore.getState().reviewFocusedWorkerId).toBe("w-b");
  });

  it("accept on last focused worker clamps to last remaining", () => {
    seedDone("w-a");
    seedDone("w-b");
    useOpsStore.getState().setReviewFocus("w-b");
    useOpsStore.getState().setReviewStatus("w-b", "accepted");
    expect(useOpsStore.getState().reviewFocusedWorkerId).toBe("w-a");
  });

  it("accept on the only focused worker clears focus to null", () => {
    seedDone("w-only");
    useOpsStore.getState().setReviewFocus("w-only");
    useOpsStore.getState().setReviewStatus("w-only", "accepted");
    expect(useOpsStore.getState().reviewFocusedWorkerId).toBeNull();
  });

  it("P3 — derived counters match O(N) computation across happy paths and failures", () => {
    // Drive five workers through a mix of transitions via ingest.
    // After each transition, the scalar counters must equal what an
    // O(N) sweep of `s.workers` and `computeNeedsReview(s)` would
    // return — that's the entire invariant P3 trades.
    const store = useOpsStore.getState();
    const wids = ["w-a", "w-b", "w-c", "w-d", "w-e"];
    let seq = 0;
    function emit(workerId: string, payload: { state: "executing" | "done" | "failed" | "blocked"; from?: string }) {
      useOpsStore.getState().ingest({
        type: "state_change",
        worker_id: workerId,
        seq: seq++,
        ts: new Date(Date.now() + seq).toISOString(),
        task_id: null,
        // @ts-expect-error — payload-shape narrowing varies by event kind
        payload,
      });
    }
    function check() {
      const s = useOpsStore.getState();
      const expectedActive = Object.values(s.workers).filter(
        (w) => w.state !== "done" && w.state !== "failed",
      ).length;
      const expectedNeeds = computeNeedsReview(s).length;
      expect(s.activeCount).toBe(expectedActive);
      expect(s.needsInputCount).toBe(expectedNeeds);
    }
    for (const w of wids) {
      store.registerWorker(w, "claude", "t");
      check();
    }
    // Three workers complete; two fail.
    emit("w-a", { state: "done" });
    check();
    emit("w-b", { state: "done" });
    check();
    emit("w-c", { state: "failed" });
    check();
    // Accept one — needsInput drops.
    store.setReviewStatus("w-a", "accepted");
    check();
    // Park another — needsInput drops further.
    store.setReviewStatus("w-c", "parked");
    check();
    // Retry — re-enters active.
    emit("w-c", { state: "executing", from: "failed" });
    check();
    // Re-finish.
    emit("w-c", { state: "done" });
    check();
    // Reset a status back to needs_review.
    store.setReviewStatus("w-a", "needs_review");
    check();
  });
});

describe("replay auto-play (FE-1)", () => {
  beforeEach(() => {
    useOpsStore.getState().reset();
  });

  function enterWithTwoEvents() {
    const st = useOpsStore.getState();
    st.enterReplay([]);
    st.beginReplay("w1");
    st.appendReplayPage([
      evt("w1", 0, "2026-01-01T00:00:00.000Z", null, {
        type: "state_change",
        payload: { state: "idle" },
      }),
      evt("w1", 1, "2026-01-01T00:00:01.000Z", null, {
        type: "state_change",
        payload: { state: "executing", from: "idle" },
      }),
    ]);
  }

  it("advanceReplay keeps playing until end-of-stream; stepReplay pauses", () => {
    enterWithTwoEvents();
    // appendReplayPage parks position at the end; rewind to play forward.
    useOpsStore.getState().setReplayPosition(0);
    useOpsStore.getState().setReplayPlaying(true);
    expect(useOpsStore.getState().replay.playing).toBe(true);

    // The ticker advance must NOT pause mid-stream (the FE-1 regression:
    // stepReplay forced playing:false, killing auto-play after one tick).
    useOpsStore.getState().advanceReplay(1);
    expect(useOpsStore.getState().replay.position).toBe(1);
    expect(useOpsStore.getState().replay.playing).toBe(true);

    // Reaching the end auto-stops playback.
    useOpsStore.getState().advanceReplay(1);
    expect(useOpsStore.getState().replay.position).toBe(2);
    expect(useOpsStore.getState().replay.playing).toBe(false);

    // A manual step always pauses, regardless of prior playing state.
    useOpsStore.getState().setReplayPlaying(true);
    useOpsStore.getState().stepReplay(-1);
    expect(useOpsStore.getState().replay.playing).toBe(false);
  });
});
