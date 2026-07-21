# Browser end-to-end tests

This Playwright suite covers the Inbox → Verdict → Revert flow and the
plan-governance surfaces. It substitutes the Tauri IPC layer with in-process mocks
under `mocks/` so the suite can run on any CI runner without a
Tauri build.

## Why Playwright

Playwright provides a deterministic, headless browser harness that contributors
and CI can run without editor-specific browser tooling.

## Layout

```
tests/e2e/
├── README.md                       (this file)
├── playwright.config.ts            launches Vite and browser projects
├── fixtures.ts                     shared `e2ePage` fixture + helpers
├── inbox-verdict-revert.spec.ts    inbox / verdict / revert contracts
├── plan-mode.spec.ts               plan-governance contracts
├── plan-mind-map-visual.spec.ts    plan visual regression
├── web-demo.spec.ts                read-only replay in Chromium/Firefox/WebKit
├── site.spec.ts                    Pages landing + hosted replay contracts
└── mocks/
    ├── tauri-core.ts               invoke + window.__viglaE2e
    ├── tauri-event.ts              listen/once/emit
    └── tauri-webview-window.ts     no-op WebviewWindow shim
```

The Vite alias config (`app/vite.config.ts`) swaps
`@tauri-apps/api/{core,event,webviewWindow}` for these mocks
when `VITE_VIGLA_E2E=1` is set.

## Running

From the repository root:

```sh
pnpm install --frozen-lockfile
pnpm -C app exec playwright install chromium
pnpm -C app run e2e
pnpm -C app run e2e:webdemo
pnpm -C app run e2e:site
```

The Playwright config launches its own Vite dev server on
http://localhost:5180 (no port collision with the default 1420
the Tauri dev cycle uses). The regular suite runs in Chromium; the web-demo
and built Pages contracts run in Chromium, Firefox, and WebKit (the Safari
engine). The Pages preview preserves the repository's `/Vigla/` base path, so
local tests exercise the same asset URLs as GitHub Pages.

## What the spec covers

| Step | Assertion |
|------|-----------|
| Default surface | `.inbox-overview` visible, `.comms-feed` absent |
| Meta+3          | `.mission-history` becomes visible |
| Row click       | `.mission-inbox` becomes visible |
| Verdict render  | Audit / Subtasks / Unresolved-issues sections render |
| Revert button   | `.revert-dialog` opens with snapshot tag in `.revert-dialog-sha` |
| Confirm revert  | `revert_mission` invoke recorded with mission id + cwd |
| Hidden + verdict| `surface_inbox_notification` invoke recorded |

The mock-handle exposed on `window.__viglaE2e` is the
single integration point — any future surface test can drive
events through `emitMissionEvent` / `emitWorkerEvent` and
introspect `invokeCalls` / `notifications` / `revertCalls`.
