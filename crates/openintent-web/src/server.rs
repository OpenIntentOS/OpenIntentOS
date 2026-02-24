//! Main web server setup and startup.
//!
//! [`WebServer`] composes the Axum router, registers all routes, and starts
//! the HTTP listener.  It also spawns a background file watcher that
//! hot-reloads `config/IDENTITY.md` whenever the file changes on disk.

use std::path::Path;
use std::sync::Arc;

use axum::Router;
use axum::http::{HeaderValue, Method};
use axum::response::Html;
use axum::routing::{delete, get, post};
use tokio::sync::RwLock;
use tower_http::cors::CorsLayer;

use openintent_adapters::Adapter;
use openintent_agent::LlmClient;
use openintent_store::{Database, SessionStore};

use crate::WebConfig;
use crate::api;
use crate::frontend::INDEX_HTML;
use crate::mcp;
use crate::state::AppState;
use crate::ws;

/// The OpenIntentOS web server.
pub struct WebServer {
    config: WebConfig,
    state: Arc<AppState>,
}

impl WebServer {
    /// Create a new web server.
    ///
    /// # Arguments
    ///
    /// * `config` - Bind address and port configuration.
    /// * `llm` - The LLM client shared across all requests.
    /// * `adapters` - The set of service adapters to expose.
    /// * `db` - The database handle.
    pub fn new(
        config: WebConfig,
        llm: Arc<LlmClient>,
        adapters: Vec<Arc<dyn Adapter>>,
        db: Database,
    ) -> Self {
        let sessions = Arc::new(SessionStore::new(db.clone()));
        let system_prompt = load_system_prompt();
        let evolution = openintent_agent::EvolutionEngine::from_env();
        let state = Arc::new(AppState {
            llm,
            adapters,
            config: config.clone(),
            db,
            sessions,
            system_prompt: Arc::new(RwLock::new(system_prompt)),
            evolution,
        });
        Self { config, state }
    }

    /// Return the `host:port` string this server will bind to.
    pub fn addr(&self) -> String {
        format!("{}:{}", self.config.bind_addr, self.config.port)
    }

    /// Build the Axum router with all routes registered.
    fn router(&self) -> Router {
        let cors = CorsLayer::new()
            .allow_origin("*".parse::<HeaderValue>().unwrap())
            .allow_methods([Method::GET, Method::POST, Method::DELETE])
            .allow_headers(tower_http::cors::Any);

        Router::new()
            // Embedded frontend.
            .route("/", get(|| async { Html(INDEX_HTML) }))
            // REST API.
            .route("/api/status", get(api::status))
            .route("/api/adapters", get(api::adapters))
            .route("/api/chat", post(api::chat))
            // Session management.
            .route("/api/sessions", get(api::list_sessions))
            .route("/api/sessions", post(api::create_session))
            .route("/api/sessions/{id}", get(api::get_session))
            .route("/api/sessions/{id}", delete(api::delete_session))
            .route(
                "/api/sessions/{id}/messages",
                get(api::get_session_messages),
            )
            // MCP (Model Context Protocol) endpoint.
            .route("/mcp", post(mcp::handle_mcp_request))
            // WebSocket.
            .route("/ws", get(ws::ws_handler))
            .layer(cors)
            .with_state(Arc::clone(&self.state))
    }

    /// Start the server and block until it is shut down.
    ///
    /// A background task watches `config/IDENTITY.md` for changes and
    /// hot-reloads the system prompt into [`AppState::system_prompt`].
    ///
    /// # Errors
    ///
    /// Returns an error if the TCP listener cannot be bound.
    pub async fn start(self) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
        let addr = self.addr();
        let router = self.router();

        // Spawn the config file watcher.
        let prompt_handle = Arc::clone(&self.state.system_prompt);
        tokio::task::spawn_blocking(move || watch_config_files(prompt_handle));

        tracing::info!(addr = %addr, "starting web server");

        let listener = tokio::net::TcpListener::bind(&addr).await?;
        axum::serve(listener, router).await?;

        Ok(())
    }
}

// ── config loading ──────────────────────────────────────────────────

/// Load the system prompt from `config/IDENTITY.md`, falling back to a
/// sensible default if the file does not exist.
fn load_system_prompt() -> String {
    let identity = Path::new("config/IDENTITY.md");
    if identity.exists() {
        std::fs::read_to_string(identity).unwrap_or_else(|_| default_system_prompt())
    } else {
        default_system_prompt()
    }
}

fn default_system_prompt() -> String {
    "You are OpenIntentOS, an AI-powered operating system assistant. \
     Your role is to understand user intents and execute tasks using available tools. \
     Be concise, accurate, and proactive. Always confirm before destructive actions."
        .to_owned()
}

// ── file watcher (runs on a blocking thread) ────────────────────────

/// Watch `config/` for file changes and hot-reload the system prompt.
///
/// This function blocks forever and is intended to be called from
/// `tokio::task::spawn_blocking`.
fn watch_config_files(system_prompt: Arc<RwLock<String>>) {
    use notify::{Config, EventKind, RecommendedWatcher, RecursiveMode, Watcher};
    use std::sync::mpsc;

    let config_dir = Path::new("config");
    if !config_dir.exists() {
        tracing::debug!("config/ directory does not exist, skipping file watcher");
        return;
    }

    let (tx, rx) = mpsc::channel();

    let mut watcher = match RecommendedWatcher::new(tx, Config::default()) {
        Ok(w) => w,
        Err(e) => {
            tracing::warn!(error = %e, "failed to create file watcher, hot-reload disabled");
            return;
        }
    };

    if let Err(e) = watcher.watch(config_dir, RecursiveMode::NonRecursive) {
        tracing::warn!(error = %e, "failed to watch config directory");
        return;
    }

    tracing::info!("config hot-reload watcher started on config/");

    for event in rx {
        let event = match event {
            Ok(e) => e,
            Err(e) => {
                tracing::warn!(error = %e, "file watcher error");
                continue;
            }
        };

        // Only react to write / create events.
        match event.kind {
            EventKind::Modify(_) | EventKind::Create(_) => {}
            _ => continue,
        }

        for path in &event.paths {
            let Some(filename) = path.file_name() else {
                continue;
            };

            if filename == "IDENTITY.md" || filename == "SOUL.md" {
                match std::fs::read_to_string(path) {
                    Ok(content) => {
                        // Use blocking write because we are on a blocking thread.
                        let prompt = system_prompt.blocking_write();
                        let old_len = prompt.len();
                        drop(prompt);

                        *system_prompt.blocking_write() = content.clone();

                        tracing::info!(
                            file = %filename.to_string_lossy(),
                            old_len,
                            new_len = content.len(),
                            "hot-reloaded system prompt"
                        );
                    }
                    Err(e) => {
                        tracing::warn!(
                            file = %filename.to_string_lossy(),
                            error = %e,
                            "failed to read updated config file"
                        );
                    }
                }
            }
        }
    }
}
