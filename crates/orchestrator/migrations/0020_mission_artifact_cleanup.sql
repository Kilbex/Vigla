-- Durable acknowledgement that an aborted mission's retained Git artifacts
-- were explicitly removed. Aborts preserve their worktrees, branches, and
-- intermediate tags for inspection; this row is written only after that
-- cleanup succeeds.
CREATE TABLE mission_artifact_cleanup (
    mission_id TEXT PRIMARY KEY NOT NULL,
    repo_root TEXT NOT NULL CHECK (length(trim(repo_root)) > 0),
    cleaned_at TEXT NOT NULL,
    FOREIGN KEY (mission_id) REFERENCES mission_outcomes(mission_id) ON DELETE CASCADE
);
