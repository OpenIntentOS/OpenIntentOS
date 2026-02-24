//! Web interface for OpenIntentOS.
//!
//! This crate will provide an HTTP/WebSocket server that exposes the
//! OpenIntentOS functionality through a web-based UI.  It will include:
//!
//! - A REST API for system management and tool invocation.
//! - A WebSocket endpoint for real-time streaming of agent output.
//! - A static file server for the web frontend assets.
//!
//! # Status
//!
//! This crate is currently a stub.  The web server will be implemented
//! once the core agent and intent pipelines are stable.

/// Placeholder web server configuration.
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

/// Placeholder web server.
pub struct WebServer {
    config: WebConfig,
}

impl WebServer {
    /// Create a new web server with the given configuration.
    pub fn new(config: WebConfig) -> Self {
        Self { config }
    }

    /// Return the bind address and port.
    pub fn addr(&self) -> String {
        format!("{}:{}", self.config.bind_addr, self.config.port)
    }
}
