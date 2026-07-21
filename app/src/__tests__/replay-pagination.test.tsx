import { act } from "@testing-library/react";
import { beforeEach, describe, expect, it, vi } from "vitest";

import type { Event, WorkerInfo } from "../bindings";
import { useOpsStore } from "../store";

// Static mock — each test overrides `commands.replayWorkerEventsPage`
// via vi.spyOn on the imported object below.
vi.mock("../bindings", async () => {
  const actual = await vi.importActual<typeof import("../bindings")>(
    "../bindings",
  );
  return {
    ...actual,
    commands: {
      ...actual.commands,
      replayWorkerEventsPage: vi.fn(),
      listRecentWorkers: vi.fn(async () => ({ status: "ok", data: [] })),
    },
  };
});

// Re-import so the spy is the mocked instance.
import { commands } from "../bindings";

function logEvent(workerId: string, seq: number): Event {
  return {
    schema_version: "1.0",
    worker_id: workerId,
    task_id: null,
    seq,
    ts: `2026-05-17T00:00:${String(seq % 60).padStart(2, "0")}.000Z`,
    type: "log",
    payload: { level: "info", stream: "stdout", line: `evt ${seq}`, tag: null },
  } as unknown as Event;
}

function makePages(total: number, pageSize: number): Event[][] {
  const pages: Event[][] = [];
  for (let start = 0; start < total; start += pageSize) {
    const end = Math.min(start + pageSize, total);
    pages.push(
      Array.from({ length: end - start }, (_, i) => logEvent("w1", start + i)),
    );
  }
  return pages;
}

function resetStore() {
  const s = useOpsStore.getState();
  s.exitReplay();
  s.reset();
}

describe("replay pagination", () => {
  beforeEach(() => {
    vi.clearAllMocks();
    resetStore();
  });

  it("loads 1500 events across three pages (512/512/476)", async () => {
    const pages = makePages(1500, 512);
    expect(pages.map((p) => p.length)).toEqual([512, 512, 476]);

    const spy = vi.mocked(commands.replayWorkerEventsPage);
    let call = 0;
    spy.mockImplementation(async () => {
      const data = pages[call] ?? [];
      call += 1;
      return { status: "ok", data } as Awaited<
        ReturnType<typeof commands.replayWorkerEventsPage>
      >;
    });

    await act(async () => {
      const seed: WorkerInfo = {
        id: "w1",
        name: "claude-1",
        vendor: "claude",
        cli_binary: "claude",
        cli_version: null,
        cwd: "/tmp",
        model: null,
        spawned_at: "2026-05-17T00:00:00.000Z",
        ended_at: null,
      } as WorkerInfo;
      useOpsStore.getState().enterReplay([seed]);
      useOpsStore.getState().beginReplay("w1");
      // Drive the same loop ReplayPanel.pickSession runs.
      let after: number | null = null;
      while (true) {
        const res = await commands.replayWorkerEventsPage("w1", after, 512);
        if (res.status !== "ok") break;
        if (res.data.length === 0) break;
        useOpsStore.getState().appendReplayPage(res.data);
        after = res.data[res.data.length - 1].seq;
        if (res.data.length < 512) break;
      }
      useOpsStore.getState().finishReplay();
    });

    const replay = useOpsStore.getState().replay;
    expect(replay.events.length).toBe(1500);
    expect(replay.position).toBe(1500);
    expect(replay.loading).toBe(false);
    expect(spy).toHaveBeenCalledTimes(3);
  });

  it("a newer pickSession cancels the in-flight loop", async () => {
    const pagesA = makePages(2048, 512);
    const pagesB = makePages(100, 512);
    const spy = vi.mocked(commands.replayWorkerEventsPage);

    // First A page resolves later (via the promise we hold here); B's
    // single page resolves synchronously. The token-based bail in
    // pickSession means after B starts, A's page-2 (and onward) is
    // never requested.
    let resolveA1!: (v: { status: "ok"; data: Event[] }) => void;
    const a1Promise = new Promise<{ status: "ok"; data: Event[] }>((resolve) => {
      resolveA1 = resolve;
    });
    let aCall = 0;
    let bCall = 0;
    spy.mockImplementation(async (workerId: string) => {
      if (workerId === "w-a") {
        if (aCall === 0) {
          aCall += 1;
          // Await the held promise; once it resolves, the value's
          // shape matches the success branch of Result and the cast
          // narrows it to the spy's return type. Casting the Promise
          // itself (without await) is invalid TS — the awaited inner
          // value is what the spy's signature expects.
          return (await a1Promise) as Awaited<ReturnType<typeof spy>>;
        }
        aCall += 1;
        return { status: "ok", data: pagesA[aCall - 1] ?? [] } as Awaited<
          ReturnType<typeof spy>
        >;
      }
      const data = pagesB[bCall] ?? [];
      bCall += 1;
      return { status: "ok", data } as Awaited<ReturnType<typeof spy>>;
    });

    // Simulate the panel's pickTokenRef pattern inline.
    let pickToken = 0;
    const pickSession = async (workerId: string) => {
      const myToken = ++pickToken;
      useOpsStore.getState().beginReplay(workerId);
      try {
        let after: number | null = null;
        while (true) {
          const res = await commands.replayWorkerEventsPage(workerId, after, 512);
          if (myToken !== pickToken) return; // newer pick won — bail
          if (res.status !== "ok") return;
          if (res.data.length === 0) break;
          useOpsStore.getState().appendReplayPage(res.data);
          after = res.data[res.data.length - 1].seq;
          if (res.data.length < 512) break;
        }
      } finally {
        if (myToken === pickToken) useOpsStore.getState().finishReplay();
      }
    };

    // Start A — it hangs on a1Promise.
    const aDone = pickSession("w-a");
    // Start B — supersedes A.
    const bDone = pickSession("w-b");
    // Now resolve A's first page; it should be discarded.
    resolveA1({ status: "ok", data: pagesA[0] });
    await Promise.all([aDone, bDone]);

    const replay = useOpsStore.getState().replay;
    expect(replay.workerId).toBe("w-b");
    expect(replay.events.length).toBe(100);
    expect(aCall).toBe(1); // A's page-2 was never requested
  });
});
