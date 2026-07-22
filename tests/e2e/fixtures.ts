// Shared Playwright fixtures. The single `e2ePage` fixture wraps
// `page` with a `goto("/")` and a wait for `window.__viglaE2e`
// — the mock-handle initialised by `tests/e2e/mocks/tauri-core.ts`
// at module-load time. Keeping the wait inside a fixture means
// every spec body can assume the mock plumbing is live.

import { test as base, expect, type Page } from "@playwright/test";

interface E2eState {
  invokeCalls: { cmd: string; args: Record<string, unknown> | undefined }[];
  notifications: { title: string; body: string }[];
  revertCalls: { missionId: string }[];
}

interface E2eFixtures {
  e2ePage: Page;
}

export const test = base.extend<E2eFixtures>({
  // eslint-disable-next-line no-empty-pattern
  e2ePage: async ({ page }, use) => {
    await page.goto("/");
    await page.waitForFunction(
      () => typeof (window as any).__viglaE2e === "object",
    );
    await expect(page.getByTestId("startup-splash")).toBeHidden();
    await use(page);
  },
});

export { expect };

export async function snapshotE2eState(page: Page): Promise<E2eState> {
  return await page.evaluate(() => {
    const h = (window as any).__viglaE2e;
    return {
      invokeCalls: [...h.invokeCalls],
      notifications: [...h.notifications],
      revertCalls: [...h.revertCalls],
    } as E2eState;
  });
}

/** Trigger a mission-event-dto emit from the page context. The
 *  payload should match the on-wire `MissionEvent` envelope
 *  (`{ mission_id, seq, ts, type, payload }`). */
export async function emitMission(
  page: Page,
  payload: Record<string, unknown>,
): Promise<void> {
  await page.evaluate(
    (p) => (window as any).__viglaE2e.emitMissionEvent(p),
    payload,
  );
}

/** Trigger a worker-event emit from the page context. */
export async function emitWorker(
  page: Page,
  payload: Record<string, unknown>,
): Promise<void> {
  await page.evaluate(
    (p) => (window as any).__viglaE2e.emitWorkerEvent(p),
    payload,
  );
}
