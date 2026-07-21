-- Snapshot compaction is repository-scoped. The legacy singleton checkpoint
-- could suppress cleanup in every other repository after one repository ran.
-- Keep the old table for upgrade compatibility; new code only uses this map.
CREATE TABLE IF NOT EXISTS snapshot_compaction_state_by_repo (
    repo_root TEXT PRIMARY KEY NOT NULL,
    last_run_at TEXT NOT NULL,
    last_pruned_count INTEGER NOT NULL DEFAULT 0
);
