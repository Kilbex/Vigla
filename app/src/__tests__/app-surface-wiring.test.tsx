// S10 — verifies the App.tsx right-rail surface switch. Mocks
// the Tauri bindings so the test focuses on the new wiring; the
// other overlays (drawer / settings / mission overlay) lazy-load
// behind Suspense and don't render in the default state.

import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { act, fireEvent, render, waitFor } from "@testing-library/react";

const bindingSpies = vi.hoisted(() => ({
  startupStatus: vi.fn(),
  healthCheck: vi.fn(),
  checkCliAuth: vi.fn(),
  workerEventListen: vi.fn(),
  missionEventListen: vi.fn(),
}));

vi.mock("../bindings", async (orig) => {
  const realModule = (await orig()) as Record<string, unknown>;
  // Wrap commands in a Proxy that returns a no-op async function for
  // any unknown property — the App pulls in many command surfaces
  // and listing them all here would drift.
  const baseCommands = (realModule.commands ?? {}) as Record<string, unknown>;
  const commandsProxy = new Proxy(baseCommands, {
    get(target, prop) {
      if (prop === "startupStatus") return bindingSpies.startupStatus;
      if (prop === "healthCheck") return bindingSpies.healthCheck;
      if (prop === "listRecentMissions") {
        return vi.fn().mockResolvedValue({ status: "ok", data: [] });
      }
      if (prop === "checkCliAuth") {
        return bindingSpies.checkCliAuth;
      }
      if (prop in target) return target[prop as string];
      return vi.fn().mockResolvedValue({ status: "ok", data: null });
    },
  });
  return {
    ...realModule,
    events: {
      workerEvent: { listen: bindingSpies.workerEventListen },
      missionEventDto: { listen: bindingSpies.missionEventListen },
    },
    commands: commandsProxy,
  };
});

import App from "../App";
import { useSurfaceStore } from "../inbox/router";
import { setShowAllEvents } from "../settings/preferences";

beforeEach(() => {
  vi.clearAllMocks();
  bindingSpies.startupStatus.mockReset();
  bindingSpies.healthCheck.mockReset();
  bindingSpies.checkCliAuth.mockReset();
  bindingSpies.workerEventListen.mockReset();
  bindingSpies.missionEventListen.mockReset();
  bindingSpies.startupStatus.mockResolvedValue({
    phase: "ready",
    error: null,
  });
  bindingSpies.healthCheck.mockResolvedValue({
    version: "0.0.1",
    uptime_ms: 0,
  });
  bindingSpies.checkCliAuth.mockResolvedValue([]);
  bindingSpies.workerEventListen.mockResolvedValue(() => {});
  bindingSpies.missionEventListen.mockResolvedValue(() => {});
  useSurfaceStore.setState({ surface: "inbox", detail: null });
  setShowAllEvents(false);
});

afterEach(() => {
  vi.useRealTimers();
  setShowAllEvents(false);
});

describe("App surface wiring", () => {
  it("renders InboxOverview by default (surface=inbox)", async () => {
    const { container } = render(<App />);
    await waitFor(() => {
      expect(container.querySelector(".inbox-overview")).not.toBeNull();
    });
    expect(container.querySelector(".comms-feed")).toBeNull();
  });

  it("keeps the deploy form visible on the default empty app surface", async () => {
    const { container } = render(<App />);
    await waitFor(() => {
      expect(container.querySelector(".operations-room-launch")).not.toBeNull();
    });
    expect(container.querySelector(".deploy-panel")).not.toBeNull();
  });

  it("renders CommsFeed when surface=ops_room and showAllEvents on", async () => {
    setShowAllEvents(true);
    useSurfaceStore.setState({ surface: "ops_room", detail: null });
    const { container } = render(<App />);
    await waitFor(() => {
      expect(container.querySelector(".comms-feed")).not.toBeNull();
    });
  });

  it("renders MissionHistory when surface=history", async () => {
    useSurfaceStore.setState({ surface: "history", detail: null });
    const { container } = render(<App />);
    await waitFor(() => {
      expect(container.querySelector(".mission-history")).not.toBeNull();
    });
  });

  it("renders MissionInbox when surface=mission_detail", async () => {
    useSurfaceStore.setState({
      surface: "mission_detail",
      detail: { missionId: "m1" },
    });
    const { container } = render(<App />);
    await waitFor(() => {
      expect(container.querySelector(".mission-inbox")).not.toBeNull();
    });
  });

  it("mounts only startup UI and startup IPC while readiness is unresolved", () => {
    bindingSpies.startupStatus.mockReturnValue(new Promise(() => {}));

    const { container } = render(<App />);

    expect(container.querySelector("[data-testid='startup-splash']")).not.toBeNull();
    expectOperationalSurfacesAbsent(container);
    expect(bindingSpies.startupStatus).toHaveBeenCalledTimes(1);
    expect(bindingSpies.healthCheck).not.toHaveBeenCalled();
    expect(bindingSpies.checkCliAuth).not.toHaveBeenCalled();
    expect(bindingSpies.workerEventListen).not.toHaveBeenCalled();
    expect(bindingSpies.missionEventListen).not.toHaveBeenCalled();
  });

  it("keeps a terminal startup failure isolated from operational IPC", async () => {
    bindingSpies.startupStatus.mockResolvedValue({
      phase: "failed",
      error: "database migration failed",
    });

    const { container } = render(<App />);
    await waitFor(() => {
      expect(container.querySelector("[data-testid='startup-error']")).not.toBeNull();
    });

    expect(container.textContent).toContain("database migration failed");
    expect(container.querySelector("[data-testid='startup-splash']")).toBeNull();
    expectOperationalSurfacesAbsent(container);
    expect(bindingSpies.healthCheck).not.toHaveBeenCalled();
    expect(bindingSpies.checkCliAuth).not.toHaveBeenCalled();
    expect(bindingSpies.workerEventListen).not.toHaveBeenCalled();
    expect(bindingSpies.missionEventListen).not.toHaveBeenCalled();
  });

  it("gates operational controls and shortcuts until both event listeners attach", async () => {
    const workerAttachment = deferred<() => void>();
    const missionAttachment = deferred<() => void>();
    bindingSpies.workerEventListen.mockReturnValue(workerAttachment.promise);
    bindingSpies.missionEventListen.mockReturnValue(missionAttachment.promise);

    const { container } = render(<App />);
    await waitFor(() => {
      expect(bindingSpies.workerEventListen).toHaveBeenCalledTimes(1);
      expect(bindingSpies.missionEventListen).toHaveBeenCalledTimes(1);
    });

    expectOperationalSurfacesAbsent(container);
    fireEvent.keyDown(window, { key: "2", metaKey: true });
    expect(useSurfaceStore.getState().surface).toBe("inbox");

    await act(async () => {
      workerAttachment.resolve(() => {});
      await workerAttachment.promise;
    });
    expectOperationalSurfacesAbsent(container);

    await act(async () => {
      missionAttachment.resolve(() => {});
      await missionAttachment.promise;
    });
    await waitFor(() => {
      expect(container.querySelector(".command-panel")).not.toBeNull();
    });

    fireEvent.keyDown(window, { key: "2", metaKey: true });
    expect(useSurfaceStore.getState().surface).toBe("ops_room");
  });

  it("keeps controls gated through a rejected listener attachment, then recovers", async () => {
    vi.useFakeTimers();
    bindingSpies.missionEventListen
      .mockRejectedValueOnce(new Error("mission listener unavailable"))
      .mockResolvedValueOnce(() => {});

    const { container } = render(<App />);
    await flushEffects();

    expect(bindingSpies.workerEventListen).toHaveBeenCalledTimes(1);
    expect(bindingSpies.missionEventListen).toHaveBeenCalledTimes(1);
    expectOperationalSurfacesAbsent(container);
    fireEvent.keyDown(window, { key: "2", metaKey: true });
    expect(useSurfaceStore.getState().surface).toBe("inbox");

    await act(async () => {
      await vi.advanceTimersByTimeAsync(250);
    });
    await flushEffects();

    expect(bindingSpies.missionEventListen).toHaveBeenCalledTimes(2);
    expect(container.querySelector(".command-panel")).not.toBeNull();
  });

  it("retries worker-event attachment with capped backoff and cleans up success", async () => {
    vi.useFakeTimers();
    const unlisten = vi.fn();
    for (let failure = 0; failure < 7; failure += 1) {
      bindingSpies.workerEventListen.mockRejectedValueOnce(
        new Error(`worker listener unavailable ${failure}`),
      );
    }
    bindingSpies.workerEventListen.mockResolvedValueOnce(unlisten);

    const { unmount } = render(<App />);
    await flushEffects();
    expect(bindingSpies.workerEventListen).toHaveBeenCalledTimes(1);

    const retryDelays = [250, 500, 1_000, 2_000, 4_000, 5_000, 5_000];
    for (const [index, delay] of retryDelays.entries()) {
      await act(async () => {
        await vi.advanceTimersByTimeAsync(delay - 1);
      });
      expect(bindingSpies.workerEventListen).toHaveBeenCalledTimes(index + 1);
      await act(async () => {
        await vi.advanceTimersByTimeAsync(1);
      });
      expect(bindingSpies.workerEventListen).toHaveBeenCalledTimes(index + 2);
    }

    unmount();
    expect(unlisten).toHaveBeenCalledTimes(1);
  });

  it("reattaches the mission-event listener after a transient rejection", async () => {
    vi.useFakeTimers();
    const unlisten = vi.fn();
    bindingSpies.missionEventListen
      .mockRejectedValueOnce(new Error("mission listener unavailable"))
      .mockResolvedValueOnce(unlisten);

    const { unmount } = render(<App />);
    await flushEffects();
    expect(bindingSpies.missionEventListen).toHaveBeenCalledTimes(1);

    await act(async () => {
      await vi.advanceTimersByTimeAsync(249);
    });
    expect(bindingSpies.missionEventListen).toHaveBeenCalledTimes(1);
    await act(async () => {
      await vi.advanceTimersByTimeAsync(1);
    });
    expect(bindingSpies.missionEventListen).toHaveBeenCalledTimes(2);

    unmount();
    expect(unlisten).toHaveBeenCalledTimes(1);
  });

  it("cancels pending listener retries when the operational app unmounts", async () => {
    vi.useFakeTimers();
    bindingSpies.workerEventListen.mockRejectedValue(
      new Error("worker listener unavailable"),
    );
    bindingSpies.missionEventListen.mockRejectedValue(
      new Error("mission listener unavailable"),
    );

    const { unmount } = render(<App />);
    await flushEffects();
    expect(bindingSpies.workerEventListen).toHaveBeenCalledTimes(1);
    expect(bindingSpies.missionEventListen).toHaveBeenCalledTimes(1);

    unmount();
    await vi.advanceTimersByTimeAsync(60_000);

    expect(bindingSpies.workerEventListen).toHaveBeenCalledTimes(1);
    expect(bindingSpies.missionEventListen).toHaveBeenCalledTimes(1);
  });
});

async function flushEffects(): Promise<void> {
  await act(async () => {
    await Promise.resolve();
    await Promise.resolve();
  });
}

function deferred<T>(): {
  promise: Promise<T>;
  resolve: (value: T) => void;
} {
  let resolve!: (value: T) => void;
  const promise = new Promise<T>((next) => {
    resolve = next;
  });
  return { promise, resolve };
}

function expectOperationalSurfacesAbsent(container: HTMLElement): void {
  expect(container.querySelector(".command-panel")).toBeNull();
  expect(container.querySelector(".operations-room")).toBeNull();
  expect(container.querySelector(".inbox-overview")).toBeNull();
  expect(container.querySelector(".mission-history")).toBeNull();
  expect(container.querySelector(".mission-inbox")).toBeNull();
  expect(container.querySelector(".comms-feed")).toBeNull();
  expect(container.querySelector(".deploy-panel")).toBeNull();
}
