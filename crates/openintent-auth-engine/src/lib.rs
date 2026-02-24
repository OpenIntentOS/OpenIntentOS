//! Authentication engine for OpenIntentOS — OAuth, API keys, session management.
//!
//! This crate will provide:
//!
//! - **OAuth 2.0 flows**: Authorization code, device code, and refresh token
//!   management for third-party services (Google, GitHub, Slack, etc.).
//! - **API key management**: Secure storage and rotation of API keys via the
//!   vault crate.
//! - **Session management**: Track active authentication sessions and handle
//!   expiry/renewal.
//!
//! # Status
//!
//! This crate is currently a stub.  The public API surface will be defined
//! as adapters requiring authentication are implemented.

use serde::{Deserialize, Serialize};

/// An authentication session for a third-party service.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AuthSession {
    /// The service provider (e.g. "github", "google", "slack").
    pub provider: String,
    /// Whether the session is currently valid.
    pub active: bool,
    /// The scopes granted in this session.
    pub scopes: Vec<String>,
}

/// Supported authentication methods.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AuthMethod {
    /// OAuth 2.0 authorization code flow.
    OAuth2,
    /// Static API key.
    ApiKey,
    /// Bearer token.
    BearerToken,
}

/// Placeholder authentication engine.
///
/// Will be expanded to manage OAuth flows, token refresh, and session
/// lifecycle in a future version.
pub struct AuthEngine {
    _private: (),
}

impl AuthEngine {
    /// Create a new authentication engine.
    pub fn new() -> Self {
        Self { _private: () }
    }
}

impl Default for AuthEngine {
    fn default() -> Self {
        Self::new()
    }
}
