# Run Playwright headless in CI with reuse-server

`playwright.config.ts` `webServer.reuseExistingServer = !!CI`
keeps cold-start under 4s. Use `--workers=1` for the Tauri-mock
suite because it asserts global window state. Always pin the
browser channel to chromium (no Firefox/WebKit) since the Tauri
runtime is Chromium-only in production.
