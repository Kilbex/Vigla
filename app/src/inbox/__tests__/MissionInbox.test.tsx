// S10 — MissionInbox component test. Mocks the active mission
// store, asserts the header / breakdown / issues / subtask list
// / revert button all render. Also asserts the back button
// returns to the inbox surface.

import { beforeEach, describe, expect, it, vi } from "vitest";
import { fireEvent, render, screen } from "@testing-library/react";
import MissionInbox from "../MissionInbox";
import { useSurfaceStore } from "../router";
import type { ActiveMission } from "../../missions/types";

vi.mock("../../bindings", () => ({
  commands: {
    revertMission: vi.fn(),
    cleanupMissionArtifacts: vi.fn(),
  },
}));

// Mock the missions store so MissionInbox sees a fully-populated
// active mission.
vi.mock("../../missions/store", async (orig) => {
  const realModule = (await orig()) as Record<string, unknown>;
  return {
    ...realModule,
    useMissionsStore: vi.fn(),
    selectActiveMission: (s: { active: ActiveMission | null }) => s.active,
  };
});

import { useMissionsStore } from "../../missions/store";

const AUDIT_PAYLOAD = JSON.stringify({
  overall: 0.83,
  test_pass: { ran: true, passed: 12, failed: 0, skipped: 0, score: 1.0 },
  scope: { in_scope: 5, out_of_scope: 0, score: 1.0 },
  regression: null,
  lint: null,
  security_flags: [],
});

const FULL_MISSION = {
  id: "mission-1",
  spec: {
    title: "Refactor parser",
    objective: "Tidy the parser",
    target_ref: "main",
    tests: null,
    supervisor_model: null,
    worker_model: null,
    worker_count: null,
    confirm_plan: null,
    scope_paths: [],
  },
  lifecycle: "merged" as const,
  startedAt: "2026-05-31T00:00:00Z",
  updatedAt: "2026-05-31T00:05:00Z",
  statusLine: "Mission merged",
  progressPercent: 100,
  tasks: [
    {
      index: 0,
      title: "subtask one",
      status: "integrated" as const,
      assignedWorkerId: "worker-1",
      integrationSha: "abc",
      snapshotTag: "vigla/pre-merge/mission-1-0",
    },
    {
      index: 1,
      title: "subtask two",
      status: "failed" as const,
      assignedWorkerId: "worker-2",
      integrationSha: null,
      snapshotTag: null,
    },
  ],
  workers: {},
  testsPassed: true,
  completionSummary: "all good",
  filesChanged: 4,
  resolution: { type: "merged" } as const,
  abortReason: null,
  attention: [],
  supervisorActivity: null,
  lastExtensionDirective: null,
  lastExtensionAt: null,
  planGeneration: 0,
  audit: { tier: "standard", overall: 0.83 },
  auditPayloadJson: AUDIT_PAYLOAD,
  verdict: {
    all_subtasks_accepted: true,
    integrated_test_pass: {
      ran: true,
      passed: 12,
      failed: 0,
      skipped: 0,
      score: 1.0,
    },
    residual_risk: "low",
    doc_coverage: 0.9,
    unresolved_issues: [
      {
        kind: "context_budget_truncated",
        dropped_count: 2,
        worker_id: "mock-1",
      },
    ],
    recommendation: { kind: "accept", audit: {}, summary: "shipped" },
  },
  inbox: [],
} as unknown as ActiveMission;

beforeEach(() => {
  window.localStorage.clear();
  useSurfaceStore.setState({
    surface: "mission_detail",
    previousSurface: "inbox",
    detail: { missionId: "mission-1", row: null },
  });
  (useMissionsStore as unknown as ReturnType<typeof vi.fn>).mockImplementation(
    (
      selector?: (s: {
        active: ActiveMission | null;
        currentRepoCwd: string | null;
      }) => unknown,
    ) => {
      const state = { active: FULL_MISSION, currentRepoCwd: "/repo" };
      return selector ? selector(state) : state;
    },
  );
});

describe("MissionInbox", () => {
  it("renders title / status badge / risk badge / audit / issues / subtasks / revert", () => {
    const { container } = render(<MissionInbox />);
    expect(screen.getByText("Refactor parser")).toBeTruthy();
    expect(screen.getByText(/^MERGED$/)).toBeTruthy();
    expect(screen.getByText(/^Low risk$/i)).toBeTruthy();
    expect(screen.getByText("0.83")).toBeTruthy();
    expect(screen.getByText(/Context truncated/i)).toBeTruthy();
    expect(container.querySelectorAll(".subtask-row")).toHaveLength(2);
    expect(screen.getByRole("button", { name: /revert mission/i })).toBeTruthy();
  });

  it("does not expose Revert before the merge disposition", () => {
    (useMissionsStore as unknown as ReturnType<typeof vi.fn>).mockImplementation(
      (
        selector?: (s: {
          active: ActiveMission | null;
          currentRepoCwd: string | null;
        }) => unknown,
      ) => {
        const state = {
          active: {
            ...FULL_MISSION,
            lifecycle: "complete_pending_merge" as const,
            resolution: null,
          },
          currentRepoCwd: "/repo",
        };
        return selector ? selector(state) : state;
      },
    );
    render(<MissionInbox />);
    expect(screen.queryByRole("button", { name: /revert mission/i })).toBeNull();
  });

  it("styles an active reverted mission as terminal instead of running", () => {
    (useMissionsStore as unknown as ReturnType<typeof vi.fn>).mockImplementation(
      (
        selector?: (s: {
          active: ActiveMission | null;
          currentRepoCwd: string | null;
        }) => unknown,
      ) => {
        const state = {
          active: {
            ...FULL_MISSION,
            lifecycle: "reverted" as const,
            restoredSha: "abc1234",
          },
          currentRepoCwd: "/repo",
        };
        return selector ? selector(state) : state;
      },
    );

    render(<MissionInbox />);

    const badge = screen.getByLabelText("Mission status: reverted");
    expect(badge.classList.contains("mission-inbox-status-badge--discarded")).toBe(
      true,
    );
    expect(badge.classList.contains("mission-inbox-status-badge--running")).toBe(
      false,
    );
  });

  it("back button transitions to inbox surface", () => {
    render(<MissionInbox />);
    fireEvent.click(screen.getByRole("button", { name: /back/i }));
    expect(useSurfaceStore.getState().surface).toBe("inbox");
  });

  // C. Empty state only when both the active slice AND the surface
  // detail are null.
  it("renders empty placeholder when active mission is null and detail is null", () => {
    useSurfaceStore.setState({ surface: "inbox", detail: null });
    (useMissionsStore as unknown as ReturnType<typeof vi.fn>).mockImplementation(
      (
        selector?: (s: {
          active: ActiveMission | null;
          currentRepoCwd: string | null;
        }) => unknown,
      ) => {
        const state = { active: null, currentRepoCwd: null };
        return selector ? selector(state) : state;
      },
    );
    render(<MissionInbox />);
    expect(screen.getByText(/no mission selected/i)).toBeTruthy();
  });

  // A. Historical row is rendered when active mission id differs.
  it("renders historical mission card when active mission id does not match", () => {
    const row = {
      mission_id: "msn-OLD",
      audit_overall: 0.91,
      tier: "standard",
      created_at: "2026-04-01T10:00:00Z",
      reverted: false,
      status: "discarded" as const,
      target_ref: "main",
      repo_root: "/repo",
      artifacts_cleaned: false,
    };
    useSurfaceStore.setState({
      surface: "mission_detail",
      detail: { missionId: "msn-OLD", row },
    });
    (useMissionsStore as unknown as ReturnType<typeof vi.fn>).mockImplementation(
      (
        selector?: (s: {
          active: ActiveMission | null;
          currentRepoCwd: string | null;
        }) => unknown,
      ) => {
        const state = { active: null, currentRepoCwd: null };
        return selector ? selector(state) : state;
      },
    );
    render(<MissionInbox />);
    expect(screen.getByText("msn-OLD")).toBeTruthy();
    expect(screen.queryByText(/no mission selected/i)).toBeNull();
  });

  // B. Active rich detail is preserved when ids match.
  it("renders rich detail (not historical chrome) when active mission id matches", () => {
    // Default beforeEach already wires mission-1 active + detail.
    const { container } = render(<MissionInbox />);
    // Rich-detail markers
    expect(container.querySelectorAll(".subtask-row")).toHaveLength(2);
    expect(screen.getByText(/Context truncated/i)).toBeTruthy();
    // Historical-card chrome would have aria-label="Mission detail
    // (historical)" — confirm the active path is taken instead.
    expect(
      container.querySelector('[aria-label="Mission detail (historical)"]'),
    ).toBeNull();
  });

  // D. Revert button visibility on historical rows.
  it("shows Revert button on a non-reverted historical row", () => {
    const row = {
      mission_id: "msn-OLD",
      audit_overall: 0.91,
      tier: "standard",
      created_at: "2026-04-01T10:00:00Z",
      reverted: false,
      status: "merged" as const,
      target_ref: "release/v1",
      repo_root: "/repo",
      artifacts_cleaned: false,
    };
    useSurfaceStore.setState({
      surface: "mission_detail",
      detail: { missionId: "msn-OLD", row },
    });
    (useMissionsStore as unknown as ReturnType<typeof vi.fn>).mockImplementation(
      (
        selector?: (s: {
          active: ActiveMission | null;
          currentRepoCwd: string | null;
        }) => unknown,
      ) => {
        const state = { active: null, currentRepoCwd: null };
        return selector ? selector(state) : state;
      },
    );
    render(<MissionInbox />);
    fireEvent.click(screen.getByRole("button", { name: /revert mission/i }));
    expect(
      screen.getByText("vigla/revert/msn-OLD/before/release/v1"),
    ).toBeTruthy();
  });

  it("hides Revert button + shows Reverted pill on a reverted historical row", () => {
    const row = {
      mission_id: "msn-OLD",
      audit_overall: 0.91,
      tier: "standard",
      created_at: "2026-04-01T10:00:00Z",
      reverted: true,
      status: "merged" as const,
      target_ref: "main",
      repo_root: "/repo",
      artifacts_cleaned: false,
    };
    useSurfaceStore.setState({
      surface: "mission_detail",
      detail: { missionId: "msn-OLD", row },
    });
    (useMissionsStore as unknown as ReturnType<typeof vi.fn>).mockImplementation(
      (
        selector?: (s: {
          active: ActiveMission | null;
          currentRepoCwd: string | null;
        }) => unknown,
      ) => {
        const state = { active: null, currentRepoCwd: null };
        return selector ? selector(state) : state;
      },
    );
    render(<MissionInbox />);
    expect(screen.queryByRole("button", { name: /revert mission/i })).toBeNull();
    expect(screen.getByText(/^REVERTED$/)).toBeTruthy();
  });

  it("hides Revert for audited, discarded, and aborted history rows", () => {
    for (const status of ["audited", "discarded", "aborted"] as const) {
      useSurfaceStore.setState({
        surface: "mission_detail",
        detail: {
          missionId: `msn-${status}`,
          row: {
            mission_id: `msn-${status}`,
            audit_overall: 0.91,
            tier: "standard",
            created_at: "2026-04-01T10:00:00Z",
            reverted: false,
            status,
            target_ref: status === "audited" ? null : "main",
            repo_root: status === "audited" ? null : "/repo",
            artifacts_cleaned: false,
          },
        },
      });
      const view = render(<MissionInbox />);
      expect(screen.queryByRole("button", { name: /revert mission/i })).toBeNull();
      view.unmount();
    }
  });

  it("offers cleanup only for an aborted row with retained artifacts", () => {
    const row = {
      mission_id: "msn-aborted",
      audit_overall: 0,
      tier: "smoke",
      created_at: "2026-07-21T10:00:00Z",
      reverted: false,
      status: "aborted" as const,
      target_ref: "main",
      repo_root: "/repo",
      artifacts_cleaned: false,
    };
    useSurfaceStore.setState({
      surface: "mission_detail",
      detail: { missionId: row.mission_id, row },
    });
    (useMissionsStore as unknown as ReturnType<typeof vi.fn>).mockImplementation(
      (selector?: (s: { active: ActiveMission | null }) => unknown) => {
        const state = { active: null };
        return selector ? selector(state) : state;
      },
    );

    const view = render(<MissionInbox />);
    expect(
      screen.getByRole("button", { name: /clean up mission artifacts/i }),
    ).toBeTruthy();
    view.unmount();

    useSurfaceStore.setState({
      surface: "mission_detail",
      detail: {
        missionId: row.mission_id,
        row: { ...row, artifacts_cleaned: true },
      },
    });
    render(<MissionInbox />);
    expect(
      screen.queryByRole("button", { name: /clean up mission artifacts/i }),
    ).toBeNull();
    expect(screen.getByRole("status")).toHaveTextContent("Artifacts cleaned.");
  });

  it("does not let a local snapshot supply a missing durable rollback target", () => {
    window.localStorage.setItem(
      "vigla.missionTrustSnapshots.v1",
      JSON.stringify({
        order: ["msn-legacy"],
        byId: {
          "msn-legacy": {
            missionId: "msn-legacy",
            title: "Legacy mission",
            lifecycle: "merged",
            startedAt: "2026-04-01T10:00:00Z",
            updatedAt: "2026-04-01T10:05:00Z",
            statusLine: "Mission merged",
            summary: null,
            audit: null,
            auditPayloadJson: null,
            verdict: null,
            testsLabel: "no test data",
            filesChanged: 0,
            changedFiles: [],
            integratedCount: 0,
            taskCount: 0,
            tasks: [],
            targetRef: "main",
            rollbackAnchor: "vigla/revert/msn-legacy/before/main",
            resolution: { type: "merged" },
            storedAt: "2026-04-01T10:05:00Z",
          },
        },
      }),
    );
    useSurfaceStore.setState({
      surface: "mission_detail",
      detail: {
        missionId: "msn-legacy",
        row: {
          mission_id: "msn-legacy",
          audit_overall: 0.91,
          tier: "standard",
          created_at: "2026-04-01T10:00:00Z",
          reverted: false,
          status: "merged",
          target_ref: null,
          repo_root: null,
          artifacts_cleaned: false,
        },
      },
    });
    (useMissionsStore as unknown as ReturnType<typeof vi.fn>).mockImplementation(
      (
        selector?: (s: {
          active: ActiveMission | null;
          currentRepoCwd: string | null;
        }) => unknown,
      ) => {
        const state = { active: null, currentRepoCwd: "/repo" };
        return selector ? selector(state) : state;
      },
    );

    render(<MissionInbox />);

    expect(screen.getByText("Legacy mission")).toBeTruthy();
    expect(screen.queryByRole("button", { name: /revert mission/i })).toBeNull();
  });
});
