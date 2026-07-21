import { render, screen } from "@testing-library/react";
import { describe, it, expect } from "vitest";
import PlanTab from "../PlanTab";
import type { ActiveMission, EnvelopeFit, MissionTask } from "../../missions/types";

const stubTask = (i: number, title: string): MissionTask => ({
  index: i,
  title,
  status: "pending",
  assignedWorkerId: null,
  integrationSha: null,
  snapshotTag: null,
  dependsOn: [],
});

const stubMission = (
  overrides: Partial<ActiveMission> = {},
): ActiveMission => ({
  id: "mid-test",
  spec: {
    title: "Add OAuth",
    objective: "implement /auth/callback",
    target_ref: "main",
    tests: null,
    supervisor_model: "claude",
    worker_model: null,
    worker_count: null,
    confirm_plan: true,
    scope_paths: [],
  },
  lifecycle: "executing",
  startedAt: "2026-05-24T00:00:00Z",
  updatedAt: "2026-05-24T00:00:00Z",
  statusLine: "Workers running",
  progressPercent: 25,
  tasks: [stubTask(0, "Implement")],
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
});

describe("PlanTab", () => {
  it("renders the mind map and the overview when present", () => {
    render(
      <PlanTab
        mission={stubMission({ planOverview: "Plain summary of plan." })}
      />,
    );
    expect(screen.getByTestId("plan-mind-map")).toBeInTheDocument();
    expect(screen.getByText(/plain summary of plan\./i)).toBeInTheDocument();
  });

  it("renders the envelope panel when envelope_fit is present", () => {
    const ef: EnvelopeFit = {
      scope: { fit: "within", note: "" },
      reversibility: { fit: "exceeds", note: "migration" },
      risk: { fit: "within", note: "" },
      quality: { fit: "within", note: "" },
    };
    render(<PlanTab mission={stubMission({ planEnvelopeFit: ef })} />);
    expect(screen.getByLabelText(/envelope fit/i)).toBeInTheDocument();
    expect(screen.getByText(/migration/i)).toBeInTheDocument();
  });

  it("omits the overview section when planOverview is null", () => {
    const { container } = render(<PlanTab mission={stubMission()} />);
    expect(
      container.querySelector(".plan-tab__overview"),
    ).not.toBeInTheDocument();
  });

  it("renders the tech_stack with [new] badge on is_new", () => {
    render(
      <PlanTab
        mission={stubMission({
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
});
