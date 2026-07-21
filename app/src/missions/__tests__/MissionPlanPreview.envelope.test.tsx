import { render, screen, fireEvent, waitFor } from "@testing-library/react";
import { beforeEach, describe, expect, it, vi } from "vitest";

vi.mock("../../bindings", () => ({
  commands: {
    confirmPlan: vi.fn(),
    regeneratePlan: vi.fn(),
    rejectPlan: vi.fn(),
  },
}));

import { commands } from "../../bindings";
import MissionPlanPreview from "../MissionPlanPreview";
import type { ActiveMission, EnvelopeFit, MissionTask } from "../types";

const stubTask = (i: number, title: string, deps: number[] = []): MissionTask => ({
  index: i,
  title,
  status: "pending",
  assignedWorkerId: null,
  integrationSha: null,
  snapshotTag: null,
  dependsOn: deps,
});

const baseMission = (
  overrides: Partial<ActiveMission> = {},
): ActiveMission => {
  const mission: ActiveMission = {
    id: "mid-test",
    spec: {
      title: "Add OAuth callback",
      objective: "implement /auth/callback",
      target_ref: "main",
      tests: null,
      supervisor_model: "claude",
      worker_model: null,
      worker_count: null,
      confirm_plan: true,
      scope_paths: [],
    },
    lifecycle: "pending_plan_approval",
    startedAt: "2026-05-24T00:00:00Z",
    updatedAt: "2026-05-24T00:00:00Z",
    statusLine: "Awaiting approval",
    progressPercent: 0,
    tasks: [stubTask(0, "Implement /auth/callback")],
    workers: {},
    testsPassed: null,
    completionSummary: null,
    filesChanged: 0,
    resolution: null,
    abortReason: null,
    attention: [],
    planGeneration: 0,
    planOverview: null,
    planTechStack: null,
    planEnvelopeFit: null,
    supervisorActivity: null,
    lastExtensionDirective: null,
    lastExtensionAt: null,
    audit: null,
    auditPayloadJson: null,
    verdict: null,
    inbox: [],
    ...overrides,
  };
  return mission;
};

describe("MissionPlanPreview QC-3", () => {
  beforeEach(() => {
    vi.mocked(commands.confirmPlan).mockReset();
    vi.mocked(commands.regeneratePlan).mockReset();
    vi.mocked(commands.rejectPlan).mockReset();
  });

  it("renders the envelope panel when planEnvelopeFit is present", () => {
    const ef: EnvelopeFit = {
      scope: { fit: "within", note: "" },
      reversibility: { fit: "exceeds", note: "migration" },
      risk: { fit: "within", note: "" },
      quality: { fit: "within", note: "" },
    };
    render(<MissionPlanPreview mission={baseMission({ planEnvelopeFit: ef })} />);
    expect(screen.getByLabelText(/envelope fit/i)).toBeInTheDocument();
    expect(screen.getByText(/migration/i)).toBeInTheDocument();
  });

  it("renders the trip banner when any bound is exceeds", () => {
    const ef: EnvelopeFit = {
      scope: { fit: "within", note: "" },
      reversibility: { fit: "exceeds", note: "migration" },
      risk: { fit: "within", note: "" },
      quality: { fit: "within", note: "" },
    };
    render(<MissionPlanPreview mission={baseMission({ planEnvelopeFit: ef })} />);
    expect(
      screen.getByText(/plan exceeds the reversibility bound/i),
    ).toBeInTheDocument();
  });

  it("does not render the banner when all bounds are within", () => {
    const ef: EnvelopeFit = {
      scope: { fit: "within", note: "" },
      reversibility: { fit: "within", note: "" },
      risk: { fit: "within", note: "" },
      quality: { fit: "within", note: "" },
    };
    render(<MissionPlanPreview mission={baseMission({ planEnvelopeFit: ef })} />);
    expect(screen.queryByText(/plan exceeds/i)).not.toBeInTheDocument();
  });

  it("renders Abort run alongside Regenerate and Approve plan", () => {
    render(<MissionPlanPreview mission={baseMission()} />);
    expect(screen.getByRole("button", { name: /^abort run$/i })).toBeInTheDocument();
    expect(screen.getByRole("button", { name: /regenerate/i })).toBeInTheDocument();
    expect(
      screen.getByRole("button", { name: /approve plan/i }),
    ).toBeInTheDocument();
  });

  it("opens the inline reject form when Abort run is clicked", () => {
    render(<MissionPlanPreview mission={baseMission()} />);
    fireEvent.click(screen.getByRole("button", { name: /^abort run$/i }));
    expect(screen.getByLabelText(/reject reason/i)).toBeInTheDocument();
  });

  it("invokes commands.rejectPlan from inside the form and closes the form on success", async () => {
    vi.mocked(commands.rejectPlan).mockResolvedValueOnce({
      status: "ok",
      data: null,
    });
    render(<MissionPlanPreview mission={baseMission()} />);

    fireEvent.click(screen.getByRole("button", { name: /^abort run$/i }));
    fireEvent.click(
      screen.getByRole("button", { name: /confirm reject without reason/i }),
    );

    await waitFor(() => {
      expect(commands.rejectPlan).toHaveBeenCalledWith(0, null);
    });
    await waitFor(() => {
      // After success, the form closes and the action row reappears.
      expect(
        screen.getByRole("button", { name: /approve plan/i }),
      ).toBeInTheDocument();
    });
  });

  it("keeps plan decisions locked until the regenerated generation arrives", async () => {
    vi.mocked(commands.regeneratePlan).mockResolvedValueOnce({
      status: "ok",
      data: null,
    });
    const { rerender } = render(
      <MissionPlanPreview mission={baseMission({ planGeneration: 0 })} />,
    );

    fireEvent.click(screen.getByRole("button", { name: /^regenerate$/i }));
    fireEvent.click(
      screen.getByRole("button", { name: /regenerate without feedback/i }),
    );

    await waitFor(() => {
      expect(commands.regeneratePlan).toHaveBeenCalledWith(0, null);
    });
    await waitFor(() => {
      expect(screen.getByRole("button", { name: /approve plan/i })).toBeDisabled();
      expect(screen.getByRole("button", { name: /^regenerate$/i })).toBeDisabled();
      expect(screen.getByRole("button", { name: /^abort run$/i })).toBeDisabled();
    });

    rerender(
      <MissionPlanPreview mission={baseMission({ planGeneration: 1 })} />,
    );
    await waitFor(() => {
      expect(screen.getByRole("button", { name: /approve plan/i })).toBeEnabled();
    });
  });

  it("renders the overview section when planOverview is present", () => {
    render(
      <MissionPlanPreview
        mission={baseMission({ planOverview: "Add a callback handler." })}
      />,
    );
    expect(screen.getByText(/add a callback handler\./i)).toBeInTheDocument();
  });

  it("renders the tech-stack rows with [new] badge on is_new", () => {
    render(
      <MissionPlanPreview
        mission={baseMission({
          planTechStack: [
            { layer: "framework", choice: "Tauri", rationale: "existing", is_new: false },
            { layer: "migrations", choice: "sqlx-cli", rationale: "new", is_new: true },
          ],
        })}
      />,
    );
    expect(screen.getAllByText(/sqlx-cli/i).length).toBeGreaterThan(0);
    expect(screen.getByText(/\[new\]/)).toBeInTheDocument();
  });

  it("shows the soft revision notice once planGeneration reaches 3", () => {
    render(<MissionPlanPreview mission={baseMission({ planGeneration: 3 })} />);
    expect(
      screen.getByText(/regenerated 3 times/i),
    ).toBeInTheDocument();
  });

  it("does NOT show the soft revision notice at generation 0", () => {
    render(<MissionPlanPreview mission={baseMission({ planGeneration: 0 })} />);
    expect(
      screen.queryByText(/regenerated .* times/i),
    ).not.toBeInTheDocument();
  });
});
