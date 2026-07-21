-- 0014_audit_reports_dedupe_mission_level.sql
--
-- F-9: audit_reports' PRIMARY KEY is (mission_id, worker_id, created_at) and
-- worker_id is nullable. SQLite treats NULLs as DISTINCT in PRIMARY KEY and
-- UNIQUE constraints, so the PK does NOT prevent duplicate *mission-level*
-- rows (worker_id IS NULL) sharing the same (mission_id, created_at) — they
-- accumulate silently (a plain INSERT with no conflict handling). Worker-level
-- rows (worker_id NOT NULL) are already protected by the PK.
--
-- worker_id stays nullable on purpose: mission-level audits are identified by
-- `worker_id IS NULL` throughout the codebase (e.g. list_recent_missions orders
-- by `(worker_id IS NULL) DESC`). So rather than a sentinel / NOT NULL table
-- rewrite, we (1) drop existing mission-level duplicates and (2) add a partial
-- UNIQUE index to prevent new ones. Paired with `ON CONFLICT DO NOTHING` in
-- audit::persist::insert_audit_at so a re-insert is a silent no-op, not an error.

-- 1. Dedupe existing mission-level duplicates, keeping the earliest (lowest
--    rowid). Required before the unique index can be created.
DELETE FROM audit_reports
WHERE worker_id IS NULL
  AND rowid NOT IN (
    SELECT MIN(rowid)
    FROM audit_reports
    WHERE worker_id IS NULL
    GROUP BY mission_id, created_at
  );

-- 2. Enforce uniqueness for mission-level rows going forward.
CREATE UNIQUE INDEX IF NOT EXISTS idx_audit_reports_mission_level_unique
    ON audit_reports (mission_id, created_at)
    WHERE worker_id IS NULL;
