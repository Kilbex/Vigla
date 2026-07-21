-- Write-ahead intent for Git dispositions. A crash between Git mutation and
-- terminal outcome persistence is reconciled on the next host startup.
CREATE TABLE IF NOT EXISTS mission_disposition_journal (
    mission_id  TEXT PRIMARY KEY,
    repo_root   TEXT NOT NULL,
    target_ref  TEXT NOT NULL,
    action      TEXT NOT NULL CHECK (action IN ('merge', 'discard')),
    created_at  TEXT NOT NULL
);
