import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { fireEvent, render, screen, waitFor } from "@testing-library/react";
import type { MissionEvent, MissionSpec } from "../bindings";
import { formatTeam } from "../missions/MissionActiveView";
import MissionOverlay from "../missions/MissionOverlay";
import { useMissionsStore } from "../missions/store";

vi.mock("../bindings", async () => {
  const actual =
    await vi.importActual<typeof import("../bindings")>("../bindings");
  return {
    ...actual,
    commands: {
      startMission: vi.fn(),
      abortMission: vi.fn(),
      resolveMission: vi.fn(),
      confirmPlan: vi.fn(),
      regeneratePlan: vi.fn(),
    },
  };
});

import { commands } from "../bindings";

const MID = "demo-7a3f";

type CreatedMissionEvent = Extract<MissionEvent, { type: "mission.created" }>;

function created(): CreatedMissionEvent {
  return {
    mission_id: MID,
    seq: 0,
    ts: "2026-05-12T00:00:00.000Z",
    type: "mission.created",
    payload: {
      spec: {
        title: "Add logout",
        objective: "Add /api/logout",
        target_ref: "main",
        tests: null,
        supervisor_model: null,
        worker_model: null,
        worker_count: null,
        confirm_plan: null,
      },
    },
  };
}

function createdWithSpec(overrides: Partial<MissionSpec>): CreatedMissionEvent {
  const base = created();
  return {
    ...base,
    payload: {
      spec: {
        ...base.payload.spec,
        ...overrides,
      },
    },
  };
}

function decomposition(): MissionEvent {
  return {
    mission_id: MID,
    seq: 1,
    ts: "2026-05-12T00:00:01.000Z",
    type: "supervisor.decomposition",
    payload: {
      tasks: [
        { index: 0, title: "Plan" },
        { index: 1, title: "Implement" },
      ],
    },
  };
}

function decompositionThree(): MissionEvent {
  return {
    mission_id: MID,
    seq: 1,
    ts: "2026-05-12T00:00:01.000Z",
    type: "supervisor.decomposition",
    payload: {
      tasks: [
        { index: 0, title: "Plan integration" },
        { index: 1, title: "Implement changes" },
        { index: 2, title: "Update documentation" },
      ],
    },
  };
}

function completed(): MissionEvent {
  return {
    mission_id: MID,
    seq: 9,
    ts: "2026-05-12T00:02:10.000Z",
    type: "mission.completed",
    payload: { summary: "2 tasks integrated", files_changed: 2 },
  };
}

describe("formatTeam", () => {
  const base = {
    title: "T",
    objective: "O",
    target_ref: "main",
    tests: null as string | null,
    supervisor_model: null as string | null,
    worker_model: null as string | null,
    worker_count: null as number | null,
    confirm_plan: null,
  };

  it("falls back to Claude supervisor and auto workers when nothing is set", () => {
    expect(formatTeam(base)).toBe("Claude supervisor · auto workers");
  });

  it("shows explicit worker count when count is set and vendor is auto", () => {
    expect(formatTeam({ ...base, worker_count: 3 })).toBe(
      "Claude supervisor · 3 workers",
    );
  });

  it("shows the vendor when worker model is set and count is auto", () => {
    expect(formatTeam({ ...base, worker_model: "codex" })).toBe(
      "Claude supervisor · Codex workers",
    );
  });

  it("combines count and vendor when both set", () => {
    expect(
      formatTeam({ ...base, worker_count: 5, worker_model: "gemini" }),
    ).toBe("Claude supervisor · 5 Gemini workers");
  });

  it("shows independently selected worker CLIs", () => {
    expect(
      formatTeam({
        ...base,
        worker_count: 3,
        worker_model: "claude:sonnet,codex:gpt-5.5,gemini:flash",
      }),
    ).toBe(
      "Claude supervisor · 3 Claude Sonnet / Codex gpt-5.5 / Gemini Flash workers",
    );
  });

  it("singularizes 'worker' when count is 1", () => {
    expect(formatTeam({ ...base, worker_count: 1 })).toBe(
      "Claude supervisor · 1 worker",
    );
    expect(formatTeam({ ...base, worker_count: 1, worker_model: "claude" })).toBe(
      "Claude supervisor · 1 Claude worker",
    );
  });

  it("honors a non-default supervisor model", () => {
    expect(formatTeam({ ...base, supervisor_model: "codex" })).toBe(
      "Codex supervisor · auto workers",
    );
  });
});

describe("MissionOverlay", () => {
  beforeEach(() => {
    useMissionsStore.getState().reset();
    vi.clearAllMocks();
  });
  afterEach(() => {
    vi.clearAllMocks();
  });

  it("renders nothing when no mission is active", () => {
    const { container } = render(<MissionOverlay />);
    expect(container.firstChild).toBeNull();
  });

  it("renders the active view with title, status, progress, and task list once a mission exists", () => {
    useMissionsStore.getState().ingest(created());
    useMissionsStore.getState().ingest(decomposition());
    render(<MissionOverlay />);
    expect(screen.getByRole("heading", { name: "Add logout" })).toBeTruthy();
    expect(screen.getByText(/Add \/api\/logout/i)).toBeTruthy();
    expect(screen.getByText("Plan")).toBeTruthy();
    expect(screen.getByText("Implement")).toBeTruthy();
    expect(screen.getByRole("button", { name: /abort/i })).toBeTruthy();
  });

  it("renders the N=3 employee team view legibly without decision actions", () => {
    useMissionsStore.getState().ingest(createdWithSpec({ worker_count: 3 }));
    useMissionsStore.getState().ingest({
      mission_id: MID,
      seq: 1,
      ts: "2026-05-12T00:00:00.500Z",
      type: "mission.execution_started",
    });
    useMissionsStore.getState().ingest(decompositionThree());
    for (let i = 0; i < 3; i++) {
      useMissionsStore.getState().ingest({
        mission_id: MID,
        seq: 2 + i,
        ts: `2026-05-12T00:00:0${i + 2}.000Z`,
        type: "worker.spawned",
        payload: {
          worker_id: `mock-${i + 1}`,
          task_index: i,
          task_title: ["Plan integration", "Implement changes", "Update documentation"][i],
        },
      });
    }

    const { container } = render(<MissionOverlay />);
    expect(screen.getByText("Claude supervisor · 3 workers")).toBeTruthy();
    expect(screen.getByTestId("supervisor-strip").textContent).toMatch(
      /dispatched mock-3/i,
    );
    expect(screen.getByText("Plan integration")).toBeTruthy();
    expect(screen.getByText("Implement changes")).toBeTruthy();
    expect(screen.getByText("Update documentation")).toBeTruthy();
    expect(container.querySelectorAll(".mission-active__task")).toHaveLength(3);
    expect(screen.getByRole("button", { name: /^abort$/i })).toBeTruthy();
    expect(screen.queryByRole("button", { name: /^merge$/i })).toBeNull();
    expect(screen.queryByRole("button", { name: /^discard$/i })).toBeNull();
    expect(
      screen.queryByRole("button", { name: /continue with directive/i }),
    ).toBeNull();
  });

  it("Abort triggers commands.abortMission", async () => {
    vi.mocked(commands.abortMission).mockResolvedValueOnce({
      status: "ok",
      data: null,
    });
    useMissionsStore.getState().ingest(created());
    useMissionsStore.getState().ingest(decomposition());
    render(<MissionOverlay />);
    fireEvent.click(screen.getByRole("button", { name: /abort/i }));
    await waitFor(() =>
      expect(commands.abortMission).toHaveBeenCalledTimes(1),
    );
  });

  it("shows the terminal verdict after merged, with Done preserving mission detail", () => {
    useMissionsStore.getState().ingest(created());
    useMissionsStore.getState().ingest(decomposition());
    useMissionsStore.getState().ingest(completed());
    useMissionsStore.getState().ingest({
      mission_id: MID,
      seq: 10,
      ts: "2026-05-12T00:02:15.000Z",
      type: "mission.merge_resolved",
      payload: { resolution: { type: "merged" } },
    });
    render(<MissionOverlay />);
    expect(screen.getByRole("heading", { name: "Merged" })).toBeTruthy();
    expect(screen.getByRole("button", { name: /view changes/i })).toBeTruthy();
    fireEvent.click(screen.getByRole("button", { name: /done/i }));
    expect(useMissionsStore.getState().active?.id).toBe(MID);
    expect(useMissionsStore.getState().terminalOverlayDismissed).toBe(true);
  });

  it("Esc dismisses the terminal screen without clearing mission detail", () => {
    useMissionsStore.getState().ingest(created());
    useMissionsStore.getState().ingest(decomposition());
    useMissionsStore.getState().ingest(completed());
    useMissionsStore.getState().ingest({
      mission_id: MID,
      seq: 10,
      ts: "2026-05-12T00:02:15.000Z",
      type: "mission.merge_resolved",
      payload: { resolution: { type: "merged" } },
    });
    render(<MissionOverlay />);
    expect(screen.getByRole("heading", { name: "Merged" })).toBeTruthy();
    fireEvent.keyDown(window, { key: "Escape" });
    expect(useMissionsStore.getState().active?.id).toBe(MID);
    expect(useMissionsStore.getState().terminalOverlayDismissed).toBe(true);
  });

  it("Esc does NOT dismiss the active mission view (would lose work)", () => {
    useMissionsStore.getState().ingest(created());
    useMissionsStore.getState().ingest(decomposition());
    render(<MissionOverlay />);
    expect(screen.getByRole("heading", { name: "Add logout" })).toBeTruthy();
    fireEvent.keyDown(window, { key: "Escape" });
    // Store unchanged — Esc is a no-op while a mission is running.
    expect(useMissionsStore.getState().active?.id).toBe(MID);
  });
});

describe("MissionReviewOutcome", () => {
  beforeEach(() => {
    useMissionsStore.getState().reset();
    vi.clearAllMocks();
  });

  function fillCompleteMission() {
    useMissionsStore.getState().ingest(created());
    useMissionsStore.getState().ingest(decomposition());
    // Spawn + submit each worker so the file list is populated.
    for (const [taskIdx, worker] of [
      [0, "mock-1"],
      [1, "mock-2"],
    ] as const) {
      useMissionsStore.getState().ingest({
        mission_id: MID,
        seq: 10 + taskIdx * 4,
        ts: "2026-05-12T00:00:10.000Z",
        type: "worker.spawned",
        payload: {
          worker_id: worker,
          task_index: taskIdx,
          task_title: taskIdx === 0 ? "Plan" : "Implement",
        },
      });
      useMissionsStore.getState().ingest({
        mission_id: MID,
        seq: 11 + taskIdx * 4,
        ts: "2026-05-12T00:00:20.000Z",
        type: "worker.result_submitted",
        payload: {
          worker_id: worker,
          files: [`MOCK_${taskIdx}.md`],
          summary: "work",
        },
      });
      useMissionsStore.getState().ingest({
        mission_id: MID,
        seq: 12 + taskIdx * 4,
        ts: "2026-05-12T00:00:25.000Z",
        type: "supervisor.integrated",
        payload: {
          worker_id: worker,
          integration_sha: "0".repeat(40),
          snapshot_tag: `vigla/snap/${MID}/${taskIdx}`,
        },
      });
    }
    useMissionsStore.getState().ingest({
      mission_id: MID,
      seq: 30,
      ts: "2026-05-12T00:02:00.000Z",
      type: "supervisor.test_result",
      payload: { passed: true, summary: "all pass" },
    });
    useMissionsStore.getState().ingest(completed());
  }

  it("offers only dispositions the runtime can complete", () => {
    fillCompleteMission();
    render(<MissionOverlay />);
    expect(screen.getByRole("heading", { name: /Add logout/ })).toBeTruthy();
    expect(screen.getByText(/2 tasks integrated/i)).toBeTruthy();
    expect(screen.getByText(/Claude supervisor · auto workers/i)).toBeTruthy();
    expect(screen.getByText("MOCK_0.md")).toBeTruthy();
    expect(screen.getByText("MOCK_1.md")).toBeTruthy();
    expect(screen.getByText(/passing/i)).toBeTruthy();
    expect(screen.getByRole("button", { name: /^merge$/i })).toBeTruthy();
    expect(screen.getByRole("button", { name: /^discard$/i })).toBeTruthy();
    expect(
      screen.queryByRole("button", { name: /continue with directive/i }),
    ).toBeNull();
  });

  it("Merge calls commands.resolveMission with merge action and waits for terminal event", async () => {
    vi.mocked(commands.resolveMission).mockResolvedValueOnce({
      status: "ok",
      data: null,
    });
    fillCompleteMission();
    render(<MissionOverlay />);
    fireEvent.click(screen.getByRole("button", { name: /^merge$/i }));
    await waitFor(() => {
      expect(commands.resolveMission).toHaveBeenCalledWith({ type: "merge" });
    });
    expect(useMissionsStore.getState().active?.lifecycle).toBe(
      "complete_pending_merge",
    );
    expect(screen.getByRole("button", { name: /^merge$/i })).toBeTruthy();
  });

  it("does not surface a stale error when the host is already merged", async () => {
    vi.mocked(commands.resolveMission).mockResolvedValueOnce({
      status: "error",
      error: "resolve not allowed from state Merged",
    });
    fillCompleteMission();
    render(<MissionOverlay />);
    fireEvent.click(screen.getByRole("button", { name: /^merge$/i }));
    await waitFor(() => {
      expect(commands.resolveMission).toHaveBeenCalledWith({ type: "merge" });
    });
    expect(screen.queryByText(/resolve not allowed from state merged/i)).toBeNull();
  });

  it("Discard calls commands.resolveMission with discard action", async () => {
    vi.mocked(commands.resolveMission).mockResolvedValueOnce({
      status: "ok",
      data: null,
    });
    fillCompleteMission();
    render(<MissionOverlay />);
    fireEvent.click(screen.getByRole("button", { name: /^discard$/i }));
    await waitFor(() => {
      expect(commands.resolveMission).toHaveBeenCalledWith({ type: "discard" });
    });
  });

  it("surfaces a resolve error inline without leaving the review screen", async () => {
    vi.mocked(commands.resolveMission).mockResolvedValueOnce({
      status: "error",
      error: "boom",
    });
    fillCompleteMission();
    render(<MissionOverlay />);
    fireEvent.click(screen.getByRole("button", { name: /^merge$/i }));
    await waitFor(() => {
      expect(screen.getByText("boom")).toBeTruthy();
    });
    expect(screen.getByRole("button", { name: /^merge$/i })).toBeTruthy();
  });

});

// ──────────────────────────────────────────────────────────────────
// QC-2: MissionPlanPreview.
// ──────────────────────────────────────────────────────────────────

describe("MissionPlanPreview", () => {
  beforeEach(() => {
    useMissionsStore.getState().reset();
    vi.clearAllMocks();
  });

  function paused(): void {
    useMissionsStore.getState().ingest(created());
    useMissionsStore.getState().ingest({
      mission_id: MID,
      seq: 1,
      ts: "2026-05-12T00:00:01.000Z",
      type: "plan.proposed",
      payload: {
        tasks: [
          { index: 0, title: "First proposed task" },
          { index: 1, title: "Second proposed task" },
        ],
        generation: 0,
      },
    });
  }

  it("renders proposed tasks with Start + Regenerate buttons when paused", () => {
    paused();
    render(<MissionOverlay />);
    // QC-3: the task titles appear in both the mind map (React-Flow
    // nodes) and the task list, so we assert presence via
    // getAllByText rather than uniqueness.
    expect(screen.getAllByText("First proposed task").length).toBeGreaterThan(
      0,
    );
    expect(screen.getAllByText("Second proposed task").length).toBeGreaterThan(
      0,
    );
    expect(screen.getByRole("button", { name: /approve plan/i })).toBeTruthy();
    expect(screen.getByRole("button", { name: /^regenerate$/i })).toBeTruthy();
  });

  it("Approve plan calls commands.confirmPlan", async () => {
    vi.mocked(commands.confirmPlan).mockResolvedValueOnce({
      status: "ok",
      data: null,
    });
    paused();
    render(<MissionOverlay />);
    fireEvent.click(screen.getByRole("button", { name: /approve plan/i }));
    await waitFor(() => {
      expect(commands.confirmPlan).toHaveBeenCalledWith(0);
    });
  });

  it("Regenerate reveals the hint textarea + two regenerate buttons + Cancel", () => {
    paused();
    render(<MissionOverlay />);
    fireEvent.click(screen.getByRole("button", { name: /^regenerate$/i }));
    expect(screen.getByPlaceholderText(/split task a/i)).toBeTruthy();
    expect(
      screen.getByRole("button", { name: /regenerate without feedback/i }),
    ).toBeTruthy();
    expect(
      screen.getByRole("button", { name: /regenerate with feedback/i }),
    ).toBeTruthy();
    expect(screen.getByRole("button", { name: /^cancel$/i })).toBeTruthy();
  });

  it("Regenerate with feedback is gated on non-empty hint", () => {
    paused();
    render(<MissionOverlay />);
    fireEvent.click(screen.getByRole("button", { name: /^regenerate$/i }));
    const withBtn = screen.getByRole("button", {
      name: /regenerate with feedback/i,
    }) as HTMLButtonElement;
    expect(withBtn.disabled).toBe(true);

    fireEvent.change(screen.getByPlaceholderText(/split task a/i), {
      target: { value: "smaller tasks please" },
    });
    expect(withBtn.disabled).toBe(false);
  });

  it("Regenerate with feedback calls regeneratePlan(trimmed hint)", async () => {
    vi.mocked(commands.regeneratePlan).mockResolvedValueOnce({
      status: "ok",
      data: null,
    });
    paused();
    render(<MissionOverlay />);
    fireEvent.click(screen.getByRole("button", { name: /^regenerate$/i }));
    fireEvent.change(screen.getByPlaceholderText(/split task a/i), {
      target: { value: "  smaller tasks please  " },
    });
    fireEvent.click(
      screen.getByRole("button", { name: /regenerate with feedback/i }),
    );
    await waitFor(() => {
      expect(commands.regeneratePlan).toHaveBeenCalledWith(
        0,
        "smaller tasks please",
      );
    });
  });

  it("Regenerate without feedback calls regeneratePlan(null)", async () => {
    vi.mocked(commands.regeneratePlan).mockResolvedValueOnce({
      status: "ok",
      data: null,
    });
    paused();
    render(<MissionOverlay />);
    fireEvent.click(screen.getByRole("button", { name: /^regenerate$/i }));
    fireEvent.click(
      screen.getByRole("button", { name: /regenerate without feedback/i }),
    );
    await waitFor(() => {
      expect(commands.regeneratePlan).toHaveBeenCalledWith(0, null);
    });
  });

  it("surfaces a confirmPlan error inline; Start button remains usable", async () => {
    vi.mocked(commands.confirmPlan).mockResolvedValueOnce({
      status: "error",
      error: "plan decision not allowed from state Executing",
    });
    paused();
    render(<MissionOverlay />);
    fireEvent.click(screen.getByRole("button", { name: /approve plan/i }));
    await waitFor(() => {
      expect(screen.getByText(/plan decision not allowed/i)).toBeTruthy();
    });
    const start = screen.getByRole("button", {
      name: /approve plan/i,
    }) as HTMLButtonElement;
    expect(start.disabled).toBe(false);
  });

  it("clears submitting on confirmPlan promise rejection so the user can retry", async () => {
    vi.mocked(commands.confirmPlan).mockRejectedValueOnce(
      new Error("IPC closed"),
    );
    paused();
    render(<MissionOverlay />);
    fireEvent.click(screen.getByRole("button", { name: /approve plan/i }));
    await waitFor(() => {
      expect(screen.getByText(/IPC closed/i)).toBeTruthy();
    });
    const start = screen.getByRole("button", {
      name: /approve plan/i,
    }) as HTMLButtonElement;
    expect(start.disabled).toBe(false);
  });
});
