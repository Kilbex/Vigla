-- 0011_audit_reports_composite_index.sql
--
-- P1 — speed up list_recent_missions_impl.
--
-- The MissionHistory query runs a multi-CTE pipeline whose inner
-- window function is
--   ROW_NUMBER() OVER (
--     PARTITION BY mission_id
--     ORDER BY (worker_id IS NULL) DESC, created_at DESC
--   )
-- against audit_reports. With only the previous single-column
-- idx_audit_reports_mission, SQLite must do a full per-partition
-- sort to satisfy the ORDER BY, costing a visible "loading…" flash
-- on every History reopen.
--
-- The composite (mission_id, created_at DESC) index lets SQLite
-- satisfy both the partition lookup and the created_at ordering
-- without an external sort. The (worker_id IS NULL) tiebreak is a
-- small in-memory pass over the already-ordered per-mission window.
--
-- idx_audit_reports_mission is kept for compatibility with queries
-- that only filter by mission_id (small storage cost).
CREATE INDEX IF NOT EXISTS idx_audit_reports_mission_created
    ON audit_reports (mission_id, created_at DESC);

-- Update SQLite's planner statistics so the new index is picked
-- on the very first list_recent_missions call after migration.
-- ANALYZE is idempotent and cheap on a small table.
ANALYZE audit_reports;
