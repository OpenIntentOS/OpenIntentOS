//! TUI error types.
//!
//! All TUI subsystems surface errors through [`TuiError`].

use thiserror::Error;

/// Unified error type for the terminal UI.
#[derive(Error, Debug)]
pub enum TuiError {
    /// An I/O operation failed.
    #[error("io error: {0}")]
    Io(#[from] std::io::Error),

    /// An agent error occurred during ReAct loop execution.
    #[error("agent error: {0}")]
    Agent(#[from] openintent_agent::AgentError),

    /// A terminal-specific error (e.g. raw mode failure).
    #[error("terminal error: {0}")]
    Terminal(String),

    /// The agent response channel was closed unexpectedly.
    #[error("agent channel closed")]
    ChannelClosed,
}

/// Convenience alias used throughout the TUI crate.
pub type Result<T> = std::result::Result<T, TuiError>;
