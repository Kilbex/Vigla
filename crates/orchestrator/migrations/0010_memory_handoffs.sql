-- S8 Context Control: per-mission record of worker → downstream
-- handoff notes. Reads by the DAG scheduler (S7) to assemble a
-- downstream brief, and by the memory kernel to harvest for
-- cross-mission recall.
--
-- One row per emitted HandoffNote. The handoff_id is the same
-- short ULID style used elsewhere (`new_memory_event_id`).
CREATE TABLE memory_handoffs (
    handoff_id TEXT PRIMARY KEY NOT NULL,
    mission_id TEXT NOT NULL,
    from_worker TEXT NOT NULL,
    to_role TEXT NOT NULL,
    note TEXT NOT NULL,
    created_at TEXT NOT NULL
);

-- Most reads are "give me every handoff for this mission";
-- secondary index supports it.
CREATE INDEX memory_handoffs_mission_idx ON memory_handoffs(mission_id);
