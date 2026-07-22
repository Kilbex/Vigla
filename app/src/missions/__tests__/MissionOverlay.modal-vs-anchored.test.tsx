// MissionOverlay: anchored (non-blocking) for non-terminal lifecycles
// vs. modal (full-screen, aria-modal, backdrop) only for terminal
// dispositions. This is the UX fix that keeps the inbox right-rail,
// ops-room workers, history surface, and Settings dialog reachable
// while a mission is executing or awaiting plan approval / merge.

import { beforeEach, describe, expect, it, vi } from "vitest";
import { fireEvent, render } from "@testing-library/react";
import type { MissionEvent } from "../../bindings";
import MissionOverlay from "../MissionOverlay";
import { useMissionsStore } from "../store";

vi.mock("../../bindings", async () => {
  const actual =
    await vi.importActual<typeof import("../../bindings")>("../../bindings");
  return {
    ...actual,
    commands: {
      abortMission: vi.fn(),
      resolveMission: vi.fn(),
      confirmPlan: vi.fn(),
      regeneratePlan: vi.fn(),
      rejectPlan: vi.fn(),
    },
  };
});

const MID = "demo-overlay";

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

function executionStarted(seq: number): MissionEvent {
  return {
    mission_id: MID,
    seq,
    ts: `2026-05-12T00:00:0${seq}.000Z`,
    type: "mission.execution_started",
  };
}

function decomposition(seq: number): MissionEvent {
  return {
    mission_id: MID,
    seq,
    ts: `2026-05-12T00:00:0${seq}.000Z`,
    type: "supervisor.decomposition",
    payload: {
      tasks: [
        { index: 0, title: "Plan" },
        { index: 1, title: "Implement" },
      ],
    },
  };
}

function planProposed(seq: number): MissionEvent {
  return {
    mission_id: MID,
    seq,
    ts: `2026-05-12T00:00:0${seq}.000Z`,
    type: "plan.proposed",
    payload: {
      tasks: [
        { index: 0, title: "Plan" },
        { index: 1, title: "Implement" },
      ],
      generation: 0,
    },
  };
}

function completed(seq: number): MissionEvent {
  return {
    mission_id: MID,
    seq,
    ts: `2026-05-12T00:02:1${seq}.000Z`,
    type: "mission.completed",
    payload: { summary: "done", files_changed: 2 },
  };
}

function mergeResolved(seq: number): MissionEvent {
  return {
    mission_id: MID,
    seq,
    ts: `2026-05-12T00:02:1${seq}.000Z`,
    type: "mission.merge_resolved",
    payload: { resolution: { type: "merged" } },
  };
}

function aborted(seq: number): MissionEvent {
  return {
    mission_id: MID,
    seq,
    ts: `2026-05-12T00:02:1${seq}.000Z`,
    type: "mission.aborted",
    payload: { reason: "user requested" },
  };
}

function rootEl(container: HTMLElement): HTMLElement {
  const el = container.querySelector(".mission-overlay") as HTMLElement | null;
  if (!el) throw new Error("expected .mission-overlay root to render");
  return el;
}

describe("MissionOverlay modal vs. anchored", () => {
  beforeEach(() => {
    useMissionsStore.getState().reset();
    vi.clearAllMocks();
  });

  it("executing lifecycle renders as anchored, non-modal", () => {
    useMissionsStore.getState().ingest(created());
    useMissionsStore.getState().ingest(executionStarted(1));
    useMissionsStore.getState().ingest(decomposition(2));
    expect(useMissionsStore.getState().active?.lifecycle).toBe("executing");

    const { container } = render(<MissionOverlay />);
    const root = rootEl(container);

    expect(root.classList.contains("mission-overlay--anchored")).toBe(true);
    expect(root.classList.contains("mission-overlay--plan-review")).toBe(false);
    expect(root.classList.contains("mission-overlay--modal")).toBe(false);
    expect(container.querySelector(".mission-overlay__backdrop")).toBeNull();
    expect(root.getAttribute("aria-modal")).toBeNull();
    expect(root.getAttribute("role")).toBe("complementary");
  });

  it("complete_pending_merge lifecycle renders as anchored, non-modal", () => {
    useMissionsStore.getState().ingest(created());
    useMissionsStore.getState().ingest(executionStarted(1));
    useMissionsStore.getState().ingest(decomposition(2));
    useMissionsStore.getState().ingest(completed(3));
    expect(useMissionsStore.getState().active?.lifecycle).toBe(
      "complete_pending_merge",
    );

    const { container } = render(<MissionOverlay />);
    const root = rootEl(container);

    expect(root.classList.contains("mission-overlay--anchored")).toBe(true);
    expect(root.classList.contains("mission-overlay--plan-review")).toBe(false);
    expect(root.classList.contains("mission-overlay--modal")).toBe(false);
    expect(container.querySelector(".mission-overlay__backdrop")).toBeNull();
    expect(root.getAttribute("aria-modal")).toBeNull();
    expect(root.getAttribute("role")).toBe("complementary");
  });

  it("pending_plan_approval lifecycle renders as anchored, non-modal", () => {
    useMissionsStore.getState().ingest(created());
    useMissionsStore.getState().ingest(planProposed(1));
    expect(useMissionsStore.getState().active?.lifecycle).toBe(
      "pending_plan_approval",
    );

    const { container } = render(<MissionOverlay />);
    const root = rootEl(container);

    expect(root.classList.contains("mission-overlay--anchored")).toBe(true);
    expect(root.classList.contains("mission-overlay--plan-review")).toBe(true);
    expect(root.classList.contains("mission-overlay--modal")).toBe(false);
    expect(container.querySelector(".mission-overlay__backdrop")).toBeNull();
    expect(root.getAttribute("aria-modal")).toBeNull();
    expect(root.getAttribute("role")).toBe("complementary");
  });

  it("merged lifecycle renders as a true modal with backdrop and aria-modal", () => {
    useMissionsStore.getState().ingest(created());
    useMissionsStore.getState().ingest(decomposition(1));
    useMissionsStore.getState().ingest(completed(2));
    useMissionsStore.getState().ingest(mergeResolved(3));
    expect(useMissionsStore.getState().active?.lifecycle).toBe("merged");

    const { container } = render(<MissionOverlay />);
    const root = rootEl(container);

    expect(root.classList.contains("mission-overlay--modal")).toBe(true);
    expect(root.classList.contains("mission-overlay--anchored")).toBe(false);
    expect(container.querySelector(".mission-overlay__backdrop")).not.toBeNull();
    expect(root.getAttribute("role")).toBe("dialog");
    expect(root.getAttribute("aria-modal")).toBe("true");
  });

  it("aborted lifecycle renders as a true modal with backdrop and aria-modal", () => {
    useMissionsStore.getState().ingest(created());
    useMissionsStore.getState().ingest(decomposition(1));
    useMissionsStore.getState().ingest(aborted(2));
    expect(useMissionsStore.getState().active?.lifecycle).toBe("aborted");

    const { container } = render(<MissionOverlay />);
    const root = rootEl(container);

    expect(root.classList.contains("mission-overlay--modal")).toBe(true);
    expect(root.classList.contains("mission-overlay--anchored")).toBe(false);
    expect(container.querySelector(".mission-overlay__backdrop")).not.toBeNull();
    expect(root.getAttribute("role")).toBe("dialog");
    expect(root.getAttribute("aria-modal")).toBe("true");
  });

  it("Esc closes terminal (merged) but not anchored (executing)", () => {
    // Terminal -> Esc dismisses the modal while preserving mission detail.
    useMissionsStore.getState().ingest(created());
    useMissionsStore.getState().ingest(decomposition(1));
    useMissionsStore.getState().ingest(completed(2));
    useMissionsStore.getState().ingest(mergeResolved(3));
    expect(useMissionsStore.getState().active?.lifecycle).toBe("merged");
    const { unmount } = render(<MissionOverlay />);
    fireEvent.keyDown(window, { key: "Escape" });
    expect(useMissionsStore.getState().active?.id).toBe(MID);
    expect(useMissionsStore.getState().terminalOverlayDismissed).toBe(true);
    unmount();

    // Anchored -> Esc is a no-op for the mission.
    useMissionsStore.getState().ingest(created());
    useMissionsStore.getState().ingest(executionStarted(1));
    useMissionsStore.getState().ingest(decomposition(2));
    expect(useMissionsStore.getState().active?.lifecycle).toBe("executing");
    render(<MissionOverlay />);
    fireEvent.keyDown(window, { key: "Escape" });
    expect(useMissionsStore.getState().active?.id).toBe(MID);
    expect(useMissionsStore.getState().active?.lifecycle).toBe("executing");
  });
});
