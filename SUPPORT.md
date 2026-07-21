# Support

Vigla is maintained in public. Choose the route that preserves context and
gets the right kind of response.

| You need | Use | Do not include |
| --- | --- | --- |
| A reproducible bug fixed | [Bug report](https://github.com/Kilbex/Vigla/issues/new?template=bug_report.yml) | Credentials, raw unredacted transcripts, private repository content |
| Help using or understanding Vigla | [GitHub Discussions Q&A](https://github.com/Kilbex/Vigla/discussions/categories/q-a) | Security vulnerabilities |
| A focused product improvement | [Feature request](https://github.com/Kilbex/Vigla/issues/new?template=feature_request.yml) | Several unrelated requests in one issue |
| A vendor-adapter change | [Adapter task](https://github.com/Kilbex/Vigla/issues/new?template=adapter_task.yml) | Live access tokens or session files |
| A security vulnerability | [Security policy](./SECURITY.md) | A public issue or Discussion |
| Help contributing code | [Contributor guide](./CONTRIBUTING.md) | Unreviewed generated changes without a test plan |

## Before filing a bug

Search existing issues, then reduce the report to the smallest deterministic
case you can. Include:

- the Vigla commit or source tag;
- macOS version and CPU architecture;
- `rustc --version`, `node --version`, and `pnpm --version`;
- relevant vendor CLI names and versions, without auth state or secrets;
- the exact command or UI sequence;
- expected and actual behavior;
- the shortest redacted log excerpt that proves the failure.

The credential-free mock harness is the preferred reproduction path. If the
problem reproduces there, say which scenario you used. If it only happens with
a real CLI, state that explicitly and replace private paths, prompts, and
output with minimal placeholders.

## Response expectations

During the initial launch window, maintainers aim to triage actionable reports
within one business day. This is a response target, not an uptime or fix-time
guarantee. Confirmed bugs receive a severity and next action; incomplete
reports receive one request for the missing reproduction details and may be
closed if those details never arrive.

GitHub Discussions is the searchable record for reusable answers. Chat may be
added as a casual community entrance, but decisions and support answers that
others will need should be mirrored back to GitHub.

## Scope boundaries

Vigla does not provide support for vendor billing, vendor account recovery,
the correctness of third-party model output, or proprietary repository code.
It does support its adapter behavior, orchestration, audit and recovery logic,
local persistence, frontend, and source-build recipe.
