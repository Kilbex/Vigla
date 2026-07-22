import { describe, it, expect, beforeEach, vi } from "vitest";
import { act, render, screen, waitFor } from "@testing-library/react";
import {
  useWorkerIdentity,
  __resetWorkerIdentityCache,
} from "../useWorkerIdentity";

// Mock the IPC surface so individual tests dictate the WorkerInfo
// outcome and we can spy on the call count for fetch-sharing checks.
vi.mock("../../bindings", () => {
  return {
    commands: {
      getWorkerInfo: vi.fn(),
    },
  };
});

import { commands } from "../../bindings";

const getWorkerInfo = commands.getWorkerInfo as unknown as ReturnType<
  typeof vi.fn
>;

function workerInfo(over: { vendor?: string; model?: string | null } = {}) {
  return {
    id: "w-info",
    name: "n",
    vendor: over.vendor ?? "claude",
    cli_binary: "claude",
    cli_version: null,
    cwd: "/tmp",
    model: over.model ?? null,
    spawned_at: "2026-01-01T00:00:00Z",
    ended_at: null,
  };
}

function Probe({
  id,
  tag = "probe",
  enabled = true,
}: {
  id: string;
  tag?: string;
  enabled?: boolean;
}) {
  const identity = useWorkerIdentity(id, enabled);
  return (
    <div data-testid={tag}>
      {identity
        ? `vendor=${identity.vendor} model=${identity.model ?? "—"}`
        : "pending"}
    </div>
  );
}

beforeEach(() => {
  __resetWorkerIdentityCache();
  getWorkerInfo.mockReset();
});

describe("useWorkerIdentity", () => {
  it("does not request WorkerInfo when event identity is authoritative", async () => {
    getWorkerInfo.mockResolvedValueOnce({
      status: "ok",
      data: workerInfo({ vendor: "claude", model: "stale-model" }),
    });
    render(<Probe id="mock-1" enabled={false} />);

    await act(async () => {
      await Promise.resolve();
    });

    expect(getWorkerInfo).not.toHaveBeenCalled();
    expect(screen.getByTestId("probe")).toHaveTextContent("pending");
  });

  it("returns null until the lookup resolves, then the cached identity", async () => {
    getWorkerInfo.mockResolvedValueOnce({
      status: "ok",
      data: workerInfo({ vendor: "codex", model: "gpt-5.5" }),
    });

    render(<Probe id="w-a" />);
    expect(screen.getByTestId("probe").textContent).toBe("pending");

    await waitFor(() => {
      expect(screen.getByTestId("probe").textContent).toBe(
        "vendor=codex model=gpt-5.5",
      );
    });
  });

  it("caches the identity so a second consumer renders synchronously", async () => {
    getWorkerInfo.mockResolvedValueOnce({
      status: "ok",
      data: workerInfo({ vendor: "gemini", model: "g-pro" }),
    });

    const first = render(<Probe id="w-b" tag="first" />);
    await waitFor(() => {
      expect(screen.getByTestId("first").textContent).toBe(
        "vendor=gemini model=g-pro",
      );
    });
    first.unmount();

    // Second mount of the same id reads the cache on first render —
    // not "pending" — and never re-issues an IPC call.
    expect(getWorkerInfo).toHaveBeenCalledTimes(1);
    render(<Probe id="w-b" tag="second" />);
    expect(screen.getByTestId("second").textContent).toBe(
      "vendor=gemini model=g-pro",
    );
    expect(getWorkerInfo).toHaveBeenCalledTimes(1);
  });

  it("retries a transport failure after a fixed delay, then caches success", async () => {
    vi.useFakeTimers();
    getWorkerInfo
      .mockRejectedValueOnce(new Error("transport down"))
      .mockResolvedValueOnce({
        status: "ok",
        data: workerInfo({ vendor: "codex", model: "gpt-recovered" }),
      });

    try {
      render(<Probe id="w-c" />);
      await act(async () => {
        await Promise.resolve();
        await Promise.resolve();
      });

      expect(getWorkerInfo).toHaveBeenCalledTimes(1);
      expect(screen.getByTestId("probe")).toHaveTextContent("pending");

      await act(async () => {
        vi.advanceTimersByTime(999);
        await Promise.resolve();
      });
      expect(getWorkerInfo).toHaveBeenCalledTimes(1);

      await act(async () => {
        vi.advanceTimersByTime(1);
        await Promise.resolve();
        await Promise.resolve();
      });

      expect(getWorkerInfo).toHaveBeenCalledTimes(2);
      expect(screen.getByTestId("probe")).toHaveTextContent(
        "vendor=codex model=gpt-recovered",
      );
    } finally {
      vi.useRealTimers();
    }
  });

  it("limits resolved command errors before entering the failure cooldown", async () => {
    vi.useFakeTimers();
    getWorkerInfo.mockResolvedValue({
      status: "error",
      error: "repository unavailable",
    });

    try {
      render(<Probe id="w-offline" />);
      await act(async () => {
        await Promise.resolve();
        await Promise.resolve();
      });
      expect(getWorkerInfo).toHaveBeenCalledTimes(1);

      await act(async () => {
        vi.advanceTimersByTime(1_000);
        await Promise.resolve();
        await Promise.resolve();
      });
      expect(getWorkerInfo).toHaveBeenCalledTimes(2);

      await act(async () => {
        vi.advanceTimersByTime(5_000);
        await Promise.resolve();
        await Promise.resolve();
      });
      expect(getWorkerInfo).toHaveBeenCalledTimes(3);

      await act(async () => {
        vi.advanceTimersByTime(29_999);
        await Promise.resolve();
      });
      expect(getWorkerInfo).toHaveBeenCalledTimes(3);

      await act(async () => {
        vi.advanceTimersByTime(1);
        await Promise.resolve();
        await Promise.resolve();
      });
      expect(getWorkerInfo).toHaveBeenCalledTimes(4);
    } finally {
      __resetWorkerIdentityCache();
      vi.useRealTimers();
    }
  });

  it("expires a missing lookup before trying the authoritative source again", async () => {
    vi.useFakeTimers();
    getWorkerInfo
      .mockResolvedValueOnce({ status: "ok", data: null })
      .mockResolvedValueOnce({
        status: "ok",
        data: workerInfo({ vendor: "gemini", model: "gemini-late" }),
      });

    try {
      render(<Probe id="w-late" />);
      await act(async () => {
        await Promise.resolve();
        await Promise.resolve();
      });

      expect(getWorkerInfo).toHaveBeenCalledTimes(1);
      await act(async () => {
        vi.advanceTimersByTime(9_999);
        await Promise.resolve();
      });
      expect(getWorkerInfo).toHaveBeenCalledTimes(1);

      await act(async () => {
        vi.advanceTimersByTime(1);
        await Promise.resolve();
        await Promise.resolve();
      });

      expect(getWorkerInfo).toHaveBeenCalledTimes(2);
      expect(screen.getByTestId("probe")).toHaveTextContent(
        "vendor=gemini model=gemini-late",
      );
    } finally {
      vi.useRealTimers();
    }
  });

  it("shares one fetch across multiple consumers of the same workerId", async () => {
    getWorkerInfo.mockResolvedValueOnce({
      status: "ok",
      data: workerInfo({ vendor: "claude", model: "sonnet" }),
    });

    render(
      <div>
        <Probe id="w-d" tag="one" />
        <Probe id="w-d" tag="two" />
      </div>,
    );

    await waitFor(() => {
      expect(screen.getByTestId("one").textContent).toBe(
        "vendor=claude model=sonnet",
      );
      expect(screen.getByTestId("two").textContent).toBe(
        "vendor=claude model=sonnet",
      );
    });
    expect(getWorkerInfo).toHaveBeenCalledTimes(1);
  });
});
