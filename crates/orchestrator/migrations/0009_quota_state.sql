-- S5 — per-vendor quota state. One row per vendor that has ever
-- emitted a QuotaExhausted signal. Read at app start to rehydrate
-- the in-memory VendorQuotaTracker; written on each new exhaustion
-- event.
CREATE TABLE IF NOT EXISTS vendor_quota_state (
    vendor TEXT PRIMARY KEY,
    last_exhausted_at_ms INTEGER,
    estimated_reset_at_ms INTEGER,
    source TEXT NOT NULL
);

CREATE INDEX IF NOT EXISTS idx_vendor_quota_state_reset
    ON vendor_quota_state (estimated_reset_at_ms);
