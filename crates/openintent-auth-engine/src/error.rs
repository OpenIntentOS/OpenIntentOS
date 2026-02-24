//! Error types for the auth engine crate.
//!
//! All auth engine operations surface errors through [`AuthEngineError`],
//! which is the single error type for this crate. Each variant carries enough
//! context for callers to decide how to handle the failure.

/// Unified error type for the OpenIntentOS auth engine.
#[derive(Debug, thiserror::Error)]
pub enum AuthEngineError {
    /// The access token has expired and no refresh token is available.
    #[error("token expired for provider {provider}")]
    TokenExpired {
        /// The provider whose token expired.
        provider: String,
    },

    /// The authorization code exchange or refresh grant was rejected by the
    /// authorization server.
    #[error("invalid grant: {reason}")]
    InvalidGrant {
        /// Explanation from the authorization server.
        reason: String,
    },

    /// An HTTP request to the authorization server failed.
    #[error("network error: {0}")]
    NetworkError(#[from] reqwest::Error),

    /// An error propagated from the vault crate.
    #[error("vault error: {0}")]
    VaultError(#[from] openintent_vault::VaultError),

    /// Configuration is missing or malformed.
    #[error("invalid configuration: {reason}")]
    InvalidConfig {
        /// What is wrong with the configuration.
        reason: String,
    },

    /// The requested authentication provider is not registered.
    #[error("provider not found: {provider}")]
    ProviderNotFound {
        /// The provider name that was not found.
        provider: String,
    },

    /// The overall authentication flow failed for a non-specific reason.
    #[error("authentication flow failed: {reason}")]
    FlowFailed {
        /// Details about why the flow failed.
        reason: String,
    },

    /// The local callback server timed out waiting for the redirect.
    #[error("callback timed out after {timeout_secs} seconds")]
    CallbackTimeout {
        /// How many seconds we waited before giving up.
        timeout_secs: u64,
    },

    /// JSON serialization or deserialization error.
    #[error("serialization error: {0}")]
    Serialization(#[from] serde_json::Error),

    /// I/O error (e.g. from the callback TCP listener).
    #[error("io error: {0}")]
    Io(#[from] std::io::Error),

    /// URL parsing error.
    #[error("url parse error: {0}")]
    UrlParse(#[from] url::ParseError),
}

/// Convenience alias used throughout this crate.
pub type Result<T> = std::result::Result<T, AuthEngineError>;

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn error_display_token_expired() {
        let err = AuthEngineError::TokenExpired {
            provider: "github".to_string(),
        };
        assert_eq!(err.to_string(), "token expired for provider github");
    }

    #[test]
    fn error_display_invalid_grant() {
        let err = AuthEngineError::InvalidGrant {
            reason: "bad code".to_string(),
        };
        assert_eq!(err.to_string(), "invalid grant: bad code");
    }

    #[test]
    fn error_display_callback_timeout() {
        let err = AuthEngineError::CallbackTimeout { timeout_secs: 120 };
        assert_eq!(err.to_string(), "callback timed out after 120 seconds");
    }

    #[test]
    fn error_display_provider_not_found() {
        let err = AuthEngineError::ProviderNotFound {
            provider: "slack".to_string(),
        };
        assert_eq!(err.to_string(), "provider not found: slack");
    }

    #[test]
    fn error_display_flow_failed() {
        let err = AuthEngineError::FlowFailed {
            reason: "state mismatch".to_string(),
        };
        assert_eq!(
            err.to_string(),
            "authentication flow failed: state mismatch"
        );
    }

    #[test]
    fn error_display_invalid_config() {
        let err = AuthEngineError::InvalidConfig {
            reason: "missing client_id".to_string(),
        };
        assert_eq!(err.to_string(), "invalid configuration: missing client_id");
    }

    #[test]
    fn error_is_send_sync() {
        fn assert_send_sync<T: Send + Sync>() {}
        assert_send_sync::<AuthEngineError>();
    }
}
