-- Vigla Memory Kernel — Phase P2 (supervisor ratification loop).
--
-- Adds typed witness signals that drive confidence scoring (V3 §2)
-- and disputes (V3 §1.2, threat #7).
--
-- Witnesses are append-only. Confidence is *derived* from witnesses,
-- not stored — recomputed by `memory/scoring.rs` on demand and cached
-- in process. Changing coefficient weights therefore needs no data
-- migration; it just invalidates the in-process cache.

CREATE TABLE memory_witnesses (
    id              TEXT PRIMARY KEY NOT NULL,                 -- ULID-shaped UUIDv7
    note_id         TEXT NOT NULL,
    kind            TEXT NOT NULL,                              -- see WitnessKind enum
    weight          REAL NOT NULL,                              -- snapshot of weight at record time
    source_event_id TEXT NOT NULL,                              -- replay anchor
    observed_at     TEXT NOT NULL,
    FOREIGN KEY (note_id) REFERENCES memory_notes(id)
);
CREATE INDEX idx_mem_wit_note ON memory_witnesses(note_id, observed_at);
CREATE INDEX idx_mem_wit_kind ON memory_witnesses(kind, observed_at);

-- Pairs of notes flagged as conflicting. P2 uses this only as an
-- input to the promotion predicate (threat #7). P5 wires the full
-- disputed-state UX with resolution events.
CREATE TABLE memory_disputes (
    id                  TEXT PRIMARY KEY NOT NULL,
    note_ids_json       TEXT NOT NULL,                          -- JSON array of >=2 ids
    detector_event_id   TEXT NOT NULL,
    state               TEXT NOT NULL DEFAULT 'open',           -- open|resolved
    resolution_event_id TEXT,
    resolution_json     TEXT,
    created_at          TEXT NOT NULL
);
CREATE INDEX idx_mem_disputes_state ON memory_disputes(state);
