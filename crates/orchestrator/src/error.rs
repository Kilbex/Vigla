//! Error type for the orchestrator's persistence surface.

use thiserror::Error;

/// Errors returned by the [`crate::Repository`] surface.
#[derive(Debug, Error)]
pub enum RepositoryError {
    /// SQL backend failure (connection, migration, query).
    #[error("sqlx: {0}")]
    Sqlx(#[from] sqlx::Error),

    /// Migration runner failed.
    #[error("migrate: {0}")]
    Migrate(#[from] sqlx::migrate::MigrateError),

    /// Payload serialize/deserialize failure. Should be unreachable in
    /// practice — events are produced by trusted code paths within the
    /// app — but this protects us from a future mismatch.
    #[error("json: {0}")]
    Json(#[from] serde_json::Error),

    /// I/O failure when creating the data directory or opening the db
    /// file (e.g. permissions, missing parent).
    #[error("io: {0}")]
    Io(#[from] std::io::Error),

    /// A row read from the database was missing a required field. This
    /// indicates the schema and the code disagree — investigate before
    /// shipping.
    #[error("row corrupt: {0}")]
    RowCorrupt(String),
}
