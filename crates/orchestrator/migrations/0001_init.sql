-- Vigla persistence — initial schema (Step 3).
--
-- Tables:
--   workers : one row per spawned worker (vendor CLI process).
--   tasks   : work items dispatched to workers; depends_on encoded as JSON.
--   events  : append-only canonical event log; payload stored opaquely as
--             JSON so unknown fields survive replay (event-schema.md §6).
--
-- Identifiers are stored as TEXT (UUIDv7 strings; see event-schema.md §1).
-- All timestamps are RFC 3339 strings in UTC with millisecond precision.

CREATE TABLE workers (
    id              TEXT PRIMARY KEY NOT NULL,
    name            TEXT NOT NULL,
    vendor          TEXT NOT NULL,
    cli_binary      TEXT NOT NULL,
    cli_version     TEXT,
    cwd             TEXT NOT NULL,
    model           TEXT,
    spawned_at      TEXT NOT NULL,
    ended_at        TEXT,
    last_state      TEXT NOT NULL DEFAULT 'idle'
);

CREATE TABLE tasks (
    id              TEXT PRIMARY KEY NOT NULL,
    parent_id       TEXT,
    title           TEXT NOT NULL,
    depends_on_json TEXT NOT NULL DEFAULT '[]',
    created_at      TEXT NOT NULL
);

CREATE TABLE events (
    worker_id       TEXT NOT NULL,
    task_id         TEXT,
    seq             INTEGER NOT NULL,
    ts              TEXT NOT NULL,
    type            TEXT NOT NULL,
    payload_json    TEXT NOT NULL,
    schema_version  TEXT NOT NULL,
    PRIMARY KEY (worker_id, seq)
);

-- Replay axes. Worker-scoped replay uses (worker_id, ts); the cross-worker
-- timeline uses ts alone. Task-scoped replay can pull events emitted by
-- multiple workers acting on the same task.
CREATE INDEX idx_events_worker_ts ON events(worker_id, ts);
CREATE INDEX idx_events_task_ts   ON events(task_id, ts);
CREATE INDEX idx_events_ts        ON events(ts);
