# Mock @tauri-apps/api/core via vitest aliases

In `vitest.config.ts` add an alias from `@tauri-apps/api/core` to
a local `__mocks__/tauri-core.ts` that exposes a stub `invoke`
returning a `Promise.resolve(undefined)`. Per-test you can
`vi.mocked(invoke).mockResolvedValueOnce(...)` to drive specific
return values. Same pattern works for `@tauri-apps/api/event` —
mock `listen` to return an unsub fn.
