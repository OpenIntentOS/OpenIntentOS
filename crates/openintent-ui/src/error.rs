//! Error types for the desktop UI crate.

use thiserror::Error;

/// Top-level error type for the iced desktop UI.
#[derive(Debug, Error)]
pub enum UiError {
    /// An error originating from the iced framework.
    #[error("iced error: {0}")]
    Iced(String),

    /// An error originating from the agent subsystem.
    #[error("agent error: {0}")]
    Agent(String),

    /// A communication channel was unexpectedly closed.
    #[error("channel closed")]
    ChannelClosed,
}

/// Convenience alias for results within this crate.
pub type Result<T> = std::result::Result<T, UiError>;
