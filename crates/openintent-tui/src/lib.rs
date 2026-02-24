//! Terminal UI for OpenIntentOS.
//!
//! This crate will provide a rich terminal user interface using a TUI
//! framework (e.g. `ratatui`).  It will include:
//!
//! - A split-pane layout with input, output, and status panels.
//! - Real-time streaming of agent output.
//! - Keyboard shortcuts for common operations.
//! - Syntax-highlighted code and file previews.
//!
//! # Status
//!
//! This crate is currently a stub.  The TUI will be implemented once the
//! core agent and intent pipelines are stable.

/// Placeholder TUI configuration.
#[derive(Debug, Clone)]
pub struct TuiConfig {
    /// Whether to enable mouse support.
    pub mouse: bool,
    /// Whether to use alternate screen mode.
    pub alternate_screen: bool,
}

impl Default for TuiConfig {
    fn default() -> Self {
        Self {
            mouse: true,
            alternate_screen: true,
        }
    }
}

/// Placeholder TUI application.
pub struct TuiApp {
    config: TuiConfig,
}

impl TuiApp {
    /// Create a new TUI application with the given configuration.
    pub fn new(config: TuiConfig) -> Self {
        Self { config }
    }

    /// Return the TUI configuration.
    pub fn config(&self) -> &TuiConfig {
        &self.config
    }
}
