//! Shared application state for the web server.
//!
//! [`AppState`] is wrapped in an `Arc` and shared across all request handlers
//! and WebSocket connections.  It holds references to the LLM client, adapters,
//! session store, and database.
//!
//! The `system_prompt` field supports hot-reload: when `config/IDENTITY.md`
//! changes on disk the file watcher updates this value and all subsequent
//! requests automatically pick up the new prompt.

use std::sync::Arc;

use openintent_adapters::Adapter;
use openintent_agent::LlmClient;
use openintent_agent::evolution::EvolutionEngine;
use openintent_store::{Database, SessionStore};
use tokio::sync::{Mutex, RwLock};

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

    /// Database handle for general queries.
    pub db: Database,

    /// Session store for conversation persistence.
    pub sessions: Arc<SessionStore>,

    /// System prompt loaded from `config/IDENTITY.md`.
    /// Wrapped in `RwLock` for hot-reload support.
    pub system_prompt: Arc<RwLock<String>>,

    /// Optional self-evolution engine for auto-filing unhandled intent issues.
    pub evolution: Option<Arc<Mutex<EvolutionEngine>>>,
}
