# SQLx default pool size is 10 — too high for SQLite

`SqlitePoolOptions::default()` sets `max_connections = 10`, but
SQLite serialises writes through a single file lock so a deep
pool just queues. For Vigla's memory kernel we cap at 5 with
WAL + Normal sync; benchmarks show no throughput delta vs 10 and
significantly lower lock contention on heavy proposal bursts.
