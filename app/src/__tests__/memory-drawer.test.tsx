import { describe, it, expect, beforeEach, vi } from "vitest";
import { act, fireEvent, render, screen, waitFor } from "@testing-library/react";
import { useMissionsStore } from "../missions/store";
import MemoryDrawer, { describeEvent, shortId, shortTs } from "../memory/MemoryDrawer";
import MemoryDrawerButton from "../memory/MemoryDrawerButton";

// IPC mocks. Each test wires whatever shape it needs.
const memoryListNotes = vi.fn();
const memoryRecentEventsForMission = vi.fn();
const memoryLatestBundleForMission = vi.fn();

vi.mock("../bindings", () => {
  return {
    commands: {
      memoryListNotes: (...args: unknown[]) => memoryListNotes(...args),
      memoryRecentEventsForMission: (...args: unknown[]) =>
        memoryRecentEventsForMission(...args),
      memoryLatestBundleForMission: (...args: unknown[]) =>
        memoryLatestBundleForMission(...args),
    },
  };
});

function setActiveMission(missionId: string | null) {
  // Reach into the store to seed the active mission. Avoids spinning
  // up the full mission-event ingest just to provide a single id.
  if (missionId === null) {
    useMissionsStore.getState().reset();
    return;
  }
  useMissionsStore.setState((prev) => ({
    ...prev,
    active: {
      id: missionId,
      spec: {
        title: "test",
        objective: "test",
        target_ref: "main",
        tests: null,
        supervisor_model: null,
        worker_model: null,
        worker_count: null,
        confirm_plan: null,
      },
      lifecycle: "created",
      tasks: [],
      workers: [],
      proposed_tasks: [],
      attention: [],
      mission_status: null,
      progress: null,
      plan_generation: 0,
      supervisor_activity: null,
      awaiting_disposition: false,
      pending_plan_approval: false,
      attention_count: 0,
    } as any,
  }));
}

describe("MemoryDrawer pure formatters", () => {
  it("shortId preserves short ids and truncates long ones", () => {
    expect(shortId("01J")).toBe("01J");
    expect(shortId("01J2345678ABCDEF")).toBe("01J23456…");
  });

  it("shortTs extracts HH:MM:SS from an RFC 3339 timestamp", () => {
    expect(shortTs("2026-05-16T14:22:01.481Z")).toBe("14:22:01");
    expect(shortTs("not-a-timestamp")).toBe("not-a-timestamp");
  });

  it("describeEvent labels each kind succinctly", () => {
    expect(
      describeEvent({
        type: "proposed",
        proposal_id: "p1",
        kind: "hazard",
        body_preview: "x",
      }),
    ).toEqual({ label: "proposed hazard", detail: "x" });

    expect(
      describeEvent({
        type: "ratified",
        proposal_id: "p1",
        note_id: "01J234567890ABCDEF",
        decision: "accept",
      }),
    ).toEqual({ label: "supervisor accept", detail: "→ 01J23456…" });

    expect(
      describeEvent({
        type: "promoted",
        note_id: "01J234567890ABCDEF",
        confidence: 0.77,
      }),
    ).toEqual({
      label: "learned after accept",
      detail: "note 01J23456… · conf 0.77",
    });

    expect(describeEvent({ type: "barrier", kind: "accept" })).toEqual({
      label: "mission accept",
      detail: null,
    });

    expect(describeEvent({ type: "other", event_type: "future_thing" })).toEqual({
      label: "future_thing",
      detail: null,
    });
  });
});

const REPO_CWD = "/tmp/test-repo";

describe("MemoryDrawer component", () => {
  beforeEach(() => {
    memoryListNotes.mockReset();
    memoryRecentEventsForMission.mockReset();
    memoryLatestBundleForMission.mockReset();
    setActiveMission(null);
    // A2: most tests need a current repo seeded. The
    // "no-repo / no-IPC" test explicitly resets this below.
    useMissionsStore.getState().setCurrentRepoCwd(REPO_CWD);
    // Default: every command returns an empty ok response. Individual
    // tests override.
    memoryListNotes.mockResolvedValue({ status: "ok", data: [] });
    memoryRecentEventsForMission.mockResolvedValue({ status: "ok", data: [] });
    memoryLatestBundleForMission.mockResolvedValue({ status: "ok", data: null });
  });

  it("renders three sections and empty hints when nothing is in memory", async () => {
    render(<MemoryDrawer onClose={() => {}} />);
    expect(screen.getByText("Attached memory")).toBeInTheDocument();
    expect(screen.getByText("Recent proposals")).toBeInTheDocument();
    expect(screen.getByText("Promoted notes")).toBeInTheDocument();
    await waitFor(() => {
      expect(memoryListNotes).toHaveBeenCalled();
    });
  });

  it("renders promoted notes when the codex has some", async () => {
    memoryListNotes.mockResolvedValue({
      status: "ok",
      data: [
        {
          id: "01J2345678",
          kind: "hazard",
          scope_kind: "repo",
          scope_value: null,
          state: "promoted",
          created_at: "2026-05-16T14:22:01.481Z",
        },
      ],
    });
    render(<MemoryDrawer onClose={() => {}} />);
    await waitFor(() => {
      expect(screen.getByText("hazard")).toBeInTheDocument();
    });
    expect(screen.getByText("1 promoted")).toBeInTheDocument();
  });

  it("queries mission-scoped endpoints only when a mission is active", async () => {
    setActiveMission("mission-abc");
    render(<MemoryDrawer onClose={() => {}} />);
    await waitFor(() => {
      expect(memoryListNotes).toHaveBeenCalled();
      expect(memoryRecentEventsForMission).toHaveBeenCalledWith(
        REPO_CWD,
        "mission-abc",
        50,
      );
      expect(memoryLatestBundleForMission).toHaveBeenCalledWith(
        REPO_CWD,
        "mission-abc",
      );
    });
  });

  it("skips mission-scoped IPC calls when no mission is active", async () => {
    render(<MemoryDrawer onClose={() => {}} />);
    await waitFor(() => {
      expect(memoryListNotes).toHaveBeenCalled();
    });
    // Mission-scoped calls were never issued — the drawer correctly
    // showed the "No active mission" subtitle instead.
    expect(memoryRecentEventsForMission).not.toHaveBeenCalled();
    expect(memoryLatestBundleForMission).not.toHaveBeenCalled();
    expect(screen.getAllByText("No active mission").length).toBeGreaterThan(0);
  });

  // A2: when no repository has been started this session, the drawer
  // renders explanatory empty state and skips ALL IPC calls. No
  // memoryListNotes either.
  it("renders no-repo state and skips all IPC when currentRepoCwd is null", async () => {
    useMissionsStore.getState().setCurrentRepoCwd(null as unknown as string);
    // Recreate the empty store state cleanly via reset.
    useMissionsStore.getState().reset();
    render(<MemoryDrawer onClose={() => {}} />);
    // The drawer should mention the no-repo condition.
    await waitFor(() => {
      expect(screen.getAllByText("No active repository").length).toBeGreaterThan(0);
    });
    expect(memoryListNotes).not.toHaveBeenCalled();
    expect(memoryRecentEventsForMission).not.toHaveBeenCalled();
    expect(memoryLatestBundleForMission).not.toHaveBeenCalled();
  });

  it("displays a per-event row using describeEvent", async () => {
    setActiveMission("mission-abc");
    memoryRecentEventsForMission.mockResolvedValue({
      status: "ok",
      data: [
        {
          event_id: "ev-1",
          mission_id: "mission-abc",
          worker_id: "w1",
          ts: "2026-05-16T14:22:01.481Z",
          kind: {
            type: "promoted",
            note_id: "01J234567890ABCDEF",
            confidence: 0.91,
          },
        },
      ],
    });
    render(<MemoryDrawer onClose={() => {}} />);
    await waitFor(() => {
      expect(screen.getByText("learned after accept")).toBeInTheDocument();
    });
    expect(screen.getByText(/conf 0.91/)).toBeInTheDocument();
    expect(screen.getByText("14:22:01")).toBeInTheDocument();
  });

  it("surfaces the first IPC error without blanking other sections", async () => {
    memoryListNotes.mockResolvedValue({
      status: "error",
      error: "list failed",
    });
    render(<MemoryDrawer onClose={() => {}} />);
    await waitFor(() => {
      expect(screen.getByRole("alert")).toHaveTextContent("list failed");
    });
    // The other sections still render (with empty hints).
    expect(screen.getByText("Attached memory")).toBeInTheDocument();
    expect(screen.getByText("Recent proposals")).toBeInTheDocument();
  });

  it("keeps other sections when one IPC command throws", async () => {
    memoryListNotes.mockRejectedValue(new Error("list exploded"));
    setActiveMission("mission-abc");
    memoryRecentEventsForMission.mockResolvedValue({
      status: "ok",
      data: [
        {
          event_id: "ev-1",
          mission_id: "mission-abc",
          worker_id: "w1",
          ts: "2026-05-16T14:22:01.481Z",
          kind: {
            type: "barrier",
            kind: "accept",
          },
        },
      ],
    });
    render(<MemoryDrawer onClose={() => {}} />);
    await waitFor(() => {
      expect(screen.getByRole("alert")).toHaveTextContent("list exploded");
    });
    expect(screen.getByText("mission accept")).toBeInTheDocument();
  });

  it("invokes onClose when the close button is clicked", () => {
    const onClose = vi.fn();
    render(<MemoryDrawer onClose={onClose} />);
    fireEvent.click(screen.getByLabelText(/close memory drawer/i));
    expect(onClose).toHaveBeenCalledTimes(1);
  });

  it("invokes onClose on Escape", () => {
    const onClose = vi.fn();
    render(<MemoryDrawer onClose={onClose} />);
    act(() => {
      fireEvent.keyDown(window, { key: "Escape" });
    });
    expect(onClose).toHaveBeenCalledTimes(1);
  });
});

describe("MemoryDrawerButton", () => {
  beforeEach(() => {
    memoryListNotes.mockReset().mockResolvedValue({ status: "ok", data: [] });
    memoryRecentEventsForMission
      .mockReset()
      .mockResolvedValue({ status: "ok", data: [] });
    memoryLatestBundleForMission
      .mockReset()
      .mockResolvedValue({ status: "ok", data: null });
    setActiveMission(null);
    // A2: seed cwd so the drawer's IPC fires when opened.
    useMissionsStore.getState().setCurrentRepoCwd(REPO_CWD);
  });

  it("renders collapsed by default and does not query IPC", () => {
    render(<MemoryDrawerButton />);
    expect(screen.queryByRole("dialog")).toBeNull();
    // Closed drawer means no IPC calls yet.
    expect(memoryListNotes).not.toHaveBeenCalled();
  });

  it("opens the drawer on click and starts polling", async () => {
    render(<MemoryDrawerButton />);
    fireEvent.click(screen.getByRole("button", { name: /^memory$/i }));
    expect(screen.getByRole("dialog")).toBeInTheDocument();
    await waitFor(() => {
      expect(memoryListNotes).toHaveBeenCalled();
    });
  });

  it("toggles closed on a second click", () => {
    render(<MemoryDrawerButton />);
    const btn = screen.getByRole("button", { name: /^memory$/i });
    fireEvent.click(btn);
    expect(screen.getByRole("dialog")).toBeInTheDocument();
    fireEvent.click(btn);
    expect(screen.queryByRole("dialog")).toBeNull();
  });
});
