// Inbox → Verdict → Revert flow under a real Chromium browser with Tauri IPC
// mocked at the Vite layer.
//
// Layout assertions stay tight on the public CSS class contract
// (`.inbox-overview`, `.mission-history`, `.mission-inbox`,
// `.revert-dialog`, `.revert-dialog-sha`, `.revert-button`,
// `.revert-dialog-confirm`) so a future React refactor that
// preserves the user-visible surface keeps the suite green.

import { test, expect, emitMission, snapshotE2eState } from "./fixtures";

const MISSION_ID = "msn-e2e-0001";
const ROLLBACK_ANCHOR = "vigla/revert/msn-e2e-0001/before/main";

function envelope(seq: number, type: string, payload: unknown): Record<string, unknown> {
  return {
    mission_id: MISSION_ID,
    seq,
    ts: new Date(2026, 4, 20, 14, seq, 0).toISOString(),
    type,
    payload,
  };
}

function missionSpec() {
  return {
    title: "Browser acceptance mission",
    objective: "Verify the Inbox → Verdict → Revert flow under Playwright.",
    target_ref: "main",
    tests: null,
    supervisor_model: null,
    worker_model: null,
    worker_count: 1,
  };
}

function verdictPayloadJson(): string {
  return JSON.stringify({
    all_subtasks_accepted: true,
    integrated_test_pass: { passed: 12, failed: 0 },
    residual_risk: "low",
    doc_coverage: 1.0,
    unresolved_issues: [],
    recommendation: {
      kind: "accept",
      audit: {
        overall: 0.94,
        test_pass: { passed: 12, failed: 0 },
        scope: null,
        regression: null,
        lint: null,
        security_flags: [],
      },
      summary: "All subtasks accepted; risk low.",
    },
  });
}

function auditPayloadJson(): string {
  return JSON.stringify({
    overall: 0.94,
    test_pass: { ran: true, passed: 12, failed: 0, skipped: 0, score: 0.96 },
    scope: null,
    regression: null,
    lint: null,
    security_flags: [],
  });
}

async function hydrateMission(page: import("@playwright/test").Page) {
  await emitMission(page, envelope(1, "mission.created", { spec: missionSpec() }));
  await emitMission(
    page,
    envelope(2, "supervisor.decomposition", {
      tasks: [
        {
          index: 0,
          title: "Implement",
          depends_on: [],
          role: "implementer",
          scope_paths: [],
        },
      ],
    }),
  );
  await emitMission(page, envelope(3, "mission.execution_started", {}));
  await emitMission(
    page,
    envelope(4, "worker.spawned", {
      worker_id: "wkr-1",
      task_index: 0,
      task_title: "Implement",
    }),
  );
  await emitMission(
    page,
    envelope(5, "supervisor.integrated", {
      worker_id: "wkr-1",
      integration_sha: "abc1234deadbeef",
      snapshot_tag: "vigla/snap/msn-e2e-0001/0",
    }),
  );
  await emitMission(
    page,
    envelope(6, "supervisor.audit_completed", {
      tier: "standard",
      overall: 0.94,
      payload_json: auditPayloadJson(),
    }),
  );
  await emitMission(
    page,
    envelope(7, "mission.completed", {
      summary: "Mission complete (E2E synthetic).",
      files_changed: 3,
    }),
  );
  await emitMission(
    page,
    envelope(8, "mission.merge_resolved", {
      resolution: { type: "merged" },
    }),
  );

  // MissionOverlay's full-screen backdrop covers the right rail
  // for terminal-state missions. The product flow dismisses it
  // with Esc → store.reset(), which also clears the active
  // mission — and MissionInbox reads from the active slot. For
  // the E2E we keep the active mission populated and hide the
  // overlay via CSS so clicks reach the history rows /
  // MissionInbox underneath. This is a test-only escape hatch;
  // production user clicks the overlay first.
  await page.addStyleTag({
    content:
      ".mission-overlay { display: none !important; } .mission-overlay__backdrop { display: none !important; }",
  });
}

test.describe("Inbox, verdict, and revert", () => {
  test("default surface renders inbox overview, not comms feed", async ({
    e2ePage: page,
  }) => {
    await expect(page.locator(".inbox-overview")).toBeVisible();
    await expect(page.locator(".comms-feed")).toHaveCount(0);
  });

  test("Meta+3 opens mission history; row click opens mission inbox", async ({
    e2ePage: page,
  }) => {
    // Hydrate an active mission so MissionInbox has a verdict to
    // render once we surface it.
    await hydrateMission(page);

    // Open history. Playwright's Meta is ⌘ on macOS; on Linux/CI it
    // maps to the Win key, which the keyboard.ts handler also
    // accepts (e.metaKey || e.ctrlKey).
    await page.keyboard.press("Meta+3");
    await expect(page.locator(".mission-history")).toBeVisible();

    // The mocked list_recent_missions returns one row; click it.
    const row = page.locator(".mission-history-row").first();
    await expect(row).toBeVisible();
    await row.click();

    await expect(page.locator(".mission-inbox")).toBeVisible();
  });

  test("verdict / audit sections render after completion_verdict_rendered", async ({
    e2ePage: page,
  }) => {
    await hydrateMission(page);
    await emitMission(
      page,
      envelope(9, "mission.completion_verdict_rendered", {
        payload_json: verdictPayloadJson(),
      }),
    );

    // Navigate to mission detail via Meta+3 → row click. Use the
    // surface store directly to avoid coupling this assertion to
    // the keyboard handler (covered by the previous test).
    await page.evaluate((id) => {
      const store = (window as any).__zustandSurfaceStore;
      // Fallback: drive through the keyboard + click path.
      void id;
    }, MISSION_ID);
    await page.keyboard.press("Meta+3");
    await page.locator(".mission-history-row").first().click();

    await expect(page.locator(".mission-inbox")).toBeVisible();
    // Risk band badge, audit breakdown, subtask list all present.
    await expect(page.locator(".mission-inbox")).toContainText("Audit breakdown");
    await expect(page.locator(".mission-inbox")).toContainText("Subtasks");
    await expect(page.locator(".mission-inbox")).toContainText("Unresolved issues");
  });

  test("revert button opens dialog with rollback anchor and calls revert_mission", async ({
    e2ePage: page,
  }) => {
    await hydrateMission(page);
    await emitMission(
      page,
      envelope(9, "mission.completion_verdict_rendered", {
        payload_json: verdictPayloadJson(),
      }),
    );

    await page.keyboard.press("Meta+3");
    await page.locator(".mission-history-row").first().click();
    await expect(page.locator(".mission-inbox")).toBeVisible();

    // The terminal overlay intentionally exposes the same action, but this
    // scenario is exercising the History detail surface underneath it.
    const revertBtn = page.locator(".mission-inbox .revert-button");
    await expect(revertBtn).toBeVisible();
    await revertBtn.click();

    const dialog = page.locator(".revert-dialog");
    await expect(dialog).toBeVisible();
    await expect(page.locator(".revert-dialog-sha")).toContainText(ROLLBACK_ANCHOR);

    await page.locator(".revert-dialog-confirm").click();

    await expect
      .poll(async () => {
        const state = await snapshotE2eState(page);
        return state.revertCalls.length;
      })
      .toBeGreaterThan(0);

    const state = await snapshotE2eState(page);
    expect(state.revertCalls[0].missionId).toBe(MISSION_ID);
    const invoke = state.invokeCalls.find((call) => call.cmd === "revert_mission");
    expect(invoke?.args).toEqual({ missionId: MISSION_ID });
  });

  test("terminal verdict fires surface_inbox_notification when window hidden", async ({
    e2ePage: page,
  }) => {
    await hydrateMission(page);

    // Force `document.visibilityState = "hidden"` for the page
    // context. The ingest reducer reads this synchronously when
    // building the banner-emit decision.
    await page.evaluate(() => {
      Object.defineProperty(document, "visibilityState", {
        configurable: true,
        get: () => "hidden",
      });
      Object.defineProperty(document, "hidden", {
        configurable: true,
        get: () => true,
      });
      document.dispatchEvent(new Event("visibilitychange"));
    });

    await emitMission(
      page,
      envelope(9, "mission.completion_verdict_rendered", {
        payload_json: verdictPayloadJson(),
      }),
    );

    await expect
      .poll(async () => {
        const state = await snapshotE2eState(page);
        return state.notifications.length;
      })
      .toBeGreaterThan(0);
  });
});
