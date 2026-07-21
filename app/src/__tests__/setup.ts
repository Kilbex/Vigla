import "@testing-library/jest-dom/vitest";
import { afterEach, vi } from "vitest";
import { cleanup } from "@testing-library/react";

// Ensure DOM is cleaned up after every component test.
afterEach(() => {
  cleanup();
});

// jsdom does not implement scrollIntoView — stub it so components that
// call it (e.g. EventFeed's auto-scroll) don't throw in tests.
if (typeof window !== "undefined") {
  window.HTMLElement.prototype.scrollIntoView = () => {};
}

// jsdom does not implement ResizeObserver — stub it so components
// that use it (e.g. @xyflow/react inside PlanMindMap) don't throw
// in tests.
if (typeof globalThis.ResizeObserver === "undefined") {
  class ResizeObserverStub {
    constructor(_cb: ResizeObserverCallback) {}
    observe(): void {}
    unobserve(): void {}
    disconnect(): void {}
  }
  globalThis.ResizeObserver = ResizeObserverStub as typeof ResizeObserver;
}

// P4 — under vitest, App.tsx's startup-readiness poll would hang
// because no Tauri host is running. Mock the raw `@tauri-apps/api`
// surface so the poll resolves to `true` immediately and the splash
// disappears. Component tests that need to assert splash behaviour
// can override these mocks locally with `vi.mocked(...)`.
vi.mock("@tauri-apps/api/core", () => ({
  invoke: vi.fn(async (cmd: string) => {
    if (cmd === "startup_status") return { phase: "ready", error: null };
    return null;
  }),
  convertFileSrc: (p: string) => p,
}));

vi.mock("@tauri-apps/api/event", () => ({
  listen: vi.fn(async () => () => {}),
  once: vi.fn(async () => () => {}),
  emit: vi.fn(async () => {}),
  TauriEvent: {},
}));
