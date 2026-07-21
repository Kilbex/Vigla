-- 0006_event_retention.sql
--
-- Bounded event persistence: two-tier retention with a cold archive.
--
-- * events_archive: cold tier mirroring `events` shape. No hot indexes
--   beyond the PK. Trim moves rows here; nothing in the application
--   surface ever inserts back into `events` from this table.
-- * workers.mission_id: nullable foreign key (no enforced FK because
--   missions aren't a persisted entity yet). Index supports
--   "all workers in mission" queries; partial behaviour via
--   `mission_id IS NOT NULL` is automatic.
-- * workers.session_id: existing column (migration 0002); add an index
--   so future "all workers with session X" queries hit an index instead of scanning the workers table.

CREATE TABLE events_archive (
    worker_id       TEXT NOT NULL,
    task_id         TEXT,
    seq             INTEGER NOT NULL,
    ts              TEXT NOT NULL,
    type            TEXT NOT NULL,
    payload_json    TEXT NOT NULL,
    schema_version  TEXT NOT NULL,
    archived_at     TEXT NOT NULL,
    PRIMARY KEY (worker_id, seq)
);

ALTER TABLE workers ADD COLUMN mission_id TEXT;
CREATE INDEX idx_workers_mission_spawned ON workers(mission_id, spawned_at);
CREATE INDEX idx_workers_session         ON workers(session_id);
