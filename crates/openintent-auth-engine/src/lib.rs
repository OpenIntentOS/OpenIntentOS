//! Authentication engine for OpenIntentOS.
//!
//! This crate provides complete authentication flow management for the
//! OpenIntentOS AI operating system, including:
//!
//! - **OAuth 2.0 Authorization Code Flow** with PKCE (RFC 7636)
//! - **Device Authorization Grant** (RFC 8628)
//! - **Local callback server** for OAuth browser redirects
//! - **Token lifecycle management**: storage, refresh, and revocation
//!
//! All tokens are stored encrypted in the [`openintent_vault`] credential
//! vault. The [`AuthManager`] orchestrates complete authentication flows
//! and handles automatic token refresh.
//!
//! # Architecture
//!
//! ```text
//! AuthManager
//! ├── OAuthFlow       (authorization code + PKCE)
//! ├── DeviceCodeFlow  (RFC 8628 device grant)
//! ├── CallbackServer  (local HTTP listener)
//! └── Vault           (encrypted token storage)
//! ```
//!
//! # Quick Start
//!
//! ```rust,no_run
//! use openintent_auth_engine::{AuthManager, OAuthConfig};
//! use openintent_vault::store::Vault;
//! use openintent_vault::crypto;
//!
//! # async fn example() -> openintent_auth_engine::error::Result<()> {
//! let key = crypto::random_bytes(crypto::KEY_LEN)?;
//! let vault = Vault::open("data/vault.db", &key)?;
//! let manager = AuthManager::new(vault);
//!
//! let config = OAuthConfig {
//!     client_id: "my-app".to_string(),
//!     client_secret: None,
//!     auth_url: "https://github.com/login/oauth/authorize".to_string(),
//!     token_url: "https://github.com/login/oauth/access_token".to_string(),
//!     redirect_uri: "http://127.0.0.1:8400/callback".to_string(),
//!     scopes: vec!["repo".to_string()],
//! };
//!
//! let tokens = manager.authenticate_oauth("github", config).await?;
//! println!("access token: {}", tokens.access_token);
//! # Ok(())
//! # }
//! ```

pub mod callback;
pub mod device_code;
pub mod error;
pub mod manager;
pub mod oauth;

// Re-export key types at the crate root for convenience.
pub use callback::CallbackServer;
pub use device_code::{DeviceCodeConfig, DeviceCodeFlow, DeviceCodeResponse};
pub use manager::AuthManager;
pub use oauth::{OAuthConfig, OAuthFlow, OAuthTokens};

use serde::{Deserialize, Serialize};

// ---------------------------------------------------------------------------
// Legacy types (kept for backward compatibility)
// ---------------------------------------------------------------------------

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
    #[serde(rename = "oauth2")]
    OAuth2,
    /// OAuth 2.0 device authorization grant (RFC 8628).
    DeviceCode,
    /// Static API key.
    ApiKey,
    /// Bearer token.
    BearerToken,
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn auth_session_serialization() {
        let session = AuthSession {
            provider: "github".to_string(),
            active: true,
            scopes: vec!["repo".to_string(), "user".to_string()],
        };

        let json = serde_json::to_string(&session).unwrap();
        let deserialized: AuthSession = serde_json::from_str(&json).unwrap();
        assert_eq!(deserialized.provider, "github");
        assert!(deserialized.active);
        assert_eq!(deserialized.scopes.len(), 2);
    }

    #[test]
    fn auth_method_serialization() {
        let method = AuthMethod::OAuth2;
        let json = serde_json::to_string(&method).unwrap();
        assert_eq!(json, "\"oauth2\"");

        let method = AuthMethod::DeviceCode;
        let json = serde_json::to_string(&method).unwrap();
        assert_eq!(json, "\"device_code\"");

        let method = AuthMethod::ApiKey;
        let json = serde_json::to_string(&method).unwrap();
        assert_eq!(json, "\"api_key\"");

        let method = AuthMethod::BearerToken;
        let json = serde_json::to_string(&method).unwrap();
        assert_eq!(json, "\"bearer_token\"");
    }

    #[test]
    fn auth_method_deserialization() {
        let method: AuthMethod = serde_json::from_str("\"oauth2\"").unwrap();
        assert_eq!(method, AuthMethod::OAuth2);

        let method: AuthMethod = serde_json::from_str("\"device_code\"").unwrap();
        assert_eq!(method, AuthMethod::DeviceCode);
    }

    #[test]
    fn re_exports_available() {
        // Verify key types are re-exported at the crate root.
        let _: fn() -> OAuthConfig = || OAuthConfig {
            client_id: String::new(),
            client_secret: None,
            auth_url: String::new(),
            token_url: String::new(),
            redirect_uri: String::new(),
            scopes: vec![],
        };
    }
}
