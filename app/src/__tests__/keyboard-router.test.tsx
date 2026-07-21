// S10 — verifies the cmd-digit keyboard router behaviour. ⌘1 /
// ⌘2 / ⌘3 transition the surface store; bare digit keys still
// route to mock-spawning (preserving B3 behaviour).

import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { act, renderHook } from "@testing-library/react";

vi.mock("../bindings", () => ({
  commands: {
    startMockWorker: vi.fn().mockResolvedValue({ status: "ok" }),
    retryWorker: vi.fn(),
  },
}));

import { useGlobalKeyboard } from "../keyboard";
import { useSurfaceStore } from "../inbox/router";
import { getShowAllEvents, setShowAllEvents } from "../settings/preferences";
import { commands } from "../bindings";

const fireCmd = (key: string) =>
  window.dispatchEvent(
    new KeyboardEvent("keydown", { key, metaKey: true, bubbles: true }),
  );
const fireBare = (key: string) =>
  window.dispatchEvent(new KeyboardEvent("keydown", { key, bubbles: true }));

beforeEach(() => {
  useSurfaceStore.setState({ surface: "inbox", detail: null });
  vi.clearAllMocks();
  setShowAllEvents(false);
});

afterEach(() => setShowAllEvents(false));

describe("keyboard router", () => {
  it("⌘1 / ⌘2 / ⌘3 navigate to inbox / ops_room / history", () => {
    renderHook(() => useGlobalKeyboard({ onOpenSettings: () => {} }));
    useSurfaceStore.getState().setSurface("history");
    act(() => fireCmd("1"));
    expect(useSurfaceStore.getState().surface).toBe("inbox");
    act(() => fireCmd("2"));
    expect(useSurfaceStore.getState().surface).toBe("ops_room");
    act(() => fireCmd("3"));
    expect(useSurfaceStore.getState().surface).toBe("history");
  });

  it("⌘2 auto-enables showAllEvents when off", () => {
    renderHook(() => useGlobalKeyboard({ onOpenSettings: () => {} }));
    act(() => fireCmd("2"));
    expect(useSurfaceStore.getState().surface).toBe("ops_room");
    expect(getShowAllEvents()).toBe(true);
  });

  it("bare digits still spawn mocks without flipping the surface", () => {
    renderHook(() => useGlobalKeyboard({ onOpenSettings: () => {} }));
    act(() => fireBare("1"));
    expect(commands.startMockWorker).toHaveBeenCalled();
    expect(useSurfaceStore.getState().surface).toBe("inbox");
  });
});
