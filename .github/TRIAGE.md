# Issue triage

This runbook keeps launch-week response fast without turning the issue tracker
into an unprioritized backlog.

## Intake

1. Move security reports out of public threads immediately and follow
   [`SECURITY.md`](../SECURITY.md). Do not quote the sensitive content while
   redirecting it.
2. Confirm the report belongs to Vigla rather than a vendor account, billing,
   or model-quality support channel.
3. Reproduce before assigning severity. Ask for one missing fact at a time.
4. Apply exactly one state label and the relevant area labels.

## State labels

| Label | Meaning | Exit condition |
| --- | --- | --- |
| `needs triage` | New and not yet reproduced | Reproduced, redirected, or closed |
| `triage/needs-repro` | Missing a deterministic signal | Reporter supplies a minimal reproduction |
| `triage/accepted` | Maintainer reproduced and accepts the scope | PR merges or issue is deliberately reprioritized |
| `triage/icebox` | Valid, but not in the active roadmap | New evidence changes priority |
| `blocked` | Progress requires a named external decision or state change | Blocker is resolved |

`triage/icebox` is an honest status, not a soft promise. Close duplicates and
link the canonical issue. Close support questions after recording the reusable
answer in Discussions or documentation.

## Severity

- `priority/critical`: data loss, credential exposure, destructive behavior,
  or the supported build cannot start. Stop normal roadmap work.
- `priority/high`: a core mission, audit, merge, or revert path is broken with
  no reasonable workaround.
- `priority/normal`: confirmed defects and scoped improvements that do not
  threaten data or the launch path.

Never infer severity from comment volume. Record the affected boundary,
reproduction, and workaround in the issue before raising priority.

## Pull request linkage

An accepted issue should name the narrowest decisive test. A closing PR must
link the issue, describe regression risk and rollback, and satisfy the full
workflow in [`CONTRIBUTING.md`](../CONTRIBUTING.md#pull-request-workflow).
