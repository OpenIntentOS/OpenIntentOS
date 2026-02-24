//! UI abstraction layer for OpenIntentOS.
//!
//! This crate will provide a unified interface for rendering UI across
//! different frontends (TUI, web, native).  It defines the shared traits
//! and types that concrete UI implementations will use.
//!
//! # Status
//!
//! This crate is currently a stub.  The UI abstraction layer will be
//! designed once the TUI and web frontends are further along.

/// Placeholder UI renderer trait.
///
/// Concrete implementations will be provided by `openintent-tui` and
/// `openintent-web`.
pub trait Renderer: Send + Sync {
    /// Render a text message to the user.
    fn render_text(&self, text: &str);

    /// Render an error message to the user.
    fn render_error(&self, error: &str);

    /// Prompt the user for input and return their response.
    fn prompt(&self, message: &str) -> String;
}

/// Placeholder UI configuration.
#[derive(Debug, Clone)]
pub struct UiConfig {
    /// Whether to use color output.
    pub color: bool,
    /// Whether to use unicode characters.
    pub unicode: bool,
}

impl Default for UiConfig {
    fn default() -> Self {
        Self {
            color: true,
            unicode: true,
        }
    }
}
