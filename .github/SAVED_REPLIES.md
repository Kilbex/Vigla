# Maintainer saved replies

GitHub saved replies are account-scoped, so this file is the versioned source
maintainers copy into their personal settings. Adjust the greeting, not the
technical request.

## Needs a reproduction

Thanks for the report. I cannot reproduce this from the current description.
Please add the smallest exact sequence that triggers it, the Vigla commit, your
macOS and tool versions, and a short redacted log excerpt showing the first
wrong result. If it reproduces with the mock harness, include the scenario
name. Please do not post credentials or private repository content.

## Confirmed and tracking

Good catch—I reproduced this on the current `main` branch and marked it
`triage/accepted`. The issue now names the failing boundary and the regression
test a fix must make green. A focused PR is welcome; comment before starting so
work is not duplicated.

## Adapter contribution welcome

This fits the adapter boundary: vendor bytes in, canonical events out, with no
process spawning in the adapter crate. A PR should add a redacted golden
fixture, the expected event assertions, and pass the adapter plus conformance
suites. Start with the relevant crate README and `CONTRIBUTING.md`; no live
vendor credentials should be required for the test.

## Roadmap redirect

The use case is valid, but it is larger than a reviewable issue-sized change.
I linked it to the public roadmap specification, where scope, non-goals, and
acceptance criteria are explicit. Please add concrete workflow evidence there;
that signal is more useful than opening parallel feature requests.

## Out of Vigla scope

This behavior belongs to the vendor account, billing, or model-output layer
rather than Vigla's adapter or orchestrator. Vigla cannot inspect or change
that service. If you find a reproducible Vigla-side translation or recovery
error, open a new report with the redacted canonical event and expected event.
