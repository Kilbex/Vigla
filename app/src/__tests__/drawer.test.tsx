import { describe, it, expect, vi, beforeEach } from "vitest";
import { fireEvent, render, screen, waitFor } from "@testing-library/react";
import Drawer from "../drawer/Drawer";
import ReviewQueue from "../comms/ReviewQueue";

vi.mock("../bindings", () => {
  return {
    commands: {
      stopWorker: vi.fn(),
      retryWorker: vi.fn(),
      continueWorker: vi.fn(),
      getWorkerInfo: vi.fn(),
      switchWorkerModel: vi.fn(),
    },
  };
});

import { commands } from "../bindings";
import { useOpsStore } from "../store";

beforeEach(() => {
  vi.clearAllMocks();
  (commands.getWorkerInfo as any).mockResolvedValue({
    status: "error",
    error: "not found",
  });
  useOpsStore.getState().reset();
});

function seedWorker(workerId: string) {
  // Insert a synthetic worker via the store actions so Drawer renders.
  useOpsStore.getState().registerWorker(workerId, "claude", "do the thing");
  // Mark as still-running so the stop button renders.
  useOpsStore.setState((prev) => ({
    workers: {
      ...prev.workers,
      [workerId]: { ...prev.workers[workerId], state: "executing" },
    },
  }));
  useOpsStore.getState().selectWorker(workerId);
}

describe("Drawer stop", () => {
  it("surfaces stop IPC errors inline", async () => {
    seedWorker("w-stop-fail");
    (commands.stopWorker as any).mockResolvedValue({
      status: "error",
      error: "supervisor: child already exited",
    });

    render(<Drawer />);
    fireEvent.click(screen.getByRole("button", { name: /^stop$/i }));

    await waitFor(() => {
      expect(screen.getByRole("alert")).toHaveTextContent(
        "supervisor: child already exited",
      );
    });
    // Button must re-enable after the error.
    expect(screen.getByRole("button", { name: /^stop$/i })).not.toBeDisabled();
  });

  it("audit-r5: drawer-head structure has title+actions in a sub-row, with squad assignment as a separate full-width row", () => {
    seedWorker("w-layout");
    render(<Drawer />);
    // drawer-head is now flex-direction: column with three potential
    // child rows: drawer-head-row (title + actions), optional
    // stopError, and drawer-squad. Verify the structural pieces.
    const head = document.querySelector(".drawer-head");
    expect(head).not.toBeNull();
    // The first child is the head-row containing title + actions.
    const headRow = head?.querySelector(".drawer-head-row");
    expect(headRow).not.toBeNull();
    expect(headRow?.querySelector(".drawer-title")).not.toBeNull();
    expect(headRow?.querySelector(".drawer-actions")).not.toBeNull();
    // The squad row is a SIBLING of the head-row, not nested inside.
    const squadRow = head?.querySelector(".drawer-squad");
    expect(squadRow).not.toBeNull();
    expect(squadRow?.parentElement).toBe(head);
    // drawer-title is NOT the parent of drawer-squad.
    expect(headRow?.querySelector(".drawer-squad")).toBeNull();
  });

  it("shows the current model and saves it for the next continuation", async () => {
    useOpsStore.getState().registerWorker(
      "w-model",
      "claude",
      "do the thing",
      "claude-sonnet-4-5",
    );
    useOpsStore.getState().selectWorker("w-model");
    (commands.switchWorkerModel as any).mockResolvedValue({
      status: "ok",
      data: {
        worker_id: "w-model",
        model: "claude-opus-4-7",
        detail: "Saved claude-opus-4-7 for this worker's next continuation.",
      },
    });

    render(<Drawer />);
    expect(screen.getByText("claude-sonnet-4-5")).toBeInTheDocument();

    const input = screen.getByLabelText(/model name/i);
    fireEvent.change(input, { target: { value: "claude-opus-4-7" } });
    fireEvent.click(screen.getByRole("button", { name: /^use next$/i }));

    await waitFor(() =>
      expect(commands.switchWorkerModel).toHaveBeenCalledWith(
        "w-model",
        "claude-opus-4-7",
      ),
    );
    await waitFor(() =>
      expect(screen.getByText(/saved claude-opus-4-7.*next continuation/i)).toBeInTheDocument(),
    );
  });

  it("lands on the result tab and shows completionSummary for a done worker", () => {
    useOpsStore.getState().registerWorker("w-done", "claude", "do the thing");
    useOpsStore.setState((prev) => ({
      workers: {
        ...prev.workers,
        "w-done": {
          ...prev.workers["w-done"],
          state: "done",
          completionSummary: "found two real bugs and one small guardrail",
        },
      },
    }));
    useOpsStore.getState().selectWorker("w-done");

    render(<Drawer />);
    const resultTab = screen.getByRole("tab", { name: "result" });
    expect(resultTab).toHaveAttribute("aria-selected", "true");
    expect(
      screen.getByLabelText("completion summary"),
    ).toHaveTextContent("found two real bugs and one small guardrail");
  });

  it("lands on the feed tab for an executing worker (no result default)", () => {
    seedWorker("w-running"); // seedWorker leaves state=executing
    render(<Drawer />);
    expect(screen.getByRole("tab", { name: "feed" })).toHaveAttribute(
      "aria-selected",
      "true",
    );
  });

  it("shows failureSummary with the failed tint when worker failed", () => {
    useOpsStore.getState().registerWorker("w-fail", "codex", "do thing");
    useOpsStore.setState((prev) => ({
      workers: {
        ...prev.workers,
        "w-fail": {
          ...prev.workers["w-fail"],
          state: "failed",
          failureSummary: "exit code 1: panicked at lib.rs:42",
        },
      },
    }));
    useOpsStore.getState().selectWorker("w-fail");
    render(<Drawer />);
    const block = screen.getByLabelText("failure summary");
    expect(block).toHaveTextContent("exit code 1: panicked at lib.rs:42");
    expect(block.className).toMatch(/drawer-result-summary--failed/);
  });

  it("shows the placeholder when result tab has neither summary nor logs", () => {
    useOpsStore.getState().registerWorker("w-empty", "gemini", "do thing");
    useOpsStore.setState((prev) => ({
      workers: {
        ...prev.workers,
        "w-empty": { ...prev.workers["w-empty"], state: "done" },
      },
    }));
    useOpsStore.getState().selectWorker("w-empty");
    render(<Drawer />);
    expect(
      screen.getByText(/no result text captured/i),
    ).toBeInTheDocument();
  });

  it("clears the stop error when the user switches workers", async () => {
    seedWorker("w-1");
    (commands.stopWorker as any).mockResolvedValue({
      status: "error",
      error: "boom",
    });

    render(<Drawer />);
    fireEvent.click(screen.getByRole("button", { name: /^stop$/i }));
    await waitFor(() =>
      expect(screen.getByRole("alert")).toHaveTextContent("boom"),
    );

    // Now seed and select a different worker.
    seedWorker("w-2");
    await waitFor(() => {
      expect(screen.queryByRole("alert")).toBeNull();
    });
  });
});

describe("Drawer retry (Step 25)", () => {
  it("shows retry button only for done/failed workers", () => {
    useOpsStore.getState().registerWorker("w-done", "claude", "do the thing");
    useOpsStore.setState((prev) => ({
      workers: {
        ...prev.workers,
        "w-done": { ...prev.workers["w-done"], state: "done" },
      },
    }));
    useOpsStore.getState().selectWorker("w-done");

    render(<Drawer />);
    expect(screen.getByRole("button", { name: /^retry$/i })).toBeInTheDocument();
  });

  it("does not show retry button for executing workers", () => {
    seedWorker("w-executing"); // seedWorker sets state to executing
    render(<Drawer />);
    expect(screen.queryByRole("button", { name: /^retry$/i })).toBeNull();
  });

  it("calls retryWorker and surfaces errors", async () => {
    useOpsStore.getState().registerWorker("w-retry-fail", "claude", "do the thing");
    useOpsStore.setState((prev) => ({
      workers: {
        ...prev.workers,
        "w-retry-fail": { ...prev.workers["w-retry-fail"], state: "failed" },
      },
    }));
    useOpsStore.getState().selectWorker("w-retry-fail");
    (commands.retryWorker as any).mockResolvedValue({
      status: "error",
      error: "resume not supported for codex",
    });

    render(<Drawer />);
    fireEvent.click(screen.getByRole("button", { name: /^retry$/i }));

    await waitFor(() => {
      expect(screen.getByRole("alert")).toHaveTextContent(
        "retry failed: resume not supported",
      );
    });
  });
});

describe("Drawer follow-up input (Step 25)", () => {
  it("shows follow-up input on result and feed tabs", () => {
    useOpsStore.getState().registerWorker("w-followup", "claude", "do the thing");
    useOpsStore.setState((prev) => ({
      workers: {
        ...prev.workers,
        "w-followup": { ...prev.workers["w-followup"], state: "done" },
      },
    }));
    useOpsStore.getState().selectWorker("w-followup");

    render(<Drawer />);
    expect(screen.getByPlaceholderText(/Send a follow-up prompt/i)).toBeInTheDocument();
  });

  it("disables follow-up input while worker is executing", () => {
    seedWorker("w-busy");
    render(<Drawer />);
    const input = screen.getByPlaceholderText(/worker is busy/i) as HTMLTextAreaElement;
    expect(input).toBeDisabled();
  });

  it("calls continueWorker and clears input on success", async () => {
    useOpsStore.getState().registerWorker("w-continue", "claude", "do the thing");
    useOpsStore.setState((prev) => ({
      workers: {
        ...prev.workers,
        "w-continue": { ...prev.workers["w-continue"], state: "done" },
      },
    }));
    useOpsStore.getState().selectWorker("w-continue");
    (commands.continueWorker as any).mockResolvedValue({ status: "ok", data: null });

    render(<Drawer />);
    const input = screen.getByPlaceholderText(/Send a follow-up prompt/i) as HTMLTextAreaElement;
    fireEvent.change(input, { target: { value: "continue with..." } });
    fireEvent.click(screen.getByRole("button", { name: /Send follow-up/i }));

    await waitFor(() => {
      expect(commands.continueWorker).toHaveBeenCalledWith("w-continue", "continue with...");
      expect(input.value).toBe("");
    });
  });

  it("surfaces continueWorker errors", async () => {
    useOpsStore.getState().registerWorker("w-continue-fail", "claude", "do the thing");
    useOpsStore.setState((prev) => ({
      workers: {
        ...prev.workers,
        "w-continue-fail": { ...prev.workers["w-continue-fail"], state: "done" },
      },
    }));
    useOpsStore.getState().selectWorker("w-continue-fail");
    (commands.continueWorker as any).mockResolvedValue({
      status: "error",
      error: "session_id_missing",
    });

    render(<Drawer />);
    const input = screen.getByPlaceholderText(/Send a follow-up prompt/i);
    fireEvent.change(input, { target: { value: "continue" } });
    fireEvent.click(screen.getByRole("button", { name: /Send follow-up/i }));

    await waitFor(() => {
      expect(screen.getByRole("alert")).toHaveTextContent("session_id_missing");
    });
  });
});

describe("Batch 3 — Drawer/queue reciprocity", () => {
  beforeEach(() => {
    vi.clearAllMocks();
    useOpsStore.getState().reset();
  });

  it("accepting from the drawer updates the queue card status pill", () => {
    useOpsStore.getState().registerWorker("w-rec", "claude", "task");
    useOpsStore.setState((prev) => ({
      workers: {
        ...prev.workers,
        "w-rec": { ...prev.workers["w-rec"], state: "done" },
      },
    }));
    useOpsStore.getState().selectWorker("w-rec");

    render(
      <>
        <ReviewQueue />
        <Drawer />
      </>,
    );

    // Click the drawer's accepted review-status button.
    const accBtn = screen.getByTitle(/mark as accepted/i);
    fireEvent.click(accBtn);

    // The reviewStatus must transition; the queue card drops out
    // (the corrected selector excludes accepted workers). After U2
    // the Review Queue hides itself entirely when nothing needs
    // attention, so the "Review Queue" title goes away too.
    expect(useOpsStore.getState().reviewStatus["w-rec"]).toBe("accepted");
    expect(screen.queryByText(/Review Queue/i)).toBeNull();
  });

  it("accepting from drawer does NOT auto-advance the drawer", () => {
    useOpsStore.getState().registerWorker("w-a", "claude", "task");
    useOpsStore.getState().registerWorker("w-b", "claude", "task");
    useOpsStore.setState((prev) => ({
      workers: {
        ...prev.workers,
        "w-a": { ...prev.workers["w-a"], state: "done" },
        "w-b": { ...prev.workers["w-b"], state: "done" },
      },
    }));
    useOpsStore.getState().selectWorker("w-a");

    render(<Drawer />);
    fireEvent.click(screen.getByTitle(/mark as accepted/i));
    // Drawer stays on w-a even though w-a left the review queue.
    expect(useOpsStore.getState().selectedWorkerId).toBe("w-a");
  });
});
