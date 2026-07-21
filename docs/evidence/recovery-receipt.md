# Launch recovery receipt

**27/27 deterministic failure trajectories reached an authority-bound
escalation without exceeding Vigla's default retry policy.** The longest path
took three recovery decisions: two retries, then escalation.

This is a small, falsifiable launch receipt—not a benchmark and not a claim
about real-world agent success rates. It exercises the production classifier
and recovery policy without vendor accounts or network access.

## Reproduce it

From a fresh clone with the pinned Rust toolchain:

```sh
cargo xtask receipt
```

The command runs the 27-case receipt and the mission-workspace atomic-revert
integration test. A passing run prints one `VIGLA_RECOVERY_RECEIPT` JSON line
and verifies that it matches the committed
[machine-readable result](./recovery-receipt.json). The receipt test is also
part of `cargo xtask ci`, so policy changes cannot leave this number stale.

## Method

The fixed case set covers every failure class designed to terminate through an
authority bound under the default policy:

| Failure class | Seeds | Maximum decisions | Terminal bound |
|---|---:|---:|---|
| Missing file | 4 | 2 | Scope |
| Command error (transient and persistent) | 8 | 2 | Quality |
| Merge conflict | 3 | 1 | Quality |
| Permissions | 3 | 1 | Risk |
| Task drift | 3 | 2 | Scope |
| Vendor crash (all six executable vendor profiles) | 6 | 3 | Risk |
| **Total** | **27** | **3** | — |

Twenty-four cases enter through `classify_failure` using seeded dispatch-error
surfaces. Three task-drift cases start from the typed drift result because drift
is emitted by the audit boundary rather than the dispatch-error classifier.
Each case repeatedly calls the same stateful `recover` entry point used by the
mission loop and fails if it:

- exceeds the configured retry count;
- pauses on a terminal-failure case;
- escalates through the wrong authority bound;
- terminates earlier or later than the declared policy; or
- disagrees with the committed JSON result.

The separate revert assertion creates a temporary Git repository, integrates a
worker commit, calls `MissionWorkspace::revert_mission`, and requires the
supervisor branch to equal its exact pre-merge SHA. It is a companion invariant,
not part of the 27-case numerator. This staged-workspace proof is distinct from
the desktop's merged-mission action, which creates a revert commit on the
recorded target branch and preserves later commits.

## Limits

- The receipt is deterministic decision-path evidence. It does not invoke a
  real model, measure task quality, or estimate a production recovery rate.
- `InadequateContext` is excluded because it is an informational request that
  deliberately does not terminate a worker.
- `QuotaExhausted` is excluded because it is a planned pause with an explicit
  reset time, not a terminal failure. Its adapter, persistence, and wake-up path
  have separate integration coverage.
- The denominator is the fixed, public case set in
  [`launch_recovery_receipt.rs`](../../crates/orchestrator/tests/launch_recovery_receipt.rs),
  not a selected subset of a larger hidden run.
