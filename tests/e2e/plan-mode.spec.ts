// Drives the plan-approval surface (MissionPlanPreview) through
// every gate the mission_loop can hit, using the existing
// emitMission helper to feed canned plan.proposed payloads. Avoids
// the scripted-supervisor Rust path; this suite asserts the FE
// behaviour for each plan-mode / envelope-fit combination.

import { test, expect, emitMission } from "./fixtures";

const MID = "msn-e2e-plan-0001";

function envelope(seq: number, type: string, payload: unknown): Record<string, unknown> {
  return {
    mission_id: MID,
    seq,
    ts: new Date(2026, 4, 24, 12, seq, 0).toISOString(),
    type,
    payload,
  };
}

function missionSpec() {
  return {
    title: "Add OAuth callback handler",
    objective: "Implement /auth/callback and migrate the session table.",
    target_ref: "main",
    tests: null,
    supervisor_model: "claude",
    worker_model: null,
    worker_count: 1,
    confirm_plan: true,
    scope_paths: [],
  };
}

const TASKS = [
  { index: 0, title: "Implement /auth/callback handler", depends_on: [] },
  { index: 1, title: "Migrate session table", depends_on: [0] },
];

async function hydrateProposedPlan(
  page: import("@playwright/test").Page,
  envelopeFit: Record<string, unknown> | null,
  overview: string | null = "Add an OAuth callback handler and migrate the session table.",
): Promise<void> {
  await emitMission(page, envelope(1, "mission.created", { spec: missionSpec() }));
  await emitMission(page, envelope(2, "mission.execution_started", null));
  await emitMission(
    page,
    envelope(3, "supervisor.decomposition", { tasks: TASKS }),
  );
  await emitMission(
    page,
    envelope(4, "plan.proposed", {
      tasks: TASKS,
      generation: 0,
      overview,
      tech_stack: [
        { layer: "auth_provider", choice: "Auth0", rationale: "matches existing", is_new: false },
        { layer: "migrations", choice: "sqlx-cli", rationale: "new", is_new: true },
      ],
      envelope_fit: envelopeFit,
    }),
  );
}

test.describe("Mission plan governance", () => {
  test("Review + Within — overlay shows mind map, envelope panel, and three actions", async ({
    e2ePage,
  }) => {
    await hydrateProposedPlan(e2ePage, {
      scope: { fit: "within", note: "all under src/auth/" },
      reversibility: { fit: "within", note: "no schema changes" },
      risk: { fit: "within", note: "no secrets" },
      quality: { fit: "within", note: "tests included" },
    });

    // Mind map renders.
    await expect(e2ePage.getByTestId("plan-mind-map")).toBeVisible();
    // Envelope panel renders.
    await expect(e2ePage.getByLabel(/envelope fit/i)).toBeVisible();
    // No trip banner.
    await expect(e2ePage.getByText(/plan exceeds/i)).toBeHidden();
    // Three actions present.
    await expect(
      e2ePage.getByRole("button", { name: /^reject plan$/i }),
    ).toBeVisible();
    await expect(
      e2ePage.getByRole("button", { name: /^regenerate$/i }),
    ).toBeVisible();
    await expect(
      e2ePage.getByRole("button", { name: /approve plan/i }),
    ).toBeVisible();
  });

  test("Review + Exceeds — overlay surfaces the trip banner naming the bound", async ({
    e2ePage,
  }) => {
    await hydrateProposedPlan(e2ePage, {
      scope: { fit: "within", note: "" },
      reversibility: { fit: "within", note: "" },
      risk: { fit: "exceeds", note: "touches billing endpoint" },
      quality: { fit: "within", note: "" },
    });

    await expect(
      e2ePage.getByText(/risk exceeds the mission envelope/i),
    ).toBeVisible();
    // Banner uses role="alert" so screen readers announce it.
    await expect(e2ePage.locator(".mission-plan-preview__banner")).toBeVisible();
  });

  test("Reject opens the inline form and dispatches commands.rejectPlan", async ({
    e2ePage,
  }) => {
    await hydrateProposedPlan(e2ePage, {
      scope: { fit: "within", note: "" },
      reversibility: { fit: "within", note: "" },
      risk: { fit: "within", note: "" },
      quality: { fit: "within", note: "" },
    });

    await e2ePage.getByRole("button", { name: /^reject plan$/i }).click();
    await expect(e2ePage.getByLabel(/reject reason/i)).toBeVisible();

    await e2ePage
      .getByLabel(/reject reason/i)
      .fill("scope too broad");
    await e2ePage
      .getByRole("button", { name: /^confirm reject$/i })
      .click();

    // The mock invoke records the reject_plan call.
    await e2ePage.waitForFunction(() => {
      const handle = (window as unknown as {
        __viglaE2e: { invokeCalls: { cmd: string }[] };
      }).__viglaE2e;
      return handle.invokeCalls.some((c) => c.cmd === "reject_plan");
    });
  });

  test("Legacy plan with no envelope_fit still renders mind map + actions", async ({
    e2ePage,
  }) => {
    // envelope_fit == null mirrors a legacy supervisor adapter. The
    // plan-preview surface still works.
    await hydrateProposedPlan(e2ePage, null, null);

    await expect(e2ePage.getByTestId("plan-mind-map")).toBeVisible();
    // No envelope panel — the component returns null when given null.
    await expect(e2ePage.getByLabel(/envelope fit/i)).toBeHidden();
    // No trip banner — there's nothing to trip.
    await expect(e2ePage.getByText(/plan exceeds/i)).toBeHidden();
    // Actions still present.
    await expect(
      e2ePage.getByRole("button", { name: /^reject plan$/i }),
    ).toBeVisible();
    await expect(
      e2ePage.getByRole("button", { name: /approve plan/i }),
    ).toBeVisible();
  });
});
