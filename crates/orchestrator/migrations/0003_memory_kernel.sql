-- Vigla Memory Kernel — Phase P0 spine.
--
-- Long-term project memory. Notes are the unit of stored knowledge;
-- bodies live on disk under `.vigla/codex/notes/<id>.md` and this
-- table is the queryable index.
--
-- Mutation rules (V3 §1.2):
--   * Workers never write here directly. Only the orchestrator's
--     memory module mutates these tables.
--   * State transitions are append-only via events; rows are mutated
--     in place only for state and last_verified_at.
--
-- All identifiers are UUIDv7 strings (matches the existing convention
-- from `orchestrator/src/ids.rs`). Timestamps are RFC 3339 UTC with
-- millisecond precision.

-- T3 codex notes. body_path resolves under the codex root configured
-- at `MemoryKernel::open` time.
CREATE TABLE memory_notes (
    id                  TEXT PRIMARY KEY NOT NULL,
    kind                TEXT NOT NULL,                         -- fact|decision|procedure|hazard
    scope_kind          TEXT NOT NULL,                         -- repo|path|vendor|supervisor|worker
    scope_value         TEXT,                                  -- nullable for scope_kind='repo'
    body_path           TEXT NOT NULL,                         -- relative to codex root
    body_hash           TEXT NOT NULL,                         -- hex digest of body (P1 wires blake3)
    state               TEXT NOT NULL DEFAULT 'owned',         -- owned|promoted|disputed|invalid
    created_event_id    TEXT NOT NULL,
    created_at          TEXT NOT NULL,
    last_verified_at    TEXT
);
CREATE INDEX idx_mem_notes_state_kind ON memory_notes(state, kind);
CREATE INDEX idx_mem_notes_scope ON memory_notes(scope_kind, scope_value);

-- Directed links. `supersedes` is the only semantic the kernel writes
-- in P0; `refines | related | conflicts_with` arrive in P5 reflection.
CREATE TABLE memory_links (
    src_note_id      TEXT NOT NULL,
    dst_note_id      TEXT NOT NULL,
    link_kind        TEXT NOT NULL,
    created_event_id TEXT NOT NULL,
    PRIMARY KEY (src_note_id, dst_note_id, link_kind)
);
CREATE INDEX idx_mem_links_dst ON memory_links(dst_note_id, link_kind);

-- Every event that contributed evidence for a note (P0 records the
-- creation event; later phases append witnesses).
CREATE TABLE memory_provenance (
    note_id  TEXT NOT NULL,
    event_id TEXT NOT NULL,
    role     TEXT NOT NULL,                                    -- proposed|ratified|witnessed|normalized|authored
    PRIMARY KEY (note_id, event_id, role)
);

-- T2 pending proposals (Modified state in MOESI-Lite). Populated by
-- the worker adapter in P2; defined here so the schema is stable.
CREATE TABLE memory_pending (
    proposal_id      TEXT PRIMARY KEY NOT NULL,
    mission_id       TEXT NOT NULL,
    worker_id        TEXT NOT NULL,
    kind             TEXT NOT NULL,
    scope_kind       TEXT NOT NULL,
    scope_value      TEXT,
    body             TEXT NOT NULL,
    derived_from     TEXT NOT NULL DEFAULT '[]',
    evidence         TEXT NOT NULL DEFAULT '[]',
    state            TEXT NOT NULL DEFAULT 'proposed',         -- proposed|normalized|ratified|rejected
    created_event_id TEXT NOT NULL,
    created_at       TEXT NOT NULL
);
CREATE INDEX idx_mem_pending_mission ON memory_pending(mission_id, state);

-- Bundle archive. Replay foundation: every byte a worker ever saw is
-- reconstructable from these rows + the rendered file on disk.
CREATE TABLE memory_bundles (
    bundle_id         TEXT PRIMARY KEY NOT NULL,
    mission_id        TEXT NOT NULL,
    worker_id         TEXT NOT NULL,
    turn              INTEGER NOT NULL,
    vendor            TEXT NOT NULL,
    hash              TEXT NOT NULL,
    page_table_json   TEXT NOT NULL,
    trace_json        TEXT NOT NULL DEFAULT '{}',
    rendered_path     TEXT NOT NULL,
    composed_event_id TEXT NOT NULL,
    rendered_event_id TEXT
);
CREATE UNIQUE INDEX idx_mem_bundles_w_turn ON memory_bundles(worker_id, turn);

-- Open enum for note kinds and scope kinds. Extending the taxonomy is
-- a data migration, not a code change.
CREATE TABLE memory_taxonomy (
    name              TEXT NOT NULL,
    category          TEXT NOT NULL,                           -- 'kind' | 'scope_kind'
    promote_threshold REAL,                                    -- only set for category='kind'
    introduced_at     TEXT NOT NULL,
    deprecated_at     TEXT,
    PRIMARY KEY (name, category)
);

-- Memory event log. Separate from `events` because memory events are
-- orchestrator-emitted (not vendor-emitted) and some carry no worker_id
-- (e.g. MemoryBarrier, MemoryNoteAuthored via CLI).
CREATE TABLE memory_events (
    event_id        TEXT PRIMARY KEY NOT NULL,
    mission_id      TEXT,
    worker_id       TEXT,
    ts              TEXT NOT NULL,
    type            TEXT NOT NULL,
    payload_json    TEXT NOT NULL,
    schema_version  TEXT NOT NULL
);
CREATE INDEX idx_mem_events_ts ON memory_events(ts);
CREATE INDEX idx_mem_events_mission_ts ON memory_events(mission_id, ts);
CREATE INDEX idx_mem_events_type_ts ON memory_events(type, ts);

-- Seed the taxonomy with the four standard kinds and their P0
-- promotion thresholds (V3 §7.5). Asymmetric: hazards promote easily
-- (false positives cheap, missed pitfalls expensive); decisions stick
-- only with strong evidence.
INSERT OR IGNORE INTO memory_taxonomy (name, category, promote_threshold, introduced_at) VALUES
    ('fact',      'kind',       0.70, '2026-05-15T00:00:00.000Z'),
    ('decision',  'kind',       0.85, '2026-05-15T00:00:00.000Z'),
    ('procedure', 'kind',       0.75, '2026-05-15T00:00:00.000Z'),
    ('hazard',    'kind',       0.55, '2026-05-15T00:00:00.000Z'),
    ('repo',       'scope_kind', NULL, '2026-05-15T00:00:00.000Z'),
    ('path',       'scope_kind', NULL, '2026-05-15T00:00:00.000Z'),
    ('vendor',     'scope_kind', NULL, '2026-05-15T00:00:00.000Z'),
    ('supervisor', 'scope_kind', NULL, '2026-05-15T00:00:00.000Z'),
    ('worker',     'scope_kind', NULL, '2026-05-15T00:00:00.000Z');
