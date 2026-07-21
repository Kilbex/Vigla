-- Phase 2 (V1.2 memory hybrid retrieval) — per-note embedding store.
--
-- Stores one 384-dim MiniLM-L6-v2 vector per promoted note, plus the
-- `model_version` it was computed under so a model swap can purge +
-- re-embed without a code-level migration.
--
-- ## BLOB layout
--
-- `vector` is the raw little-endian byte representation of a `[f32; N]`
-- array (4 bytes per element, no header). Length validation lives in
-- `memory::retrieval::storage::deserialize` — a corrupt or wrong-dim
-- BLOB returns an error rather than silently producing garbage cosine
-- scores. The choice of raw BLOB over `sqlite-vss` is deliberate: we
-- only need cosine similarity (a hot loop over `Vec<f32>`), and
-- bundling a native vector-search extension doubles the DMG signing
-- surface for negligible recall benefit at 20-1k note corpora.
--
-- ## Lifecycle invariants
--
-- - A row exists if and only if the note is `promoted` AND has been
--   embedded under the current `MODEL_VERSION`.
-- - When a note demotes back to `owned`, its row is left alone — the
--   row will be reused on re-promotion (saves an embed call). If the
--   note is hard-deleted, the FK cascade drops the embedding too.
-- - When `MODEL_VERSION` bumps, the kernel deletes every row whose
--   `model_version` differs from the running constant before backfill.

CREATE TABLE memory_note_embeddings (
  note_id        TEXT NOT NULL PRIMARY KEY
                   REFERENCES memory_notes(id) ON DELETE CASCADE,
  vector         BLOB NOT NULL,
  model_version  TEXT NOT NULL,
  computed_at    TEXT NOT NULL
);

CREATE INDEX idx_memory_note_embeddings_model
  ON memory_note_embeddings(model_version);
