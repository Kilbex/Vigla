import { describe, it, expect, beforeEach, vi } from "vitest";
import { fireEvent, render } from "@testing-library/react";
import { useOpsStore } from "../store";
import { useGlobalKeyboard } from "../keyboard";

vi.mock("../bindings", () => ({
  commands: {
    retryWorker: vi.fn().mockResolvedValue({ status: "ok", data: null }),
    continueWorker: vi.fn().mockResolvedValue({ status: "ok", data: null }),
    startMockWorker: vi.fn().mockResolvedValue({ status: "ok", data: null }),
  },
}));

import { commands } from "../bindings";

function Harness() {
  useGlobalKeyboard({ onOpenSettings: () => {} });
  return null;
}

function seedDone(workerId: string, vendor: "claude" | "codex" | "gemini" | "mock" = "claude") {
  useOpsStore.getState().registerWorker(workerId, vendor, "task");
  useOpsStore.setState((prev) => ({
    workers: {
      ...prev.workers,
      [workerId]: { ...prev.workers[workerId], state: "done" },
    },
  }));
}

describe("Batch 3 — keyboard map", () => {
  beforeEach(() => {
    vi.clearAllMocks();
    useOpsStore.getState().reset();
  });

  it("J advances focus from null to first; K from null to last", () => {
    seedDone("w-a");
    seedDone("w-b");
    seedDone("w-c");
    render(<Harness />);

    fireEvent.keyDown(window, { key: "j" });
    expect(useOpsStore.getState().reviewFocusedWorkerId).toBe("w-a");

    useOpsStore.getState().setReviewFocus(null);
    fireEvent.keyDown(window, { key: "k" });
    expect(useOpsStore.getState().reviewFocusedWorkerId).toBe("w-c");
  });

  it("J/K clamp at the ends of the queue", () => {
    seedDone("w-a");
    seedDone("w-b");
    render(<Harness />);

    useOpsStore.getState().setReviewFocus("w-b");
    fireEvent.keyDown(window, { key: "j" });
    expect(useOpsStore.getState().reviewFocusedWorkerId).toBe("w-b");

    useOpsStore.getState().setReviewFocus("w-a");
    fireEvent.keyDown(window, { key: "k" });
    expect(useOpsStore.getState().reviewFocusedWorkerId).toBe("w-a");
  });

  it("O opens the drawer on focused worker", () => {
    seedDone("w-o");
    render(<Harness />);
    useOpsStore.getState().setReviewFocus("w-o");
    fireEvent.keyDown(window, { key: "o" });
    expect(useOpsStore.getState().selectedWorkerId).toBe("w-o");
  });

  it("R calls retryWorker for Claude focused worker", () => {
    seedDone("w-claude", "claude");
    render(<Harness />);
    useOpsStore.getState().setReviewFocus("w-claude");
    fireEvent.keyDown(window, { key: "r" });
    expect(commands.retryWorker).toHaveBeenCalledWith("w-claude");
  });

  it("R is a no-op for Codex focused worker", () => {
    seedDone("w-codex", "codex");
    render(<Harness />);
    useOpsStore.getState().setReviewFocus("w-codex");
    fireEvent.keyDown(window, { key: "r" });
    expect(commands.retryWorker).not.toHaveBeenCalled();
  });

  it("⇧R dispatches continue-expand event for Claude worker", () => {
    seedDone("w-shift", "claude");
    render(<Harness />);
    useOpsStore.getState().setReviewFocus("w-shift");

    let detail: any = null;
    const listener = (e: Event) => {
      detail = (e as CustomEvent).detail;
    };
    window.addEventListener("vigla:continue-expand", listener);

    fireEvent.keyDown(window, { key: "R", shiftKey: true });
    expect(detail?.workerId).toBe("w-shift");

    window.removeEventListener("vigla:continue-expand", listener);
  });

  it("A mutates reviewStatus to accepted", () => {
    seedDone("w-a", "claude");
    render(<Harness />);
    useOpsStore.getState().setReviewFocus("w-a");
    fireEvent.keyDown(window, { key: "a" });
    expect(useOpsStore.getState().reviewStatus["w-a"]).toBe("accepted");
  });

  it("X mutates reviewStatus to rejected", () => {
    seedDone("w-x", "claude");
    render(<Harness />);
    useOpsStore.getState().setReviewFocus("w-x");
    fireEvent.keyDown(window, { key: "x" });
    expect(useOpsStore.getState().reviewStatus["w-x"]).toBe("rejected");
  });

  it("A/X/R/⇧R are inert in replay mode", () => {
    seedDone("w-rep", "claude");
    useOpsStore.getState().setReviewFocus("w-rep");
    useOpsStore.getState().enterReplay([]);
    render(<Harness />);
    fireEvent.keyDown(window, { key: "a" });
    fireEvent.keyDown(window, { key: "x" });
    fireEvent.keyDown(window, { key: "r" });
    fireEvent.keyDown(window, { key: "R", shiftKey: true });
    expect(commands.retryWorker).not.toHaveBeenCalled();
    // accept/reject not applied either.
    expect(useOpsStore.getState().reviewStatus["w-rep"]).toBeUndefined();
  });

  it("keys are inert when focus is in a textarea", () => {
    seedDone("w-ta", "claude");
    useOpsStore.getState().setReviewFocus("w-ta");
    const { container } = render(
      <>
        <Harness />
        <textarea data-testid="ta" />
      </>,
    );
    const ta = container.querySelector("textarea")!;
    ta.focus();
    fireEvent.keyDown(ta, { key: "a", target: ta });
    fireEvent.keyDown(ta, { key: "j", target: ta });
    expect(useOpsStore.getState().reviewStatus["w-ta"]).toBeUndefined();
  });

  it("Esc cascade: clears review focus when drawer closed and replay off", () => {
    seedDone("w-esc", "claude");
    useOpsStore.getState().setReviewFocus("w-esc");
    render(<Harness />);
    fireEvent.keyDown(window, { key: "Escape" });
    expect(useOpsStore.getState().reviewFocusedWorkerId).toBeNull();
  });

  it("Esc cascade: exits replay when no focus and drawer closed", () => {
    useOpsStore.getState().enterReplay([]);
    render(<Harness />);
    fireEvent.keyDown(window, { key: "Escape" });
    expect(useOpsStore.getState().replay.mode).toBe("live");
  });

  it("Esc cascade: leaves drawer Esc to the drawer when drawer is open", () => {
    seedDone("w-d", "claude");
    useOpsStore.getState().selectWorker("w-d");
    useOpsStore.getState().setReviewFocus("w-d");
    render(<Harness />);
    fireEvent.keyDown(window, { key: "Escape" });
    // Global handler did NOT clear focus (drawer's own Esc will close
    // the drawer; that's not under test here).
    expect(useOpsStore.getState().reviewFocusedWorkerId).toBe("w-d");
  });

  it("J/K with drawer open moves both queue focus and drawer selection", () => {
    seedDone("w-a", "claude");
    seedDone("w-b", "claude");
    useOpsStore.getState().setReviewFocus("w-a");
    useOpsStore.getState().selectWorker("w-a");
    render(<Harness />);
    fireEvent.keyDown(window, { key: "j" });
    expect(useOpsStore.getState().reviewFocusedWorkerId).toBe("w-b");
    expect(useOpsStore.getState().selectedWorkerId).toBe("w-b");
  });

  it("accept does NOT auto-advance the drawer", () => {
    seedDone("w-a", "claude");
    seedDone("w-b", "claude");
    useOpsStore.getState().setReviewFocus("w-a");
    useOpsStore.getState().selectWorker("w-a");
    render(<Harness />);
    fireEvent.keyDown(window, { key: "a" });
    // Drawer must stay on w-a even though the queue focus auto-moved.
    expect(useOpsStore.getState().selectedWorkerId).toBe("w-a");
  });
});
