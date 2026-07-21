-- Terminal mission disposition, recorded independently of audit history.
--
-- Audit rows answer "how did the work score?"; this table answers "what did
-- the user ultimately do with it?" Keeping those facts separate prevents an
-- audited-but-discarded mission from being presented as merged (and therefore
-- revertible) by the History UI.
CREATE TABLE mission_outcomes (
    mission_id TEXT PRIMARY KEY,
    target_ref TEXT NOT NULL CHECK (length(trim(target_ref)) > 0),
    state TEXT NOT NULL CHECK (state IN ('merged', 'discarded', 'aborted')),
    updated_at TEXT NOT NULL
);

CREATE INDEX idx_mission_outcomes_updated_at
    ON mission_outcomes (updated_at DESC);
