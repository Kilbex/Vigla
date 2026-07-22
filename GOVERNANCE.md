# Governance

Vigla is currently a single-maintainer project. This document makes that
constraint explicit and defines how technical decisions, releases, maintainer
changes, conflicts, and continuity are handled while the community grows.

## Decision making

Routine changes use lazy consensus through public issues and pull requests.
Contributors should state the problem, alternatives considered, verification,
and compatibility impact. The maintainer makes the final call when consensus
does not emerge, with a short written rationale tied to project scope,
security, maintainability, and the published roadmap.

Changes to persisted data, public interfaces, security boundaries, governance,
or supported platforms require a public design issue or decision record before
implementation. Vulnerability details are the exception and follow
[SECURITY.md](SECURITY.md) until coordinated disclosure is safe.

Only maintainers may merge pull requests, create tags, publish releases, or
change repository security settings. A release may be tagged only from a green
protected `main` commit after the documented release gate passes.

## Maintainer lifecycle

Maintainers are added based on a sustained record of sound reviews, respectful
community work, security judgment, and reliable follow-through—not employer,
volume, or popularity. An existing maintainer nominates the candidate in a
public governance issue; active maintainers record the decision and grant the
least repository access needed.

A maintainer may step down at any time. Access may be suspended promptly for a
credible security risk and removed for repeated conduct violations, prolonged
unexplained inactivity, or misuse of project authority. Except for urgent
security containment, the reason and decision are recorded publicly without
publishing private reporter information.

## Conflicts and appeals

Maintainers disclose material conflicts and recuse themselves when another
maintainer can decide. Technical decisions may be appealed once in the original
issue with new evidence; disagreement alone is not misconduct.

While Vigla has only one maintainer, it cannot promise independent internal
adjudication. Reports about GitHub-hosted conduct by that maintainer may be
escalated independently through [GitHub Support's abuse-reporting
channel](https://support.github.com/contact/report-abuse). Sensitive security
reports must never be posted publicly. Adding a second trusted maintainer and a
separate private conduct contact is a launch-governance priority.

## Succession

The maintainer should keep recovery access current and ensure that another
trusted person can be designated before a planned absence. On departure, the
maintainer will transfer the repository and related project-controlled assets
to an active maintainer who accepts these obligations, then remove obsolete
access.

If no transfer is possible, the final project notice should state the archival
status and identify any endorsed successor. The Apache-2.0 license always
permits the community to fork the last public source, but a fork must not imply
control of Vigla's original GitHub identity or release channels.

Governance changes use the same public pull-request process and must update
this document in the release that adopts them.
