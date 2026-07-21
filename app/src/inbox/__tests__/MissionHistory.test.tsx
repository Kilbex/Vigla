// S10 — MissionHistory component test. Mocks the Tauri command
// and asserts table rendering, the reverted-mission pill, surface
// transitions on row click, and the error path.

import { beforeEach, describe, expect, it, vi } from "vitest";
import { fireEvent, render, screen, waitFor } from "@testing-library/react";
import MissionHistory from "../MissionHistory";
import { useSurfaceStore } from "../router";
import type { MissionHistoryRow } from "../bindings-shim";

vi.mock("../../bindings", () => ({
  commands: {
    listRecentMissions: vi.fn(),
  },
}));

import { commands } from "../../bindings";

const ROWS = [
  {
    mission_id: "mission-3",
    tier: "deep",
    audit_overall: 0.92,
    created_at: "2026-05-31T11:00:00Z",
    reverted: false,
    status: "merged" as const,
    target_ref: "main",
    repo_root: "/repo",
    artifacts_cleaned: false,
  },
  {
    mission_id: "mission-2",
    tier: "standard",
    audit_overall: 0.55,
    created_at: "2026-05-31T10:00:00Z",
    reverted: true,
    status: "merged" as const,
    target_ref: "main",
    repo_root: "/repo",
    artifacts_cleaned: false,
  },
  {
    mission_id: "mission-1",
    tier: "smoke",
    audit_overall: 0.78,
    created_at: "2026-05-31T09:00:00Z",
    reverted: false,
    status: "discarded" as const,
    target_ref: "main",
    repo_root: "/repo",
    artifacts_cleaned: false,
  },
];

const setRowsOk = (rows: MissionHistoryRow[]) =>
  (commands.listRecentMissions as ReturnType<typeof vi.fn>).mockResolvedValue({
    status: "ok",
    data: rows,
  });

beforeEach(() => {
  useSurfaceStore.setState({
    surface: "history",
    previousSurface: "history",
    detail: null,
  });
  vi.clearAllMocks();
});

describe("MissionHistory", () => {
  it("renders one row per result and calls listRecentMissions with limit=20", async () => {
    setRowsOk(ROWS);
    const { container } = render(<MissionHistory />);
    await waitFor(() => {
      expect(container.querySelectorAll(".mission-history-row")).toHaveLength(3);
    });
    expect(commands.listRecentMissions).toHaveBeenCalledWith(20);
  });

  it("renders the reverted pill + risk-coloured cells", async () => {
    setRowsOk(ROWS);
    const { container } = render(<MissionHistory />);
    await waitFor(() => {
      expect(screen.getByText(/^Reverted$/)).toBeTruthy();
      expect(container.querySelector(".mission-history-overall--good")).not.toBeNull();
      expect(container.querySelector(".mission-history-overall--warning")).not.toBeNull();
    });
  });

  it("renders empty state when no rows", async () => {
    setRowsOk([]);
    render(<MissionHistory />);
    await waitFor(() => expect(screen.getByText(/no missions yet/i)).toBeTruthy());
  });

  it("clicking a row opens the mission_detail surface with the full row payload", async () => {
    setRowsOk(ROWS);
    const { container } = render(<MissionHistory />);
    await waitFor(() =>
      expect(container.querySelectorAll(".mission-history-row")).toHaveLength(3),
    );
    fireEvent.click(container.querySelectorAll(".mission-history-row")[0]);
    expect(useSurfaceStore.getState().surface).toBe("mission_detail");
    expect(useSurfaceStore.getState().detail).toEqual({
      missionId: "mission-3",
      row: ROWS[0],
    });
  });

  it("renders an error state when the command returns Err", async () => {
    (commands.listRecentMissions as ReturnType<typeof vi.fn>).mockResolvedValue({
      status: "error",
      error: "db unavailable",
    });
    render(<MissionHistory />);
    await waitFor(() => expect(screen.getByText(/db unavailable/i)).toBeTruthy());
  });

  it("shows long mission ids in full and keeps the full id in title", async () => {
    const longRow = {
      mission_id: "msn-very-long-id-12345678",
      tier: "deep",
      audit_overall: 0.9,
      created_at: "2026-05-31T11:00:00Z",
      reverted: false,
      status: "merged" as const,
      target_ref: "release/v1",
      repo_root: "/repo",
      artifacts_cleaned: false,
    };
    setRowsOk([longRow]);
    const { container } = render(<MissionHistory />);
    await waitFor(() =>
      expect(container.querySelectorAll(".mission-history-row")).toHaveLength(1),
    );
    const cell = container.querySelector(".mission-history-mission") as HTMLElement;
    expect(cell.textContent).toBe("msn-very-long-id-12345678");
    expect(cell.getAttribute("title")).toBe("msn-very-long-id-12345678");
  });

  it("renders short mission ids unchanged with the full id in title", async () => {
    const shortRow = {
      mission_id: "m1",
      tier: "smoke" as const,
      audit_overall: 0.8,
      created_at: "2026-05-31T09:00:00Z",
      reverted: false,
      status: "audited" as const,
      target_ref: null,
      repo_root: null,
      artifacts_cleaned: false,
    };
    setRowsOk([shortRow]);
    const { container } = render(<MissionHistory />);
    await waitFor(() =>
      expect(container.querySelectorAll(".mission-history-row")).toHaveLength(1),
    );
    const cell = container.querySelector(".mission-history-mission") as HTMLElement;
    expect(cell.textContent).toBe("m1");
    expect(cell.getAttribute("title")).toBe("m1");
  });

  it("renders the Merged pill (not an em-dash) for non-reverted rows", async () => {
    setRowsOk(ROWS);
    const { container } = render(<MissionHistory />);
    await waitFor(() =>
      expect(container.querySelectorAll(".mission-history-row")).toHaveLength(3),
    );
    const rows = container.querySelectorAll(".mission-history-row");
    const firstRowCells = rows[0].querySelectorAll("td");
    const firstRowStatusCell = firstRowCells[firstRowCells.length - 1];
    const mergedPill = firstRowStatusCell.querySelector(
      ".mission-history-merged-pill",
    ) as HTMLElement | null;
    expect(mergedPill).not.toBeNull();
    expect(mergedPill!.textContent).toBe("Merged");
    expect(firstRowStatusCell.textContent).not.toContain("—");
  });

  it("does not label discarded or legacy audited rows as merged", async () => {
    setRowsOk(ROWS);
    render(<MissionHistory />);
    await waitFor(() => expect(screen.getByText(/^Discarded$/)).toBeTruthy());
    expect(screen.getAllByText(/^Merged$/)).toHaveLength(1);
  });

  it("renders the Reverted pill for reverted rows", async () => {
    setRowsOk(ROWS);
    const { container } = render(<MissionHistory />);
    await waitFor(() =>
      expect(container.querySelectorAll(".mission-history-row")).toHaveLength(3),
    );
    const revertedRow = container.querySelector(
      ".mission-history-row--reverted",
    ) as HTMLElement;
    const pill = revertedRow.querySelector(
      ".mission-history-reverted-pill",
    ) as HTMLElement | null;
    expect(pill).not.toBeNull();
    expect(pill!.textContent).toBe("Reverted");
  });

  it("clicking a row with a long mission id opens the detail surface with the full id", async () => {
    const longRow = {
      mission_id: "msn-very-long-id-12345678",
      tier: "deep",
      audit_overall: 0.9,
      created_at: "2026-05-31T11:00:00Z",
      reverted: false,
      status: "merged" as const,
      target_ref: "main",
      repo_root: "/repo",
      artifacts_cleaned: false,
    };
    setRowsOk([longRow]);
    const { container } = render(<MissionHistory />);
    await waitFor(() =>
      expect(container.querySelectorAll(".mission-history-row")).toHaveLength(1),
    );
    fireEvent.click(container.querySelector(".mission-history-row")!);
    expect(useSurfaceStore.getState().surface).toBe("mission_detail");
    expect(useSurfaceStore.getState().detail).toEqual({
      missionId: "msn-very-long-id-12345678",
      row: longRow,
    });
  });
});
