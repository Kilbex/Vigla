import { defineConfig, devices } from "@playwright/test";
import path from "node:path";

const appDir = path.resolve(__dirname, "..", "..", "app");
const repoDir = path.resolve(appDir, "..");
const webDemo = process.env.VIGLA_E2E_WEB_DEMO === "1";
const site = process.env.VIGLA_E2E_SITE === "1";
const baseURL = site ? "http://127.0.0.1:5190/Vigla/" : "http://127.0.0.1:5180";

// Playwright config for the browser acceptance suite. Vite runs against the
// real `app/` sources with `VITE_VIGLA_E2E=1`, which the
// app-side vite.config.ts uses to alias `@tauri-apps/api/{core,
// event,webviewWindow}` to the in-process mocks under
// `tests/e2e/mocks/`. No Tauri runtime, no orchestrator — the
// suite asserts the UI contracts that the desktop application relies on.
export default defineConfig({
  testDir: __dirname,
  fullyParallel: false,
  reporter: process.env.CI ? "github" : "list",
  use: {
    baseURL,
    trace: "retain-on-failure",
  },
  // The landing and browser-replay specs need different build modes and
  // servers. Their dedicated scripts pass the file explicitly; the default
  // desktop-UI suite must not discover them under the ordinary Vite server.
  testIgnore: webDemo || site ? [] : ["**/site.spec.ts", "**/web-demo.spec.ts"],
  webServer: site
    ? {
        command: "node scripts/serve-site.mjs",
        cwd: repoDir,
        url: baseURL,
        timeout: 60_000,
        reuseExistingServer: !process.env.CI,
      }
    : {
        command:
          "pnpm exec vite --strictPort --host 127.0.0.1 --port 5180 --clearScreen false",
        cwd: appDir,
        url: baseURL,
        timeout: 60_000,
        reuseExistingServer: !process.env.CI,
        env: {
          VITE_VIGLA_E2E: "1",
          ...(webDemo
            ? { VITE_VIGLA_WEB_DEMO: "1", VITE_VIGLA_BASE: "/" }
            : {}),
        },
      },
  projects: webDemo || site
    ? [
        { name: "chromium", use: { ...devices["Desktop Chrome"] } },
        {
          name: "firefox",
          use: {
            ...devices["Desktop Firefox"],
            launchOptions: { timeout: 30_000 },
          },
        },
        { name: "webkit", use: { ...devices["Desktop Safari"] } },
      ]
    : [
        {
          name: "chromium",
          use: { ...devices["Desktop Chrome"] },
        },
      ],
});
