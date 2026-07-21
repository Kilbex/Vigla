import { describe, it, expect, beforeEach, vi } from "vitest";
import { render, screen, waitFor } from "@testing-library/react";
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

function Probe({ id, tag = "probe" }: { id: string; tag?: string }) {
  const identity = useWorkerIdentity(id);
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

  it("treats a rejected/erroring lookup as 'missing' and stops re-fetching", async () => {
    getWorkerInfo.mockRejectedValueOnce(new Error("boom"));

    const first = render(<Probe id="w-c" tag="first" />);
    await waitFor(() => {
      // Stays "pending" → null because the lookup never resolved into
      // an identity; the cache entry is "missing".
      expect(screen.getByTestId("first").textContent).toBe("pending");
    });
    first.unmount();

    // A second consumer doesn't trigger another fetch.
    render(<Probe id="w-c" tag="second" />);
    expect(screen.getByTestId("second").textContent).toBe("pending");
    expect(getWorkerInfo).toHaveBeenCalledTimes(1);
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
