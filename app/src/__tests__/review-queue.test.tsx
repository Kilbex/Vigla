import { describe, it, expect, beforeEach, vi } from "vitest";
import { act, fireEvent, render, screen, waitFor, within } from "@testing-library/react";
import { useOpsStore } from "../store";
import ReviewQueue from "../comms/ReviewQueue";

vi.mock("../bindings", () => {
  return {
    commands: {
      retryWorker: vi.fn(),
      continueWorker: vi.fn(),
    },
  };
});

import { commands } from "../bindings";

function seedDone(workerId: string, vendor: "claude" | "codex" | "gemini" | "mock" = "claude") {
  useOpsStore.getState().registerWorker(workerId, vendor, "test task");
  useOpsStore.setState((prev) => ({
    workers: {
      ...prev.workers,
      [workerId]: { ...prev.workers[workerId], state: "done" },
    },
  }));
}

function deferred<T>() {
  let resolve!: (value: T) => void;
  let reject!: (reason?: unknown) => void;
  const promise = new Promise<T>((res, rej) => {
    resolve = res;
    reject = rej;
  });
  return { promise, resolve, reject };
}

describe("Batch 3 — Review Queue actionable cards", () => {
  beforeEach(() => {
    vi.clearAllMocks();
    useOpsStore.getState().reset();
  });

  it("hides itself when no workers need review (U2 — escalation-only surface)", () => {
    const { container } = render(<ReviewQueue />);
    expect(container.firstChild).toBeNull();
    expect(screen.queryByText("Review Queue")).toBeNull();
  });

  it("lists done/failed workers with 5 action buttons each", () => {
    seedDone("w-done", "claude");
    useOpsStore.getState().registerWorker("w-failed", "claude", "task");
    useOpsStore.setState((prev) => ({
      workers: {
        ...prev.workers,
        "w-failed": { ...prev.workers["w-failed"], state: "failed" },
      },
    }));

    render(<ReviewQueue />);
    expect(screen.getByText(/Review Queue \(2\)/)).toBeInTheDocument();
    expect(screen.getAllByRole("button", { name: /open drawer/i })).toHaveLength(2);
    expect(screen.getAllByRole("button", { name: /^retry$/i })).toHaveLength(2);
    expect(screen.getAllByRole("button", { name: /^continue$/i })).toHaveLength(2);
    expect(screen.getAllByRole("button", { name: /^accept$/i })).toHaveLength(2);
    expect(screen.getAllByRole("button", { name: /^reject$/i })).toHaveLength(2);
  });

  it("excludes accepted/rejected workers from the queue (selector fix)", () => {
    seedDone("w-acc");
    seedDone("w-keep");
    useOpsStore.getState().setReviewStatus("w-acc", "accepted");
    render(<ReviewQueue />);
    expect(screen.getByText(/Review Queue \(1\)/)).toBeInTheDocument();
  });

  it("retry button is disabled for codex worker with M3 tooltip", () => {
    seedDone("w-codex", "codex");
    render(<ReviewQueue />);
    const btn = screen.getByRole("button", { name: /^retry$/i });
    expect(btn).toBeDisabled();
    expect(btn.getAttribute("title")).toMatch(/codex.*M3/);
  });

  it("continue button is disabled for gemini worker", () => {
    seedDone("w-gemini", "gemini");
    render(<ReviewQueue />);
    const btn = screen.getByRole("button", { name: /^continue$/i });
    expect(btn).toBeDisabled();
  });

  it("accept button mutates reviewStatus", () => {
    seedDone("w-a");
    render(<ReviewQueue />);
    fireEvent.click(screen.getByRole("button", { name: /^accept$/i }));
    expect(useOpsStore.getState().reviewStatus["w-a"]).toBe("accepted");
  });

  it("reject button mutates reviewStatus", () => {
    seedDone("w-r");
    render(<ReviewQueue />);
    fireEvent.click(screen.getByRole("button", { name: /^reject$/i }));
    expect(useOpsStore.getState().reviewStatus["w-r"]).toBe("rejected");
  });

  it("open button opens the drawer (selectedWorkerId)", () => {
    seedDone("w-o");
    render(<ReviewQueue />);
    fireEvent.click(screen.getByRole("button", { name: /open drawer/i }));
    expect(useOpsStore.getState().selectedWorkerId).toBe("w-o");
  });

  it("continue button expands inline textarea", () => {
    seedDone("w-c", "claude");
    render(<ReviewQueue />);
    expect(screen.queryByLabelText(/continue follow-up prompt/i)).toBeNull();
    fireEvent.click(screen.getByRole("button", { name: /^continue$/i }));
    expect(screen.getByLabelText(/continue follow-up prompt/i)).toBeInTheDocument();
  });

  it("submitting inline textarea calls commands.continueWorker", async () => {
    seedDone("w-s", "claude");
    (commands.continueWorker as any).mockResolvedValue({ status: "ok", data: null });

    render(<ReviewQueue />);
    fireEvent.click(screen.getByRole("button", { name: /^continue$/i }));
    const input = screen.getByLabelText(/continue follow-up prompt/i) as HTMLTextAreaElement;
    fireEvent.change(input, { target: { value: "do more" } });
    fireEvent.click(screen.getByText(/send →/));

    await waitFor(() => {
      expect(commands.continueWorker).toHaveBeenCalledWith("w-s", "do more");
    });
  });

  it("does not leak typed continue text across cards (per-card state)", () => {
    seedDone("w-A", "claude");
    seedDone("w-B", "claude");
    const { container } = render(<ReviewQueue />);

    const cardA = container.querySelector('[data-worker-id="w-A"]') as HTMLElement;
    const cardB = container.querySelector('[data-worker-id="w-B"]') as HTMLElement;

    // Open continue on A and type a prompt meant for worker A.
    fireEvent.click(within(cardA).getByRole("button", { name: /^continue$/i }));
    const inputA = within(cardA).getByLabelText(
      /continue follow-up prompt/i,
    ) as HTMLTextAreaElement;
    fireEvent.change(inputA, { target: { value: "prompt for A" } });

    // Switch to B's continue without cancelling A first.
    fireEvent.click(within(cardB).getByRole("button", { name: /^continue$/i }));
    const inputB = within(cardB).getByLabelText(
      /continue follow-up prompt/i,
    ) as HTMLTextAreaElement;

    // B's textarea must start empty — A's text must not bleed into B,
    // or hitting "send" would route A's prompt to worker B.
    expect(inputB.value).toBe("");
  });

  it("surfaces continue error inline; textarea stays open", async () => {
    seedDone("w-ce", "claude");
    (commands.continueWorker as any).mockResolvedValue({
      status: "error",
      error: "session_id_missing",
    });

    render(<ReviewQueue />);
    fireEvent.click(screen.getByRole("button", { name: /^continue$/i }));
    const input = screen.getByLabelText(/continue follow-up prompt/i);
    fireEvent.change(input, { target: { value: "x" } });
    fireEvent.click(screen.getByText(/send →/));

    await waitFor(() =>
      expect(screen.getByRole("alert")).toHaveTextContent(/continue failed/i),
    );
    expect(screen.getByLabelText(/continue follow-up prompt/i)).toBeInTheDocument();
  });

  it("keeps a pending continue completion scoped to its worker", async () => {
    seedDone("w-A", "claude");
    seedDone("w-B", "claude");
    const requestA = deferred<{ status: "ok"; data: null }>();
    (commands.continueWorker as any).mockReturnValueOnce(requestA.promise);
    const { container } = render(<ReviewQueue />);
    const cardA = container.querySelector('[data-worker-id="w-A"]') as HTMLElement;
    const cardB = container.querySelector('[data-worker-id="w-B"]') as HTMLElement;

    fireEvent.click(within(cardA).getByRole("button", { name: /^continue$/i }));
    fireEvent.change(
      within(cardA).getByLabelText(/continue follow-up prompt/i),
      { target: { value: "prompt for A" } },
    );
    fireEvent.click(within(cardA).getByText(/send →/i));

    fireEvent.click(within(cardB).getByRole("button", { name: /^continue$/i }));
    const inputB = within(cardB).getByLabelText(
      /continue follow-up prompt/i,
    ) as HTMLTextAreaElement;
    expect(inputB).toBeEnabled();
    fireEvent.change(inputB, { target: { value: "draft for B" } });

    await act(async () => {
      requestA.reject(new Error("late failure from A"));
      await Promise.resolve();
    });

    expect(within(cardB).queryByRole("alert")).toBeNull();
    expect(inputB).toHaveValue("draft for B");
    expect(inputB).toBeEnabled();

    fireEvent.click(within(cardA).getByRole("button", { name: /^continue$/i }));
    expect(within(cardA).getByRole("alert")).toHaveTextContent(
      "continue failed: late failure from A",
    );
    expect(
      within(cardA).getByLabelText(/continue follow-up prompt/i),
    ).toHaveValue("prompt for A");
  });

  it("does not collapse another worker's draft after a stale success", async () => {
    seedDone("w-success-A", "claude");
    seedDone("w-success-B", "claude");
    const requestA = deferred<{ status: "ok"; data: null }>();
    (commands.continueWorker as any).mockReturnValueOnce(requestA.promise);
    const { container } = render(<ReviewQueue />);
    const cardA = container.querySelector(
      '[data-worker-id="w-success-A"]',
    ) as HTMLElement;
    const cardB = container.querySelector(
      '[data-worker-id="w-success-B"]',
    ) as HTMLElement;

    fireEvent.click(within(cardA).getByRole("button", { name: /^continue$/i }));
    fireEvent.change(
      within(cardA).getByLabelText(/continue follow-up prompt/i),
      { target: { value: "send A" } },
    );
    fireEvent.click(within(cardA).getByText(/send →/i));

    fireEvent.click(within(cardB).getByRole("button", { name: /^continue$/i }));
    const inputB = within(cardB).getByLabelText(
      /continue follow-up prompt/i,
    );
    fireEvent.change(inputB, { target: { value: "keep B open" } });

    await act(async () => {
      requestA.resolve({ status: "ok", data: null });
      await requestA.promise;
    });

    expect(
      within(cardB).getByLabelText(/continue follow-up prompt/i),
    ).toHaveValue("keep B open");
  });

  it("re-enables continue controls after a rejected IPC promise (does not brick)", async () => {
    // Regression: the binding re-throws on an Error-instance rejection
    // (Rust panic / IPC failure). submitContinue must catch it so
    // continueBusy resets — otherwise the textarea + send + cancel freeze
    // until app reload.
    seedDone("w-rej", "claude");
    (commands.continueWorker as any).mockRejectedValue(
      new Error("ipc transport failed"),
    );

    render(<ReviewQueue />);
    fireEvent.click(screen.getByRole("button", { name: /^continue$/i }));
    const input = screen.getByLabelText(
      /continue follow-up prompt/i,
    ) as HTMLTextAreaElement;
    fireEvent.change(input, { target: { value: "x" } });
    fireEvent.click(screen.getByText(/send →/));

    await waitFor(() =>
      expect(screen.getByRole("alert")).toHaveTextContent(/continue failed/i),
    );
    // The controls must be usable again, not stuck disabled.
    expect(
      screen.getByLabelText(/continue follow-up prompt/i),
    ).not.toBeDisabled();
  });

  it("guards retry while pending and recovers from a rejected IPC promise", async () => {
    seedDone("w-retry-reject", "claude");
    (commands.retryWorker as any).mockRejectedValueOnce(
      new Error("retry IPC unavailable"),
    );

    render(<ReviewQueue />);
    const retryButton = screen.getByRole("button", { name: /^retry$/i });
    fireEvent.click(retryButton);
    expect(retryButton).toBeDisabled();
    fireEvent.click(retryButton);
    expect(commands.retryWorker).toHaveBeenCalledTimes(1);

    await waitFor(() => {
      expect(screen.getByRole("alert")).toHaveTextContent(
        "retry failed: retry IPC unavailable",
      );
    });
    expect(retryButton).toBeEnabled();
  });

  it("Esc inside textarea collapses the inline area", () => {
    seedDone("w-esc", "claude");
    render(<ReviewQueue />);
    fireEvent.click(screen.getByRole("button", { name: /^continue$/i }));
    const input = screen.getByLabelText(/continue follow-up prompt/i);
    fireEvent.keyDown(input, { key: "Escape" });
    expect(screen.queryByLabelText(/continue follow-up prompt/i)).toBeNull();
  });

  it("click on card sets reviewFocusedWorkerId without opening drawer", () => {
    seedDone("w-f", "claude");
    render(<ReviewQueue />);
    fireEvent.click(screen.getByLabelText(/review card/i));
    expect(useOpsStore.getState().reviewFocusedWorkerId).toBe("w-f");
    expect(useOpsStore.getState().selectedWorkerId).toBeNull();
  });

  it("focused card has the focused class", () => {
    seedDone("w-f2", "claude");
    useOpsStore.getState().setReviewFocus("w-f2");
    render(<ReviewQueue />);
    expect(screen.getByLabelText(/review card/i).className).toMatch(/review-queue-item--focused/);
  });
});

describe("Batch 2 — Review Status (preserved)", () => {
  beforeEach(() => {
    vi.clearAllMocks();
    useOpsStore.getState().reset();
  });

  it("stores review status per worker", () => {
    useOpsStore.getState().registerWorker("w1", "mock", "task");
    useOpsStore.getState().registerWorker("w2", "mock", "task");
    useOpsStore.getState().setReviewStatus("w1", "accepted");
    useOpsStore.getState().setReviewStatus("w2", "rejected");
    expect(useOpsStore.getState().getReviewStatus("w1")).toBe("accepted");
    expect(useOpsStore.getState().getReviewStatus("w2")).toBe("rejected");
  });

  it("supports all review status values", () => {
    useOpsStore.getState().registerWorker("w", "mock", "task");
    const statuses: Array<"needs_review" | "accepted" | "rejected" | "parked"> = [
      "needs_review",
      "accepted",
      "rejected",
      "parked",
    ];
    for (const status of statuses) {
      useOpsStore.getState().setReviewStatus("w", status);
      expect(useOpsStore.getState().getReviewStatus("w")).toBe(status);
    }
  });

  it("returns undefined for unset review status", () => {
    useOpsStore.getState().registerWorker("w", "mock", "task");
    expect(useOpsStore.getState().getReviewStatus("w")).toBeUndefined();
  });

  it("survives replay mode", () => {
    useOpsStore.getState().registerWorker("w-replay", "mock", "task");
    useOpsStore.getState().setReviewStatus("w-replay", "accepted");
    useOpsStore.getState().enterReplay([]);
    useOpsStore.getState().setReviewStatus("w-replay", "rejected");
    useOpsStore.getState().exitReplay();
    expect(useOpsStore.getState().getReviewStatus("w-replay")).toBe("rejected");
  });
});
