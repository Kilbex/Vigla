CREATE TABLE IF NOT EXISTS audit_reports (
    mission_id TEXT NOT NULL,
    worker_id TEXT,
    tier TEXT NOT NULL,
    overall REAL NOT NULL,
    payload_json TEXT NOT NULL,
    created_at TEXT NOT NULL,
    PRIMARY KEY (mission_id, worker_id, created_at)
);

CREATE INDEX IF NOT EXISTS idx_audit_reports_mission
    ON audit_reports (mission_id);
