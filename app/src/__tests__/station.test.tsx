import { describe, it, expect, beforeEach, vi } from "vitest";
import { act, render, screen, waitFor } from "@testing-library/react";
import { ReactFlowProvider } from "@xyflow/react";
import Station, { type StationData } from "../operations/Station";
import type { Vendor, WorkerState } from "../bindings";
import type { WorkerSnapshot } from "../store/types";
import { useOpsStore } from "../store";
import { __resetWorkerIdentityCache } from "../operations/useWorkerIdentity";

// Identity overlay (P1-6) reads through `commands.getWorkerInfo`.
// Mock the bindings module so each test controls the IPC outcome.
vi.mock("../bindings", () => {
  return {
    commands: {
      getWorkerInfo: vi.fn(),
    },
  };
});

import { commands } from "../bindings";

function snapshot(over: Partial<WorkerSnapshot> = {}): WorkerSnapshot {
  return {
    id: "01HFAKE",
    shortId: "01HFAKE",
    vendor: "claude",
    model: null,
    state: "idle",
    spawnedAt: 0,
    currentTaskId: null,
    currentTaskTitle: null,
    progress: null,
    etaMs: null,
    progressNote: null,
    filesAdded: 0,
    filesModified: 0,
    filesDeleted: 0,
    linesAdded: 0,
    linesRemoved: 0,
    testsPassed: 0,
    testsFailed: 0,
    testsSkipped: 0,
    lastSuite: null,
    costUsd: 0,
    inputTokens: 0,
    outputTokens: 0,
    recentLog: null,
    blockedOn: null,
    failureSummary: null,
    completionSummary: null,
    flashUntil: 0,
    eventCount: 0,
    missionScoped: false,
    missionId: null,
    missionTimeline: [],
    ...over,
  };
}

function nodeProps(snap: WorkerSnapshot): { data: StationData; id: string; type: "station" } {
  return {
    id: snap.id,
    type: "station",
    data: snap as StationData,
  };
}

function renderStation(snap: WorkerSnapshot) {
  // React Flow's `Handle` requires a flow context — provider only.
  return render(
    <ReactFlowProvider>
      <Station {...(nodeProps(snap) as unknown as React.ComponentProps<typeof Station>)} />
    </ReactFlowProvider>,
  );
}

beforeEach(() => {
  useOpsStore.getState().reset();
  __resetWorkerIdentityCache();
  // Default: identity layer reports "missing" so tests fall back to
  // the snapshot vendor/model. Individual tests override per case.
  (commands.getWorkerInfo as unknown as ReturnType<typeof vi.fn>).mockResolvedValue({
    status: "ok",
    data: null,
  });
});

describe("Station — placeholder avatar", () => {
  it("renders a worker-avatar element with vendor + state metadata", () => {
    renderStation(snapshot({ vendor: "claude", state: "executing" }));
    const avatar = screen.getByTestId("worker-avatar");
    expect(avatar).toBeInTheDocument();
    expect(avatar).toHaveAttribute("data-vendor", "claude");
    expect(avatar).toHaveAttribute("data-state", "executing");
    expect(avatar).toHaveAttribute("data-role", "strategist");
  });

  it("uses role='img' with an accessible label including vendor + state", () => {
    renderStation(snapshot({ vendor: "codex", state: "blocked" }));
    const avatar = screen.getByRole("img", { name: /codex worker/i });
    expect(avatar).toHaveAccessibleName(/codex worker/i);
    expect(avatar).toHaveAccessibleName(/blocked/i);
  });

  it("applies the matching state-ring + animation class names", () => {
    renderStation(snapshot({ vendor: "gemini", state: "done" }));
    const avatar = screen.getByTestId("worker-avatar");
    expect(avatar.className).toMatch(/worker-avatar--state-done/);
    expect(avatar.className).toMatch(/worker-avatar--anim-lock-in/);
    expect(avatar.className).toMatch(/worker-avatar--vendor-gemini/);
  });

  it("renders the same DOM shape across every (vendor, state) combination", () => {
    // Stable layout is a hard requirement: vendor/state changes must
    // never reflow the station header. We approximate that by asserting
    // every combination renders the same number of avatar sub-elements.
    const vendors: Vendor[] = ["claude", "codex", "gemini", "opencode", "mock"];
    const states: WorkerState[] = [
      "idle", "planning", "executing", "blocked", "reviewing", "done", "failed",
    ];
    let referenceShape: number | null = null;
    for (const vendor of vendors) {
      for (const state of states) {
        const { unmount } = renderStation(snapshot({ vendor, state }));
        const avatar = screen.getByTestId("worker-avatar");
        const shape = avatar.children.length;
        if (referenceShape === null) referenceShape = shape;
        else expect(shape).toBe(referenceShape);
        // Always exactly one role band, ring, overlay, and one inner
        // content node (a glyph for placeholder vendors, a sprite for
        // portrait vendors). Either fills the slot — DOM shape is stable.
        expect(avatar.querySelector(".worker-avatar__ring")).not.toBeNull();
        expect(avatar.querySelector(".worker-avatar__role-band")).not.toBeNull();
        expect(avatar.querySelector(".worker-avatar__overlay")).not.toBeNull();
        const inner =
          avatar.querySelector(".worker-avatar__glyph") ??
          avatar.querySelector(".worker-avatar__sprite");
        expect(inner).not.toBeNull();
        unmount();
      }
    }
  });

  it("renders the current model when known", () => {
    renderStation(snapshot({ model: "claude-sonnet-4-5" }));
    expect(screen.getByText("claude-sonnet-4-5")).toBeInTheDocument();
  });

  it("falls back to '?' for an unknown future vendor without crashing", () => {
    renderStation(
      snapshot({ vendor: "future-vendor" as Vendor, state: "idle" }),
    );
    const avatar = screen.getByTestId("worker-avatar");
    expect(avatar).toHaveAttribute("data-vendor", "unknown");
    const glyph = avatar.querySelector(".worker-avatar__glyph");
    expect(glyph?.textContent).toBe("?");
  });
});

// ---------------------------------------------------------------------
// Step 19 — squad-color bar at the top of the station tile.
// ---------------------------------------------------------------------

describe("Station — squad bar (Step 19)", () => {
  it("renders the squad-bar element with no color when worker is unassigned", () => {
    const snap = snapshot({ id: "w-no-squad", vendor: "claude", state: "idle" });
    const { container } = renderStation(snap);
    const bar = container.querySelector(".station__squad-bar");
    expect(bar).not.toBeNull();
    // No color modifier class.
    expect(bar?.className).toBe("station__squad-bar");
    // aria-hidden when unassigned (decorative-only placeholder).
    expect(bar?.getAttribute("aria-hidden")).toBe("true");
  });

  it("applies the squad color modifier when the worker is assigned", () => {
    const id = useOpsStore.getState().createSquad("Frontend", "indigo");
    useOpsStore.getState().assignWorkerToSquad("w-assigned", id);

    const snap = snapshot({ id: "w-assigned", vendor: "codex", state: "executing" });
    const { container } = renderStation(snap);
    const bar = container.querySelector(".station__squad-bar");
    expect(bar?.className).toMatch(/station__squad-bar--indigo/);
    expect(bar?.getAttribute("title")).toBe("Squad: Frontend");
    expect(bar?.getAttribute("aria-label")).toBe("Squad Frontend");
  });

  it("updates color when the squad's color changes", () => {
    const id = useOpsStore.getState().createSquad("X", "indigo");
    useOpsStore.getState().assignWorkerToSquad("w-recolor", id);
    const snap = snapshot({ id: "w-recolor", vendor: "claude", state: "idle" });
    const { container } = renderStation(snap);
    expect(container.querySelector(".station__squad-bar")?.className).toMatch(
      /station__squad-bar--indigo/,
    );

    act(() => {
      useOpsStore.getState().setSquadColor(id, "coral");
    });
    expect(container.querySelector(".station__squad-bar")?.className).toMatch(
      /station__squad-bar--coral/,
    );
  });
});

// ---------------------------------------------------------------------
// Step 21 — squad lead chevron badge.
// ---------------------------------------------------------------------

describe("Station — lead badge (Step 21)", () => {
  it("does not render the lead badge when the worker is unassigned", () => {
    renderStation(snapshot({ id: "w-no-squad" }));
    expect(screen.queryByTestId("station-lead-badge")).toBeNull();
  });

  it("does not render the lead badge for a non-lead squad member", () => {
    const id = useOpsStore.getState().createSquad("X", "sage");
    useOpsStore.getState().assignWorkerToSquad("w-member", id);
    // No setSquadLead — w-member is not lead.
    renderStation(snapshot({ id: "w-member" }));
    expect(screen.queryByTestId("station-lead-badge")).toBeNull();
  });

  it("renders the chevron tinted by the squad color when worker is lead", () => {
    const id = useOpsStore.getState().createSquad("Frontend", "indigo");
    useOpsStore.getState().assignWorkerToSquad("w-lead", id);
    useOpsStore.getState().setSquadLead(id, "w-lead");

    renderStation(snapshot({ id: "w-lead" }));
    const badge = screen.getByTestId("station-lead-badge");
    expect(badge.className).toMatch(/station__lead-badge--indigo/);
    expect(badge.getAttribute("title")).toBe("Squad lead — Frontend");
    expect(badge.getAttribute("aria-label")).toBe("Squad lead of Frontend");
    expect(badge.textContent).toBe("▲");
  });

  it("removes the chevron when the lead designation is cleared", () => {
    const id = useOpsStore.getState().createSquad("X", "plum");
    useOpsStore.getState().assignWorkerToSquad("w-clear", id);
    useOpsStore.getState().setSquadLead(id, "w-clear");

    renderStation(snapshot({ id: "w-clear" }));
    expect(screen.getByTestId("station-lead-badge")).toBeInTheDocument();

    act(() => {
      useOpsStore.getState().setSquadLead(id, null);
    });
    expect(screen.queryByTestId("station-lead-badge")).toBeNull();
  });
});

/// Regression: most CLIs (and the `claude_happy` mock) stop emitting
/// progress events around 75% and signal completion via `state=done` /
/// `completion`. Without the terminal-state override the bar stays at
/// the last-reported value, leaving a "DONE 75%" frame on every
/// successful run.
describe("Station — terminal-state progress display", () => {
  it("renders 100% on a done worker even when last progress was 75", () => {
    renderStation(snapshot({ state: "done", progress: 75, etaMs: 900 }));
    expect(screen.getByText("100%")).toBeInTheDocument();
    expect(screen.queryByText(/eta/)).toBeNull();
  });

  it("renders 100% on a done worker that never emitted any progress", () => {
    renderStation(snapshot({ state: "done", progress: null, etaMs: null }));
    expect(screen.getByText("100%")).toBeInTheDocument();
  });

  it("preserves last-reported progress on a failed worker (no jump to 100)", () => {
    renderStation(snapshot({ state: "failed", progress: 35, etaMs: 5000 }));
    expect(screen.getByText("35%")).toBeInTheDocument();
    expect(screen.queryByText(/eta/)).toBeNull();
  });

  it("keeps mid-run progress unchanged for executing/reviewing workers", () => {
    const { unmount } = renderStation(
      snapshot({ state: "executing", progress: 25, etaMs: 2700 }),
    );
    expect(screen.getByText("25%")).toBeInTheDocument();
    expect(screen.getByText("3s eta")).toBeInTheDocument();
    unmount();

    renderStation(snapshot({ state: "reviewing", progress: 75, etaMs: 900 }));
    expect(screen.getByText("75%")).toBeInTheDocument();
    expect(screen.getByText("1s eta")).toBeInTheDocument();
  });
});

describe("Station — open-output affordance", () => {
  it("renders the open-output hint on a done worker", () => {
    renderStation(snapshot({ state: "done", completionSummary: "ok" }));
    expect(screen.getByText(/open full output/i)).toBeInTheDocument();
  });

  it("renders the open-output hint on a failed worker", () => {
    renderStation(snapshot({ state: "failed", failureSummary: "boom" }));
    expect(screen.getByText(/open full output/i)).toBeInTheDocument();
  });

  it("does not render the open-output hint while executing", () => {
    renderStation(snapshot({ state: "executing", progressNote: "step 2" }));
    expect(screen.queryByText(/open full output/i)).toBeNull();
  });
});

describe("Station — HUD polish (Task 4)", () => {
  it("station root has no hud-corners/hud-glow-edge framing (reserved for the canvas)", () => {
    renderStation(snapshot());
    const tile = screen.getByRole("button");
    expect(tile).not.toHaveClass("hud-corners");
    expect(tile).not.toHaveClass("hud-glow-edge");
  });
});

describe("Station — HUD polish (Task 5)", () => {
  it("adds hud-scanline only when state is executing", () => {
    const { rerender } = renderStation(snapshot({ state: "idle" }));
    expect(screen.getByRole("button")).not.toHaveClass("hud-scanline");

    rerender(
      <ReactFlowProvider>
        <Station
          {...(nodeProps(snapshot({ state: "executing" })) as unknown as React.ComponentProps<typeof Station>)}
        />
      </ReactFlowProvider>,
    );
    expect(screen.getByRole("button")).toHaveClass("hud-scanline");
  });
});

describe("Station — HUD polish (Task 6): chroma flash on select", () => {
  it("callsign does not get hud-chroma on click (chroma effect retired)", () => {
    const { container } = renderStation(snapshot());
    const tile = screen.getByRole("button");
    const callsign = container.querySelector(".station__callsign") as HTMLElement;

    // The chromatic-aberration flash was retired in the premium-tactical
    // redesign — clicking selects the worker without any chroma class.
    expect(callsign).not.toHaveClass("hud-chroma");
    act(() => {
      tile.click();
    });
    expect(callsign).not.toHaveClass("hud-chroma");
  });
});

// ---------------------------------------------------------------------
// P1-6 — Identity overlay (vendor chip + WorkerInfo enrichment).
// ---------------------------------------------------------------------

// ---------------------------------------------------------------------
// P2-18 — Hide zero-only counter chips in the station footer.
// ---------------------------------------------------------------------

describe("Station — footer counters (P2-18)", () => {
  function counters(container: HTMLElement): HTMLElement[] {
    return Array.from(
      container.querySelectorAll<HTMLElement>(".station__counter"),
    );
  }

  it("renders all three counters when files/tests/cost are all > 0", () => {
    const { container } = renderStation(
      snapshot({
        filesAdded: 2,
        filesModified: 1,
        testsPassed: 5,
        testsFailed: 0,
        costUsd: 0.123,
      }),
    );
    const chips = counters(container);
    expect(chips).toHaveLength(3);
    expect(chips[0].textContent).toBe("+2/~1");
    expect(chips[1].textContent).toBe("5✓ 0✗");
    expect(chips[2].textContent).toBe("$0.123");
  });

  it("hides the files chip when both filesAdded and filesModified are 0", () => {
    const { container } = renderStation(
      snapshot({
        filesAdded: 0,
        filesModified: 0,
        testsPassed: 3,
        testsFailed: 0,
        costUsd: 0.01,
      }),
    );
    const chips = counters(container);
    // Tests chip + cost chip remain; no chip with "+0/~0".
    expect(chips).toHaveLength(2);
    expect(chips.some((c) => /\+0\/~0/.test(c.textContent ?? ""))).toBe(false);
    expect(chips[0].textContent).toBe("3✓ 0✗");
    expect(chips[1].textContent).toBe("$0.010");
  });

  it("hides the tests chip when both testsPassed and testsFailed are 0", () => {
    const { container } = renderStation(
      snapshot({
        filesAdded: 1,
        filesModified: 2,
        testsPassed: 0,
        testsFailed: 0,
        costUsd: 0.05,
      }),
    );
    const chips = counters(container);
    expect(chips).toHaveLength(2);
    expect(chips.some((c) => /0✓ 0✗/.test(c.textContent ?? ""))).toBe(false);
    expect(chips[0].textContent).toBe("+1/~2");
    expect(chips[1].textContent).toBe("$0.050");
  });

  it("always renders the cost chip, even when costUsd is 0", () => {
    const { container } = renderStation(
      snapshot({
        filesAdded: 0,
        filesModified: 0,
        testsPassed: 0,
        testsFailed: 0,
        costUsd: 0,
      }),
    );
    const chips = counters(container);
    expect(chips).toHaveLength(1);
    expect(chips[0].textContent).toBe("$0.000");
    expect(chips[0].getAttribute("title")).toBe("cost (USD)");
  });

  it("uses the improved tooltip phrasing on files and tests chips", () => {
    const { container } = renderStation(
      snapshot({
        filesAdded: 3,
        filesModified: 4,
        testsPassed: 7,
        testsFailed: 2,
        costUsd: 0.5,
      }),
    );
    const chips = counters(container);
    expect(chips[0].getAttribute("title")).toBe("files added / modified");
    expect(chips[1].getAttribute("title")).toBe("tests passed / failed");
    // testsFailed > 0 still triggers the alert color.
    expect(chips[1].className).toMatch(/station__counter--alert/);
  });
});

describe("Station — identity overlay (P1-6)", () => {
  it("falls back to snapshot vendor when WorkerInfo lookup is missing", async () => {
    (commands.getWorkerInfo as ReturnType<typeof vi.fn>).mockResolvedValue({
      status: "ok",
      data: null,
    });
    renderStation(
      snapshot({ id: "w-id-fallback-1", vendor: "claude", model: "sonnet" }),
    );
    const avatar = screen.getByTestId("worker-avatar");
    expect(avatar).toHaveAttribute("data-vendor", "claude");
    // Snapshot model passes through too.
    expect(screen.getByText("sonnet")).toBeInTheDocument();
  });

  it("prefers WorkerInfo vendor + model when the overlay resolves", async () => {
    (commands.getWorkerInfo as ReturnType<typeof vi.fn>).mockResolvedValue({
      status: "ok",
      data: {
        id: "w-id-overlay-1",
        name: "x",
        vendor: "codex",
        cli_binary: "codex",
        cli_version: null,
        cwd: "/tmp",
        model: "gpt-5.5",
        spawned_at: "2026-01-01T00:00:00Z",
        ended_at: null,
      },
    });
    renderStation(
      snapshot({ id: "w-id-overlay-1", vendor: "mock", model: null }),
    );
    // After the overlay resolves, avatar reflects codex.
    await waitFor(() => {
      expect(screen.getByTestId("worker-avatar")).toHaveAttribute(
        "data-vendor",
        "codex",
      );
    });
    expect(screen.getByText(/Codex/)).toBeInTheDocument();
    expect(screen.getByText("gpt-5.5")).toBeInTheDocument();
  });

  it("hides the vendor chip when the effective vendor stays at the mock fallback", () => {
    (commands.getWorkerInfo as ReturnType<typeof vi.fn>).mockResolvedValue({
      status: "ok",
      data: null,
    });
    const { container } = renderStation(
      snapshot({ id: "w-id-mock-1", vendor: "mock", model: null }),
    );
    expect(container.querySelector(".station__vendor")).toBeNull();
  });

  it("renders a pending model chip (not broken placeholder data) when no model is known", () => {
    (commands.getWorkerInfo as ReturnType<typeof vi.fn>).mockResolvedValue({
      status: "ok",
      data: null,
    });
    renderStation(
      snapshot({ id: "w-id-no-model-1", vendor: "claude", model: null }),
    );
    expect(screen.getByText("model pending")).toBeInTheDocument();
    expect(screen.queryByText("model: —")).toBeNull();
    expect(screen.queryByText("model: default")).toBeNull();
  });
});
