-- Canonical repository identity is part of a terminal mission outcome.
-- NULL is retained only for rows written by pre-0017 builds; rollback is
-- deliberately disabled for those legacy rows.
ALTER TABLE mission_outcomes ADD COLUMN repo_root TEXT;
CREATE INDEX IF NOT EXISTS idx_mission_outcomes_repo_root
    ON mission_outcomes(repo_root, updated_at DESC);
