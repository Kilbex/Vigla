# GitHub traffic snapshots

GitHub exposes repository views, clones, popular paths, and referrers for a
rolling 14-day window. Vigla's scheduled collector stores those responses in a
separate private repository before they expire. This is repository operations
measurement only; Vigla does not add product analytics, tracking code, or
telemetry to the desktop app.

## Deploy immediately, before launch announcements

The canonical repository is already public, so the rolling retention clock is
running. Do not wait for the announcement campaign to configure this workflow.

1. Create a private repository dedicated to the archive, for example
   `Kilbex/vigla-private-metrics`. Keep it private: referrer and popular-path
   data should not be committed to the Vigla source repository. Initialize the
   archive with a README so it has a default branch for Actions to check out.
2. Create two narrowly scoped fine-grained personal access tokens:
   - A source token selected only for `Kilbex/Vigla`, with
     **Administration: read** for GitHub's traffic endpoints.
   - An archive token selected only for the private metrics repository, with
     **Contents: read and write**.
3. Add these Actions secrets to `Kilbex/Vigla`:
   - `VIGLA_METRICS_REPOSITORY`: the archive in `owner/name` form.
   - `VIGLA_TRAFFIC_READ_TOKEN`: the source token from step 2.
   - `VIGLA_METRICS_WRITE_TOKEN`: the archive token from step 2.
4. Open **Actions → Traffic snapshot → Run workflow**. Confirm that the run
   writes `data/traffic-YYYY.jsonl`, `reports/YYYY-MM-DD.md`, and
   `reports/latest.md` to the private archive.
5. Run it again on the following UTC day. Confirm the second JSONL record is
   present and the seven-day report does not double-count overlapping API data.

The workflow then runs daily at 08:17 UTC. Its git commit identity is a generic
noreply bot address; it does not embed a maintainer's name or email.

## Local verification

The collector is dependency-free on Node 22:

```sh
node --test scripts/github-traffic.test.mjs

GITHUB_TOKEN=github_pat_… node scripts/github-traffic.mjs snapshot \
  --repo Kilbex/Vigla \
  --output ./private-metrics/data/traffic-$(date -u +%Y).jsonl

node scripts/github-traffic.mjs summary \
  --input-dir ./private-metrics/data \
  --output ./private-metrics/reports/latest.md
```

Each snapshot is fetched in full before one JSONL record is appended. Reports
merge daily points by timestamp, keep the newest observation, and report seven
consecutive UTC calendar days, preventing overlapping or sparse rolling windows
from inflating totals. Referrer and path tables are explicitly labeled as
rolling 14-day responses; release download counts are lifetime counters.
