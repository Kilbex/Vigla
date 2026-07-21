//! Errors returned by the Memory Kernel.

use thiserror::Error;

#[derive(Debug, Error)]
pub enum MemoryError {
    /// SQL backend failure (pool, query, txn).
    #[error("sqlx: {0}")]
    Sqlx(#[from] sqlx::Error),

    /// Migration runner failed when opening a per-repo memory pool
    /// (A2). The on-disk file at `<repo>/.vigla/memory/memory.sqlite`
    /// either doesn't accept the migration set or is corrupted.
    #[error("migrate: {0}")]
    Migrate(#[from] sqlx::migrate::MigrateError),

    /// Body file / codex directory I/O failure.
    #[error("io: {0}")]
    Io(#[from] std::io::Error),

    /// JSON encode/decode failure on memory event payloads or
    /// metadata columns.
    #[error("json: {0}")]
    Json(#[from] serde_json::Error),

    /// Caller passed a scope or kind that's not in `memory_taxonomy`.
    #[error("unknown taxonomy term: {category}={name}")]
    UnknownTaxonomy { category: String, name: String },

    /// A scope_kind that requires a value was missing one (everything
    /// except `repo`).
    #[error("scope kind {0} requires a non-empty value")]
    MissingScopeValue(String),

    /// `note_show` / `note_supersede` referenced a note that doesn't
    /// exist or has been replaced.
    #[error("note not found: {0}")]
    NoteNotFound(String),

    /// `note_add` body exceeded the per-note cap (V3 §13 decision 4).
    #[error("note body too large: {actual} bytes (cap {cap})")]
    BodyTooLarge { actual: usize, cap: usize },

    /// A row read from the database had a column that didn't decode
    /// to the expected vocabulary. Indicates a schema / code drift.
    #[error("row corrupt: {0}")]
    RowCorrupt(String),

    /// A concurrent ratify call won the optimistic lock race for this
    /// proposal. The first caller committed; this one must abort.
    #[error("proposal {0} was already ratified by a concurrent caller")]
    AlreadyRatified(String),
}
