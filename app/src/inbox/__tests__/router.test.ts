// S10 — pure store tests for the surface router. The store is a
// trivial Zustand slice; the value is the typed surface enum +
// the mission-detail address shape.

import { beforeEach, describe, expect, it } from "vitest";
import { useSurfaceStore } from "../router";

describe("surface router", () => {
  beforeEach(() => {
    useSurfaceStore.setState({
      surface: "inbox",
      previousSurface: "inbox",
      detail: null,
    });
  });

  it("defaults to inbox surface, null detail", () => {
    const st = useSurfaceStore.getState();
    expect(st.surface).toBe("inbox");
    expect(st.detail).toBeNull();
  });

  it("setSurface transitions to ops_room / history (clearing detail)", () => {
    useSurfaceStore.getState().setSurface("ops_room");
    expect(useSurfaceStore.getState().surface).toBe("ops_room");
    useSurfaceStore.getState().setSurface("history");
    expect(useSurfaceStore.getState().surface).toBe("history");
    expect(useSurfaceStore.getState().detail).toBeNull();
  });

  it("openMission(id) sets surface=mission_detail with the id and null row", () => {
    useSurfaceStore.getState().openMission("mission-7");
    expect(useSurfaceStore.getState().surface).toBe("mission_detail");
    expect(useSurfaceStore.getState().detail).toEqual({
      missionId: "mission-7",
      row: null,
    });
  });

  it("openMission(id, row) stores the full row alongside the id", () => {
    const row = {
      mission_id: "mission-9",
      tier: "standard",
      audit_overall: 0.77,
      created_at: "2026-04-01T10:00:00Z",
      reverted: false,
      status: "merged" as const,
      target_ref: "main",
      repo_root: "/repo",
      artifacts_cleaned: false,
    };
    useSurfaceStore.getState().openMission("mission-9", row);
    expect(useSurfaceStore.getState().detail).toEqual({
      missionId: "mission-9",
      row,
    });
  });

  it("setSurface clears detail when leaving mission_detail", () => {
    useSurfaceStore.getState().openMission("mission-9", {
      mission_id: "mission-9",
      tier: "standard",
      audit_overall: 0.77,
      created_at: "2026-04-01T10:00:00Z",
      reverted: false,
      status: "merged",
      target_ref: "main",
      repo_root: "/repo",
      artifacts_cleaned: false,
    });
    expect(useSurfaceStore.getState().detail).not.toBeNull();
    useSurfaceStore.getState().setSurface("history");
    expect(useSurfaceStore.getState().detail).toBeNull();
  });

  it("back() returns to inbox from any non-inbox surface; no-op from inbox", () => {
    useSurfaceStore.getState().openMission("mission-7");
    useSurfaceStore.getState().back();
    expect(useSurfaceStore.getState().surface).toBe("inbox");
    expect(useSurfaceStore.getState().detail).toBeNull();
    // History → inbox
    useSurfaceStore.getState().setSurface("history");
    useSurfaceStore.getState().back();
    expect(useSurfaceStore.getState().surface).toBe("inbox");
    // Inbox back is no-op
    useSurfaceStore.getState().back();
    expect(useSurfaceStore.getState().surface).toBe("inbox");
  });

  it("back() from a History mission returns to History", () => {
    useSurfaceStore.getState().setSurface("history");
    useSurfaceStore.getState().openMission("mission-9");
    expect(useSurfaceStore.getState().surface).toBe("mission_detail");
    useSurfaceStore.getState().back();
    expect(useSurfaceStore.getState().surface).toBe("history");
    expect(useSurfaceStore.getState().detail).toBeNull();
  });
});
