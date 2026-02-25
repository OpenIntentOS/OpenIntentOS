//! Adapter error types.
//!
//! All adapter subsystems surface errors through [`AdapterError`].  Each
//! variant carries enough context for callers to decide how to handle the
//! failure without inspecting opaque strings.

/// Unified error type for OpenIntentOS adapters.
#[derive(Debug, thiserror::Error)]
pub enum AdapterError {
    /// An I/O operation failed within the adapter.
    #[error("io error: {0}")]
    IoError(#[from] std::io::Error),

    /// The requested tool does not exist on this adapter.
    #[error("tool not found: `{tool_name}` on adapter `{adapter_id}`")]
    ToolNotFound {
        adapter_id: String,
        tool_name: String,
    },

    /// The parameters supplied to a tool are invalid.
    #[error("invalid parameters for tool `{tool_name}`: {reason}")]
    InvalidParams { tool_name: String, reason: String },

    /// A tool invocation failed.
    #[error("execution failed for tool `{tool_name}`: {reason}")]
    ExecutionFailed { tool_name: String, reason: String },

    /// The adapter requires authentication that has not been configured.
    #[error("authentication required for adapter `{adapter_id}`: provider={provider}")]
    AuthRequired {
        adapter_id: String,
        provider: String,
    },

    /// JSON serialization or deserialization failed.
    #[error("serialization error: {0}")]
    SerializationError(#[from] serde_json::Error),

    /// An operation exceeded its time limit.
    #[error("timeout after {seconds}s: {reason}")]
    Timeout { seconds: u64, reason: String },

    /// Configuration error in adapter setup.
    #[error("configuration error: {0}")]
    ConfigError(String),

    /// Invalid input provided to adapter.
    #[error("invalid input: {0}")]
    InvalidInput(String),

    /// Execution error during adapter operation.
    #[error("execution error: {0}")]
    ExecutionError(String),

    /// Catch-all for unexpected internal errors.  Prefer a typed variant
    /// whenever possible.
    #[error("internal adapter error: {0}")]
    Internal(String),
}

/// Convenience alias used throughout the adapters crate.
pub type Result<T> = std::result::Result<T, AdapterError>;