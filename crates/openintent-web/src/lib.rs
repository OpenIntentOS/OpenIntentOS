//! Web interface for OpenIntentOS.
//!
//! This crate provides an HTTP/WebSocket server that exposes the OpenIntentOS
//! functionality through a web-based UI.  It includes:
//!
//! - A REST API for system status and adapter/tool discovery.
//! - A WebSocket endpoint for real-time streaming of agent output.
//! - An embedded single-page HTML frontend served at `/`.
//! - An MCP (Model Context Protocol) endpoint for tool exposure to LLMs.

pub mod api;
pub mod frontend;
pub mod mcp;
pub mod server;
pub mod state;
pub mod ws;

pub use mcp::McpServer;
pub use server::WebServer;
pub use state::AppState;

/// Web server configuration.
#[derive(Debug, Clone)]
pub struct WebConfig {
    /// The address to bind the HTTP server to.
    pub bind_addr: String,
    /// The port to listen on.
    pub port: u16,
}

impl Default for WebConfig {
    fn default() -> Self {
        Self {
            bind_addr: "127.0.0.1".into(),
            port: 3000,
        }
    }
}
