import { describe, it, expect, beforeEach, vi } from "vitest";
import { fireEvent, render, screen, waitFor } from "@testing-library/react";
import { useOpsStore } from "../store";
import CommandPanel from "../command-panel/CommandPanel";
import { commands } from "../bindings";

vi.mock("../bindings", () => {
  return {
    commands: {
      healthCheck: vi.fn().mockResolvedValue({
        ok: true,
        uptime_ms: 0,
        version: "0.0.1-test",
      }),
      // PinNoteButton renders inside CommandPanel; stub the command
      // so the integrated test doesn't fall through to a real
      // tauri-specta invoke (which would throw in the test renderer).
      memoryPinNote: vi.fn().mockResolvedValue({
        status: "ok",
        data: { outcome: "pinned", note_id: "test-id", promoted: true },
      }),
      // MemoryDrawerButton mounts the drawer when toggled. In the
      // command-panel tests the button stays collapsed, but the
      // import surface must still satisfy the type checker — stub
      // the read commands with shape-correct ok responses so any
      // accidental mount in a future test renders cleanly.
      memoryListNotes: vi.fn().mockResolvedValue({ status: "ok", data: [] }),
      memoryRecentEventsForMission: vi
        .fn()
        .mockResolvedValue({ status: "ok", data: [] }),
      memoryLatestBundleForMission: vi
        .fn()
        .mockResolvedValue({ status: "ok", data: null }),
    },
  };
});

function seedDone(workerId: string, vendor: "claude" | "codex" | "gemini" | "mock" = "claude") {
  useOpsStore.getState().registerWorker(workerId, vendor, "task");
  useOpsStore.setState((prev) => ({
    workers: {
      ...prev.workers,
      [workerId]: { ...prev.workers[workerId], state: "done" },
    },
    // P3 — bypassing applyEvent's state_change handler, so
    // maintain the derived counters manually.
    activeCount: prev.activeCount - 1,
    needsInputCount: prev.needsInputCount + 1,
  }));
}

describe("Batch 3 — CommandPanel needs-input counter", () => {
  beforeEach(() => {
    vi.clearAllMocks();
    useOpsStore.getState().reset();
  });

  it("hides needs-input chip when count is 0", () => {
    render(<CommandPanel />);
    expect(screen.queryByText(/needs input/i)).toBeNull();
  });

  it("renders ⚠ N needs input when there are needs-review workers", () => {
    seedDone("w1");
    seedDone("w2");
    render(<CommandPanel />);
    expect(screen.getByText(/⚠ 2 needs input/)).toBeInTheDocument();
  });

  it("accepting one reduces the count to 1", async () => {
    seedDone("w1");
    seedDone("w2");
    render(<CommandPanel />);
    expect(screen.getByText(/⚠ 2 needs input/)).toBeInTheDocument();
    useOpsStore.getState().setReviewStatus("w1", "accepted");
    await waitFor(() => {
      expect(screen.getByText(/⚠ 1 needs input/)).toBeInTheDocument();
    });
  });

  it("hides chip after all workers accepted", async () => {
    seedDone("w1");
    render(<CommandPanel />);
    expect(screen.getByText(/⚠ 1 needs input/)).toBeInTheDocument();
    useOpsStore.getState().setReviewStatus("w1", "accepted");
    await waitFor(() => {
      expect(screen.queryByText(/needs input/i)).toBeNull();
    });
  });

  it("clicking the chip sets reviewFocusedWorkerId to the first queue worker", () => {
    seedDone("w-first");
    seedDone("w-second");
    render(<CommandPanel />);
    fireEvent.click(screen.getByText(/needs input/i));
    // workerOrder is insertion order, so first is w-first.
    expect(useOpsStore.getState().reviewFocusedWorkerId).toBe("w-first");
  });

  it("counter chips render without hud-corners framing (reserved for hero panels)", () => {
    render(<CommandPanel />);
    const chips = document.querySelectorAll(".command-panel .meta");
    expect(chips.length).toBeGreaterThanOrEqual(4);
    chips.forEach((chip) => expect(chip).not.toHaveClass("hud-corners"));
  });
});

it("renders a rotating reticle glyph before the brand wordmark", () => {
  render(<CommandPanel />);
  const reticle = document.querySelector(".command-panel-reticle");
  expect(reticle).not.toBeNull();
  expect(reticle?.tagName.toLowerCase()).toBe("svg");
});

describe("P2-14 — CommandPanel uptime/version tri-state", () => {
  const healthCheckMock = commands.healthCheck as unknown as ReturnType<
    typeof vi.fn
  >;

  beforeEach(() => {
    vi.clearAllMocks();
    useOpsStore.getState().reset();
    // Default — the inherited resolved mock from the outer vi.mock
    // call is fine, but clearAllMocks wipes the implementation, so
    // restore the success default per-test below if needed.
    healthCheckMock.mockReset();
  });

  function uptimeChip(): HTMLElement {
    const chips = document.querySelectorAll(".command-panel .meta");
    const chip = Array.from(chips).find((c) =>
      c.textContent?.includes("uptime"),
    ) as HTMLElement | undefined;
    if (!chip) throw new Error("uptime chip not found");
    return chip;
  }

  function versionChip(): HTMLElement {
    const chip = document.querySelector(
      ".command-panel-version",
    ) as HTMLElement | null;
    if (!chip) throw new Error("version chip not found");
    return chip;
  }

  it("uptime renders a hud-skeleton (no em-dash) while health is pending", () => {
    // Never-resolving promise — health stays null, no error set.
    healthCheckMock.mockReturnValue(new Promise(() => {}));
    render(<CommandPanel />);
    const chip = uptimeChip();
    expect(chip.querySelector(".hud-skeleton")).not.toBeNull();
    expect(chip.textContent).not.toContain("—");
  });

  it("uptime renders 00:00:00 (not em-dash) when health resolves with uptime_ms: 0", async () => {
    healthCheckMock.mockResolvedValue({
      ok: true,
      uptime_ms: 0,
      version: "0.0.1-test",
    });
    render(<CommandPanel />);
    await waitFor(() => {
      expect(uptimeChip().textContent).toMatch(/00:00:00/);
    });
    const chip = uptimeChip();
    expect(chip.querySelector(".hud-skeleton")).toBeNull();
    expect(chip.textContent).not.toContain("—");
  });

  it("version renders a hud-skeleton between 'v' and the end while health is pending", () => {
    healthCheckMock.mockReturnValue(new Promise(() => {}));
    render(<CommandPanel />);
    const chip = versionChip();
    expect(chip.querySelector(".hud-skeleton")).not.toBeNull();
    // 'v' prefix is still rendered, em-dash must not appear.
    expect(chip.textContent).toContain("v");
    expect(chip.textContent).not.toContain("—");
  });

  it("version chip shows faint italic 'n/a' with the error in title when healthCheck rejects", async () => {
    healthCheckMock.mockRejectedValue(new Error("IPC down"));
    render(<CommandPanel />);
    await waitFor(() => {
      const chip = versionChip();
      expect(chip.textContent).toMatch(/n\/a/);
    });
    const chip = versionChip();
    const faint = chip.querySelector(".mission-review__faint");
    expect(faint).not.toBeNull();
    expect(faint!.textContent).toBe("n/a");
    expect(chip.getAttribute("title")).toMatch(/IPC down/);
    expect(chip.textContent).not.toContain("—");
  });
});

describe("P1-12 — channel toggle uses plain labels", () => {
  beforeEach(() => {
    vi.clearAllMocks();
    useOpsStore.getState().reset();
  });

  it("default state shows 'Live' label, not 'CH-01 LIVE'", () => {
    render(<CommandPanel />);
    const button = document.querySelector(".command-panel-channel") as HTMLElement;
    expect(button).not.toBeNull();
    const label = button.querySelector(".command-panel-channel__label");
    expect(label?.textContent).toBe("Live");
    expect(screen.queryByText("CH-01 LIVE")).toBeNull();
    expect(screen.queryByText("CH-02 REPLAY")).toBeNull();
  });

  it("replay state shows 'History mode' label", async () => {
    render(<CommandPanel />);
    useOpsStore.getState().enterReplay([]);
    await waitFor(() => {
      const label = document.querySelector(".command-panel-channel__label");
      expect(label?.textContent).toBe("History mode");
    });
    expect(screen.queryByText("CH-01 LIVE")).toBeNull();
    expect(screen.queryByText("CH-02 REPLAY")).toBeNull();
  });

  it("aria-label updates between live and history states", async () => {
    render(<CommandPanel />);
    expect(
      document
        .querySelector(".command-panel-channel")
        ?.getAttribute("aria-label")
        ?.toLowerCase(),
    ).toContain("live");
    useOpsStore.getState().enterReplay([]);
    await waitFor(() => {
      expect(
        document
          .querySelector(".command-panel-channel")
          ?.getAttribute("aria-label")
          ?.toLowerCase(),
      ).toContain("history");
    });
  });
});
