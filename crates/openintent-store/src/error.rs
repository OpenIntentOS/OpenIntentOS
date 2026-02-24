//! Error types for the openintent-store crate.
//!
//! All storage operations return [`StoreError`] via [`StoreResult`].
//! Uses `thiserror` for ergonomic, zero-cost error definitions.

use thiserror::Error;

/// Alias for `Result<T, StoreError>`.
pub type StoreResult<T> = Result<T, StoreError>;

/// Errors that can occur in the storage engine.
#[derive(Debug, Error)]
pub enum StoreError {
    /// SQLite operation failed.
    #[error("sqlite error: {0}")]
    Sqlite(#[from] rusqlite::Error),

    /// JSON serialization or deserialization failed.
    #[error("json error: {0}")]
    Json(#[from] serde_json::Error),

    /// A schema migration failed.
    #[error("migration v{version} failed: {message}")]
    Migration { version: u32, message: String },

    /// The requested record was not found.
    #[error("{entity} not found: {id}")]
    NotFound { entity: &'static str, id: String },

    /// An invalid argument was provided to a store operation.
    #[error("invalid argument: {0}")]
    InvalidArgument(String),

    /// A blocking task was cancelled or panicked.
    #[error("background task failed: {0}")]
    TaskJoin(String),

    /// Cache operation failed.
    #[error("cache error: {0}")]
    Cache(String),
}

impl From<tokio::task::JoinError> for StoreError {
    fn from(err: tokio::task::JoinError) -> Self {
        Self::TaskJoin(err.to_string())
    }
}
