import { act, fireEvent, render, screen, waitFor } from "@testing-library/react";
import { beforeEach, describe, expect, it, vi } from "vitest";

import type { Event, WorkerInfo } from "../bindings";
import ReplayPanel from "../replay/ReplayPanel";
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

function makePages(
  total: number,
  pageSize: number,
  workerId = "w1",
): Event[][] {
  const pages: Event[][] = [];
  for (let start = 0; start < total; start += pageSize) {
    const end = Math.min(start + pageSize, total);
    pages.push(
      Array.from({ length: end - start }, (_, i) =>
        logEvent(workerId, start + i),
      ),
    );
  }
  return pages;
}

function worker(id: string, name = id): WorkerInfo {
  return {
    id,
    name,
    vendor: "claude",
    cli_binary: "claude",
    cli_version: null,
    cwd: "/tmp",
    model: null,
    spawned_at: "2026-05-17T00:00:00.000Z",
    ended_at: null,
  } as WorkerInfo;
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
        useOpsStore.getState().appendReplayPage("w1", res.data);
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
    const pagesA = makePages(2048, 512, "w-a");
    const pagesB = makePages(100, 512, "w-b");
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
          useOpsStore.getState().appendReplayPage(workerId, res.data);
          after = res.data[res.data.length - 1].seq;
          if (res.data.length < 512) break;
        }
      } finally {
        if (myToken === pickToken) useOpsStore.getState().finishReplay();
      }
    };

    // Start A — it hangs on a1Promise.
    useOpsStore.getState().enterReplay([]);
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

  it("drops a deferred page that resolves after returning to live mode", async () => {
    let resolvePage!: (value: { status: "ok"; data: Event[] }) => void;
    vi.mocked(commands.replayWorkerEventsPage).mockImplementation(
      () =>
        new Promise((resolve) => {
          resolvePage = resolve;
        }) as ReturnType<typeof commands.replayWorkerEventsPage>,
    );

    useOpsStore.getState().registerWorker("live-worker", "claude", "Live task");
    useOpsStore.getState().enterReplay([worker("w1", "claude-1")]);
    render(<ReplayPanel />);

    fireEvent.click(screen.getByRole("button", { name: /claude-1/i }));
    await waitFor(() => {
      expect(commands.replayWorkerEventsPage).toHaveBeenCalledTimes(1);
    });
    fireEvent.click(screen.getByRole("button", { name: /back to live/i }));

    await act(async () => {
      resolvePage({ status: "ok", data: [logEvent("w1", 0)] });
      await Promise.resolve();
    });

    const state = useOpsStore.getState();
    expect(state.replay.mode).toBe("live");
    expect(state.replay.events).toEqual([]);
    expect(state.workerOrder).toEqual(["live-worker"]);
    expect(state.workers.w1).toBeUndefined();
  });

  it("invalidates a deferred page and clears loading when the panel unmounts", async () => {
    let resolvePage!: (value: { status: "ok"; data: Event[] }) => void;
    vi.mocked(commands.replayWorkerEventsPage).mockImplementation(
      () =>
        new Promise((resolve) => {
          resolvePage = resolve;
        }) as ReturnType<typeof commands.replayWorkerEventsPage>,
    );

    useOpsStore.getState().enterReplay([worker("w1", "claude-1")]);
    const { unmount } = render(<ReplayPanel />);
    fireEvent.click(screen.getByRole("button", { name: /claude-1/i }));
    await waitFor(() => {
      expect(commands.replayWorkerEventsPage).toHaveBeenCalledTimes(1);
    });

    unmount();
    await act(async () => {
      resolvePage({ status: "ok", data: [logEvent("w1", 0)] });
      await Promise.resolve();
    });

    const replay = useOpsStore.getState().replay;
    expect(replay.events).toEqual([]);
    expect(replay.loading).toBe(false);
  });

  it("keeps paging controls inert until a deferred final page is projected", async () => {
    const firstPage = makePages(512, 512, "w1")[0];
    let resolveFinalPage!: (value: { status: "ok"; data: Event[] }) => void;
    let call = 0;
    vi.mocked(commands.replayWorkerEventsPage).mockImplementation(async () => {
      call += 1;
      if (call === 1) return { status: "ok", data: firstPage };
      return await new Promise((resolve) => {
        resolveFinalPage = resolve;
      });
    });

    useOpsStore.getState().enterReplay([worker("w1", "claude-1")]);
    render(<ReplayPanel />);
    fireEvent.click(screen.getByRole("button", { name: /claude-1/i }));

    await waitFor(() => {
      expect(useOpsStore.getState().replay.events).toHaveLength(512);
      expect(commands.replayWorkerEventsPage).toHaveBeenCalledTimes(2);
    });
    const rewind = screen.getByRole("button", { name: /rewind/i });
    const disabledWhileLoading = (rewind as HTMLButtonElement).disabled;
    fireEvent.click(rewind);

    await act(async () => {
      resolveFinalPage({ status: "ok", data: [logEvent("w1", 512)] });
      await Promise.resolve();
    });
    await waitFor(() => {
      expect(useOpsStore.getState().replay.loading).toBe(false);
    });

    const state = useOpsStore.getState();
    expect(state.replay.position).toBe(513);
    expect(state.workers.w1.eventCount).toBe(513);
    expect(disabledWhileLoading).toBe(true);
  });

  it("surfaces a selected session load error and retries it", async () => {
    vi.mocked(commands.replayWorkerEventsPage)
      .mockResolvedValueOnce({
        status: "error",
        error: "recording is temporarily unavailable",
      })
      .mockResolvedValueOnce({
        status: "ok",
        data: [logEvent("w1", 0)],
      });

    useOpsStore.getState().enterReplay([worker("w1", "claude-1")]);
    render(<ReplayPanel />);
    fireEvent.click(screen.getByRole("button", { name: /claude-1/i }));

    const alert = await screen.findByRole("alert");
    expect(alert).toHaveTextContent("recording is temporarily unavailable");
    expect(
      screen.getByRole("button", { name: /retry replay/i }),
    ).toBeEnabled();

    fireEvent.click(screen.getByRole("button", { name: /retry replay/i }));
    await waitFor(() => {
      expect(commands.replayWorkerEventsPage).toHaveBeenCalledTimes(2);
      expect(useOpsStore.getState().replay.events).toHaveLength(1);
    });
    expect(screen.queryByRole("alert")).not.toBeInTheDocument();
  });

  it("rejects a replay page whose request no longer owns the replay", () => {
    useOpsStore.getState().enterReplay([worker("w-b")]);
    useOpsStore.getState().beginReplay("w-b");

    useOpsStore.getState().appendReplayPage("w-a", [logEvent("w-a", 0)]);

    expect(useOpsStore.getState().replay.events).toEqual([]);
    expect(useOpsStore.getState().workers["w-a"]).toBeUndefined();
  });

  it("accepts an aggregate recording owned by the selected session", () => {
    useOpsStore.getState().enterReplay([worker("mission-recording")]);
    useOpsStore.getState().beginReplay("mission-recording");

    useOpsStore
      .getState()
      .appendReplayPage("mission-recording", [
        logEvent("worker-a", 0),
        logEvent("worker-b", 1),
      ]);

    expect(useOpsStore.getState().replay.events).toHaveLength(2);
    expect(Object.keys(useOpsStore.getState().workers)).toEqual([
      "worker-a",
      "worker-b",
    ]);
  });
});
