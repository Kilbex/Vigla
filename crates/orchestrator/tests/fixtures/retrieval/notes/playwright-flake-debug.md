# Headless Chromium clock skew breaks visibility tests

Playwright's `visibilityState = "hidden"` override does not fire a
synchronous `visibilitychange` event in headless Chromium 121+;
you must dispatch the event explicitly with
`window.dispatchEvent(new Event("visibilitychange"))`. Forgetting
this is the #1 cause of "passes locally, fails on CI" in the
e2e/inbox-verdict-revert.spec.ts flow.
