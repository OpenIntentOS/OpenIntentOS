//! Error types for SQLite adapter.

use thiserror::Error;

/// Errors that can occur when using the SQLite adapter.
#[derive(Error, Debug)]
pub enum SqliteError {
    /// Connection pool error.
    #[error("connection pool error: {0}")]
    ConnectionPool(String),

    /// Database connection error.
    #[error("database connection failed: {0}")]
    Connection(String),

    /// Query execution error.
    #[error("query execution failed: {0}")]
    QueryExecution(String),

    /// Migration error.
    #[error("migration failed: {0}")]
    MigrationFailed(String),

    /// Configuration error.
    #[error("configuration error: {0}")]
    Configuration(String),

    /// Serialization/deserialization error.
    #[error("serialization error: {0}")]
    Serialization(String),

    /// Invalid parameter error.
    #[error("invalid parameter: {0}")]
    InvalidParameter(String),

    /// Transaction error.
    #[error("transaction error: {0}")]
    Transaction(String),

    /// Schema error.
    #[error("schema error: {0}")]
    Schema(String),
}

impl From<sqlx::Error> for SqliteError {
    fn from(err: sqlx::Error) -> Self {
        match err {
            sqlx::Error::Database(db_err) => {
                SqliteError::QueryExecution(db_err.to_string())
            }
            sqlx::Error::Io(io_err) => {
                SqliteError::Connection(io_err.to_string())
            }
            sqlx::Error::PoolClosed => {
                SqliteError::ConnectionPool("connection pool is closed".to_string())
            }
            sqlx::Error::PoolTimedOut => {
                SqliteError::ConnectionPool("connection pool timed out".to_string())
            }
            _ => SqliteError::QueryExecution(err.to_string()),
        }
    }
}

impl From<serde_json::Error> for SqliteError {
    fn from(err: serde_json::Error) -> Self {
        SqliteError::Serialization(err.to_string())
    }
}

/// Result type for SQLite operations.
pub type SqliteResult<T> = Result<T, SqliteError>;