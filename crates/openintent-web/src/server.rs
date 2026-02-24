//! Main web server setup and startup.
//!
//! [`WebServer`] composes the Axum router, registers all routes, and starts
//! the HTTP listener.

use std::sync::Arc;

use axum::Router;
use axum::http::{HeaderValue, Method};
use axum::response::Html;
use axum::routing::{delete, get, post};
use tower_http::cors::CorsLayer;

use openintent_adapters::Adapter;
use openintent_agent::LlmClient;
use openintent_store::{Database, SessionStore};

use crate::WebConfig;
use crate::api;
use crate::frontend::INDEX_HTML;
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
        let state = Arc::new(AppState {
            llm,
            adapters,
            config: config.clone(),
            db,
            sessions,
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
            // WebSocket.
            .route("/ws", get(ws::ws_handler))
            .layer(cors)
            .with_state(Arc::clone(&self.state))
    }

    /// Start the server and block until it is shut down.
    ///
    /// # Errors
    ///
    /// Returns an error if the TCP listener cannot be bound.
    pub async fn start(self) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
        let addr = self.addr();
        let router = self.router();

        tracing::info!(addr = %addr, "starting web server");

        let listener = tokio::net::TcpListener::bind(&addr).await?;
        axum::serve(listener, router).await?;

        Ok(())
    }
}
