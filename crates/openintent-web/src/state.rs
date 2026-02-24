//! Shared application state for the web server.
//!
//! [`AppState`] is wrapped in an `Arc` and shared across all request handlers
//! and WebSocket connections.  It holds references to the LLM client and the
//! set of registered adapters.

use std::sync::Arc;

use openintent_adapters::Adapter;
use openintent_agent::LlmClient;

use crate::WebConfig;

/// Shared state accessible from every Axum handler.
#[derive(Clone)]
pub struct AppState {
    /// The LLM client used for chat completions.
    pub llm: Arc<LlmClient>,

    /// Registered service adapters (filesystem, shell, etc.).
    pub adapters: Vec<Arc<dyn Adapter>>,

    /// Web server configuration.
    pub config: WebConfig,
}
