# Supervised-fleet benchmark

Status: planned evidence work for the post-launch v0.2 window. The launch
receipt is intentionally smaller; this study must not reuse its result as if it
were a general productivity claim.

## Question

For repository tasks with reviewable tests, how does one coding agent compare
with a supervised three-worker Vigla mission on completion, defects caught
before merge, wall-clock time, and vendor-reported cost?

## Pre-registration

Before running any paid model:

- select 20 tasks from a public, redistributable benchmark or publish a
  purpose-built fixture set with licenses;
- freeze task inclusion and exclusion rules;
- record repository commits, tool versions, model identifiers, prompts,
  budgets, retry limits, and machine specifications;
- define success from tests plus a blinded patch review, not self-report;
- define how timeouts, infrastructure failures, and unusable outputs count;
- commit the analysis script before inspecting aggregate results.

## Conditions

Each task runs in both conditions from the same clean repository state:

1. one agent with the full task and the same maximum spend/time envelope;
2. one Vigla mission with three workers and the supervisor audit enabled.

Randomize condition order. Use fresh vendor sessions. If total budget cannot be
matched exactly, report the difference and normalize nothing silently.

## Measures

- task completion rate;
- integrated test pass rate;
- defects found by the supervisor before merge;
- defects missed and found by blinded review;
- wall-clock time to accepted or terminal outcome;
- input/output tokens and reported USD cost by role;
- retry, scrub, escalation, and revert counts;
- operator interventions.

Publish per-task rows, not only averages. Show Vigla losses, timeouts, and
outliers. Confidence intervals and effect sizes matter more than a headline
percentage on a 20-task sample.

## Reproducibility contract

The future `benches/missions/` harness must:

- run fixtures without modifying their source definitions;
- write an immutable event log and machine-readable result per run;
- redact secrets without deleting failure classifications;
- separate raw vendor output from publishable canonical events;
- regenerate every table and chart from committed analysis code;
- let a stranger substitute their own credentials and models.

## Acceptance gate

A fresh clone can validate the harness without credentials, and a credentialed
operator can reproduce the published tables from the documented commits. The
write-up states the sample, uncertainty, missing data, model/version drift,
hardware, sponsor conflicts, and every deviation from the pre-registration.
