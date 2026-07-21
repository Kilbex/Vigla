# Set up jsdom plus testing-library for component tests

`vitest.config.ts` `test.environment = "jsdom"`. Add the
`testing-library jest-dom` package to `setupFiles` so matchers like
`toBeInTheDocument()` are available. React 19's `act` no longer
needs to wrap state updates; the warnings are spurious — silence
them via `setupFiles`. Component tests run in 3s for ~400 cases.
