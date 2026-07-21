-- S4: Auto-Integration & Rollback
--
-- mission_revert_log: audit-trail for revert_mission calls. One
-- row per revert. Used to surface "this mission was reverted on X"
-- in the inbox (S3 consumer) and to enforce idempotency: a
-- subsequent revert on the same mission is a no-op.
CREATE TABLE IF NOT EXISTS mission_revert_log (
    mission_id TEXT NOT NULL,
    reverted_at TEXT NOT NULL,
    restored_sha TEXT NOT NULL,
    pre_merge_tag TEXT NOT NULL,
    PRIMARY KEY (mission_id, reverted_at)
);

CREATE INDEX IF NOT EXISTS idx_mission_revert_log_mission
    ON mission_revert_log (mission_id);

-- snapshot_compaction_state: single-row table tracking when the
-- nightly compaction job last ran. The app-start tokio task reads
-- this on boot; if last_run_at is older than 24h, the job runs
-- immediately; otherwise it sleeps until the next 24h boundary.
CREATE TABLE IF NOT EXISTS snapshot_compaction_state (
    id INTEGER PRIMARY KEY CHECK (id = 1),
    last_run_at TEXT NOT NULL,
    last_pruned_count INTEGER NOT NULL DEFAULT 0
);
