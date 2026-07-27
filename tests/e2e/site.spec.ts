import { expect, test } from "@playwright/test";

test.describe("GitHub Pages launch surface", () => {
  test("loads the landing page and read-only replay without external requests", async ({
    page,
  }) => {
    const consoleErrors: string[] = [];
    const pageErrors: string[] = [];
    const requestedOrigins = new Set<string>();
    page.on("console", (message) => {
      if (message.type() === "error") consoleErrors.push(message.text());
    });
    page.on("pageerror", (error) => pageErrors.push(error.message));
    page.on("request", (request) => requestedOrigins.add(new URL(request.url()).origin));

    const response = await page.goto("/Vigla/", { waitUntil: "networkidle" });
    expect(response?.ok()).toBe(true);
    await expect(page).toHaveTitle("Vigla — Open-source mission control for coding agents");
    await expect(page.locator('link[rel="canonical"]')).toHaveAttribute(
      "href",
      "https://kilbex.github.io/Vigla/",
    );
    await expect(page.locator('meta[property="og:image"]')).toHaveAttribute(
      "content",
      "https://kilbex.github.io/Vigla/media/social-preview.png",
    );
    await expect(page.locator('meta[property="og:image:alt"]')).toHaveAttribute(
      "content",
      /four isolated coding-agent work lines/i,
    );
    await expect(page.locator('meta[name="twitter:image:alt"]')).toHaveAttribute(
      "content",
      /coding-agent work lines/i,
    );
    const structuredData = JSON.parse(
      (await page.locator('script[type="application/ld+json"]').textContent()) ?? "{}",
    );
    const software = structuredData["@graph"]?.find(
      (entry: { "@type"?: string }) => entry["@type"] === "SoftwareApplication",
    );
    expect(software).toMatchObject({
      name: "Vigla",
      applicationCategory: "DeveloperApplication",
      operatingSystem: "macOS 12 or later",
      isAccessibleForFree: true,
      codeRepository: "https://github.com/Kilbex/Vigla",
      offers: { price: "0", priceCurrency: "USD" },
    });
    // The page must not sell the replay as a capture of a real session; the
    // events are hand-authored in app/src/demo/recordings.ts.
    await expect(page.locator(".visual-status .status-live")).toHaveText(
      "Scripted replay · no vendor calls",
    );
    await expect(page.getByRole("heading", { level: 1 })).toContainText(
      "Supervise the merge",
    );
    await expect(page.getByText("27 / 27 seeded paths bounded")).toBeVisible();

    const heroImage = page.locator(".hero-visual img");
    await expect(heroImage).toBeVisible();
    expect(
      await heroImage.evaluate((image: HTMLImageElement) => image.complete && image.naturalWidth > 0),
    ).toBe(true);
    expect(
      await page.evaluate(() => document.documentElement.scrollWidth <= window.innerWidth),
    ).toBe(true);
    expect(
      [...requestedOrigins].every((origin) => origin === "http://127.0.0.1:5190"),
    ).toBe(true);
    expect(
      await page.evaluate(() =>
        performance
          .getEntriesByType("resource")
          .every((entry) => !entry.name.endsWith("/media/vigla-demo.webp")),
      ),
    ).toBe(true);

    await page.getByRole("button", { name: "Play 16-second preview" }).click();
    await expect(heroImage).toHaveAttribute("src", /vigla-demo\.webp$/);
    await expect(page.getByRole("button", { name: "Stop preview" })).toHaveAttribute(
      "aria-pressed",
      "true",
    );
    await page.getByRole("button", { name: "Stop preview" }).click();
    await expect(heroImage).toHaveAttribute("src", /vigla-demo-poster\.webp$/);

    await page.getByRole("link", { name: "Open the 16-second replay" }).click();
    await expect(page).toHaveURL(/\/Vigla\/demo\/$/);
    await expect(page.getByRole("region", { name: "Scripted replay demo" })).toBeVisible();
    await expect(page.locator(".station")).toHaveCount(3);

    expect([...requestedOrigins]).toEqual(["http://127.0.0.1:5190"]);
    expect(consoleErrors).toEqual([]);
    expect(pageErrors).toEqual([]);
  });

  test("stays usable at mobile width and honors reduced motion", async ({ page }) => {
    await page.setViewportSize({ width: 390, height: 844 });
    await page.emulateMedia({ reducedMotion: "reduce" });
    await page.goto("/Vigla/", { waitUntil: "networkidle" });

    const layout = await page.evaluate(() => {
      const actions = [...document.querySelectorAll<HTMLElement>(".hero-actions .button")];
      const heroCopy = document.querySelector<HTMLElement>(".hero-copy");
      return {
        noOverflow: document.documentElement.scrollWidth <= window.innerWidth,
        actionsMeetTouchTarget: actions.every((action) => action.getBoundingClientRect().height >= 44),
        animationIsReduced: heroCopy
          ? Number.parseFloat(getComputedStyle(heroCopy).animationDuration) <= 0.00001
          : false,
      };
    });
    expect(layout.noOverflow).toBe(true);
    expect(layout.actionsMeetTouchTarget).toBe(true);
    expect(layout.animationIsReduced).toBe(true);

    const skipLink = page.getByRole("link", { name: "Skip to content" });
    await skipLink.focus();
    await expect(skipLink).toBeFocused();
    await page.keyboard.press("Enter");
    await expect(page.locator("#main")).toBeFocused();
  });
});
