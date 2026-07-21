import { test, expect, snapshotE2eState } from "./fixtures";

async function stationsFitOperationsRoom(
  page: import("@playwright/test").Page,
): Promise<boolean> {
  return page.evaluate(() => {
    const room = document.querySelector(".operations-room")?.getBoundingClientRect();
    const stations = [...document.querySelectorAll(".station")].map((node) =>
      node.getBoundingClientRect(),
    );
    if (!room || stations.length !== 3) return false;
    return stations.every(
      (station) =>
        station.left >= room.left &&
        station.right <= room.right &&
        station.top >= room.top &&
        station.bottom <= room.bottom,
    );
  });
}

test.describe("read-only web replay", () => {
  test("plays, steps, scrubs, and switches between all recorded outcomes", async ({
    e2ePage: page,
  }) => {
    await expect(page.getByRole("region", { name: "Recorded replay demo" })).toBeVisible();
    await expect(page.locator(".app-grid")).toHaveAttribute("inert", "");

    await expect(page.locator(".station")).toHaveCount(3);
    await expect(page.locator(".replay-controls")).toBeVisible();
    // React Flow schedules fitView after its nodes and container settle.
    await expect.poll(() => stationsFitOperationsRoom(page)).toBe(true);

    const playToggle = page.locator(".replay-controls .replay-btn").first();
    if (/pause/i.test((await playToggle.textContent()) ?? "")) {
      await playToggle.click();
    }
    await page.getByRole("button", { name: /rewind/i }).click();
    await expect(page.locator(".replay-target")).toContainText("0 / 19");
    await page.getByRole("button", { name: /step →/i }).click();
    await expect(page.locator(".replay-target")).toContainText("1 / 19");

    const scrubber = page.getByRole("slider", { name: "replay position" });
    await scrubber.fill("19");
    await expect(page.locator(".station--done")).toHaveCount(3);

    await page.getByRole("button", { name: "Bound tripped" }).click();
    await page.getByRole("button", { name: /end →/i }).click();
    await expect(page.locator(".station--failed")).toHaveCount(1);
    await expect(page.locator(".web-demo-outcome")).toContainText(
      "Quality bound · review required",
    );

    await page.getByRole("button", { name: "Quota paused" }).click();
    await page.getByRole("button", { name: /end →/i }).click();
    await expect(page.locator(".station--blocked")).toHaveCount(1);
    await expect(page.locator(".web-demo-outcome")).toContainText(
      "Work preserved · timed resume",
    );

    const state = await snapshotE2eState(page);
    expect(
      state.invokeCalls.some((call) =>
        /^(start_|continue_worker|retry_worker|stop_worker)/.test(call.cmd),
      ),
    ).toBe(false);
  });

  test("keeps the recorded mission usable at a narrow viewport", async ({ page }) => {
    await page.setViewportSize({ width: 390, height: 844 });
    await page.goto("/");
    await page.waitForFunction(
      () => typeof (window as any).__viglaE2e === "object",
    );
    await expect(page.locator(".station")).toHaveCount(3);

    const readLayout = () =>
      page.evaluate(() => {
        const controls = document
          .querySelector(".replay-controls")
          ?.getBoundingClientRect();
        return {
          noPageOverflow: document.documentElement.scrollWidth <= window.innerWidth,
          controlsVisible:
            controls != null && controls.left >= 0 && controls.right <= window.innerWidth,
        };
      });

    await expect.poll(readLayout).toEqual({
      noPageOverflow: true,
      controlsVisible: true,
    });
    await expect.poll(() => stationsFitOperationsRoom(page)).toBe(true);
    await expect(page.locator(".replay-controls .replay-btn:visible")).toHaveCount(4);
  });
});
