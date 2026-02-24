//! OAuth 2.0 Authorization Code Flow with PKCE.
//!
//! This module implements the core OAuth 2.0 authorization code flow as
//! defined in RFC 6749, with Proof Key for Code Exchange (PKCE) as defined
//! in RFC 7636. PKCE is mandatory for all flows to prevent authorization
//! code interception attacks.
//!
//! # Flow Overview
//!
//! 1. Generate a PKCE code verifier and code challenge.
//! 2. Build an authorization URL and redirect the user.
//! 3. Receive the authorization code via a callback.
//! 4. Exchange the code + verifier for tokens.
//! 5. Optionally refresh tokens when they expire.

use base64::Engine;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use ring::digest;
use ring::rand::{SecureRandom, SystemRandom};
use serde::{Deserialize, Serialize};
use url::Url;

use crate::error::{AuthEngineError, Result};

/// Length of the PKCE code verifier in bytes (before base64 encoding).
const PKCE_VERIFIER_BYTES: usize = 32;

// ---------------------------------------------------------------------------
// Configuration
// ---------------------------------------------------------------------------

/// Configuration for an OAuth 2.0 authorization code flow.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OAuthConfig {
    /// The OAuth client ID.
    pub client_id: String,

    /// The OAuth client secret (confidential clients only).
    pub client_secret: Option<String>,

    /// The authorization endpoint URL.
    pub auth_url: String,

    /// The token endpoint URL.
    pub token_url: String,

    /// The redirect URI registered with the authorization server.
    pub redirect_uri: String,

    /// The scopes to request.
    pub scopes: Vec<String>,
}

// ---------------------------------------------------------------------------
// Token types
// ---------------------------------------------------------------------------

/// Tokens returned by the authorization server after a successful exchange.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OAuthTokens {
    /// The access token used to authenticate API requests.
    pub access_token: String,

    /// The refresh token used to obtain new access tokens.
    pub refresh_token: Option<String>,

    /// Unix timestamp (seconds) when the access token expires.
    pub expires_at: Option<i64>,

    /// The token type (typically "Bearer").
    pub token_type: String,

    /// The scopes that were granted.
    pub scopes: Vec<String>,
}

/// Raw token response from the authorization server.
///
/// This is the JSON shape returned by most OAuth token endpoints. We parse
/// this internally and convert to [`OAuthTokens`].
#[derive(Debug, Deserialize)]
struct TokenResponse {
    access_token: String,
    refresh_token: Option<String>,
    expires_in: Option<i64>,
    token_type: Option<String>,
    scope: Option<String>,
}

impl TokenResponse {
    /// Convert into [`OAuthTokens`], computing `expires_at` from `expires_in`.
    fn into_tokens(self) -> OAuthTokens {
        let expires_at = self
            .expires_in
            .map(|secs| chrono::Utc::now().timestamp() + secs);

        let scopes = self
            .scope
            .map(|s| s.split_whitespace().map(String::from).collect())
            .unwrap_or_default();

        OAuthTokens {
            access_token: self.access_token,
            refresh_token: self.refresh_token,
            expires_at,
            token_type: self.token_type.unwrap_or_else(|| "Bearer".to_string()),
            scopes,
        }
    }
}

/// Raw error response from the authorization server.
#[derive(Debug, Deserialize)]
struct TokenErrorResponse {
    error: String,
    error_description: Option<String>,
}

// ---------------------------------------------------------------------------
// PKCE helpers
// ---------------------------------------------------------------------------

/// Generate a PKCE code verifier (random 32 bytes, base64url encoded).
///
/// # Errors
///
/// Returns an error if the system CSPRNG fails.
pub fn generate_pkce_verifier() -> Result<String> {
    let rng = SystemRandom::new();
    let mut bytes = [0u8; PKCE_VERIFIER_BYTES];
    rng.fill(&mut bytes)
        .map_err(|_| AuthEngineError::FlowFailed {
            reason: "failed to generate PKCE verifier: CSPRNG error".to_string(),
        })?;
    Ok(URL_SAFE_NO_PAD.encode(bytes))
}

/// Derive the PKCE code challenge from a code verifier using SHA-256.
///
/// `challenge = BASE64URL(SHA256(verifier))`
pub fn pkce_challenge(verifier: &str) -> String {
    let hash = digest::digest(&digest::SHA256, verifier.as_bytes());
    URL_SAFE_NO_PAD.encode(hash.as_ref())
}

// ---------------------------------------------------------------------------
// OAuth flow
// ---------------------------------------------------------------------------

/// Manages an OAuth 2.0 authorization code flow with PKCE.
///
/// This struct is stateless — all state is passed explicitly via method
/// parameters. It uses `reqwest` for token exchange HTTP calls and `ring`
/// for PKCE SHA-256 hashing.
pub struct OAuthFlow {
    config: OAuthConfig,
    client: reqwest::Client,
}

impl OAuthFlow {
    /// Create a new OAuth flow with the given configuration.
    pub fn new(config: OAuthConfig) -> Self {
        Self {
            config,
            client: reqwest::Client::new(),
        }
    }

    /// Build the authorization URL the user should visit.
    ///
    /// Includes PKCE `code_challenge` (S256) and a `state` parameter for
    /// CSRF protection. The caller must generate the PKCE verifier via
    /// [`generate_pkce_verifier`] and pass the corresponding challenge here.
    ///
    /// # Errors
    ///
    /// Returns [`AuthEngineError::UrlParse`] if the `auth_url` in the config
    /// is not a valid URL.
    pub fn authorization_url(&self, state: &str, code_challenge: &str) -> Result<String> {
        let mut url = Url::parse(&self.config.auth_url)?;

        {
            let mut params = url.query_pairs_mut();
            params.append_pair("response_type", "code");
            params.append_pair("client_id", &self.config.client_id);
            params.append_pair("redirect_uri", &self.config.redirect_uri);
            params.append_pair("state", state);
            params.append_pair("code_challenge", code_challenge);
            params.append_pair("code_challenge_method", "S256");

            if !self.config.scopes.is_empty() {
                params.append_pair("scope", &self.config.scopes.join(" "));
            }
        }

        Ok(url.to_string())
    }

    /// Exchange an authorization code for tokens.
    ///
    /// The `code_verifier` must be the same verifier whose challenge was sent
    /// in the authorization URL.
    ///
    /// # Errors
    ///
    /// Returns [`AuthEngineError::InvalidGrant`] if the server rejects the
    /// code, or [`AuthEngineError::NetworkError`] on transport failure.
    pub async fn exchange_code(&self, code: &str, code_verifier: &str) -> Result<OAuthTokens> {
        let mut params = vec![
            ("grant_type", "authorization_code"),
            ("code", code),
            ("redirect_uri", self.config.redirect_uri.as_str()),
            ("client_id", self.config.client_id.as_str()),
            ("code_verifier", code_verifier),
        ];

        // Include client_secret if this is a confidential client.
        let secret_binding;
        if let Some(ref secret) = self.config.client_secret {
            secret_binding = secret.clone();
            params.push(("client_secret", &secret_binding));
        }

        tracing::debug!(token_url = %self.config.token_url, "exchanging authorization code");

        let response = self
            .client
            .post(&self.config.token_url)
            .form(&params)
            .send()
            .await?;

        Self::parse_token_response(response).await
    }

    /// Refresh an access token using a refresh token.
    ///
    /// # Errors
    ///
    /// Returns [`AuthEngineError::InvalidGrant`] if the refresh token is
    /// invalid or revoked.
    pub async fn refresh_token(&self, refresh_token: &str) -> Result<OAuthTokens> {
        let mut params = vec![
            ("grant_type", "refresh_token"),
            ("refresh_token", refresh_token),
            ("client_id", self.config.client_id.as_str()),
        ];

        let secret_binding;
        if let Some(ref secret) = self.config.client_secret {
            secret_binding = secret.clone();
            params.push(("client_secret", &secret_binding));
        }

        tracing::debug!(token_url = %self.config.token_url, "refreshing access token");

        let response = self
            .client
            .post(&self.config.token_url)
            .form(&params)
            .send()
            .await?;

        Self::parse_token_response(response).await
    }

    /// Check whether the given tokens are expired.
    ///
    /// Returns `true` if the tokens have an `expires_at` timestamp and that
    /// timestamp is in the past (with a 60-second safety margin).
    pub fn is_expired(tokens: &OAuthTokens) -> bool {
        match tokens.expires_at {
            Some(expires_at) => {
                let now = chrono::Utc::now().timestamp();
                // Treat tokens as expired 60 seconds early to avoid edge cases
                // where we use a token that expires mid-request.
                now >= (expires_at - 60)
            }
            // No expiry info means we assume the token is valid.
            None => false,
        }
    }

    /// Parse the HTTP response from the token endpoint.
    async fn parse_token_response(response: reqwest::Response) -> Result<OAuthTokens> {
        let status = response.status();

        if status.is_success() {
            let token_response: TokenResponse = response.json().await?;
            tracing::debug!("token exchange successful");
            Ok(token_response.into_tokens())
        } else {
            let body = response.text().await.unwrap_or_default();

            // Try to parse as an OAuth error response.
            if let Ok(error_response) = serde_json::from_str::<TokenErrorResponse>(&body) {
                let reason = error_response
                    .error_description
                    .unwrap_or(error_response.error);
                Err(AuthEngineError::InvalidGrant { reason })
            } else {
                Err(AuthEngineError::InvalidGrant {
                    reason: format!("HTTP {status}: {body}"),
                })
            }
        }
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    fn test_config() -> OAuthConfig {
        OAuthConfig {
            client_id: "test-client-id".to_string(),
            client_secret: Some("test-secret".to_string()),
            auth_url: "https://auth.example.com/authorize".to_string(),
            token_url: "https://auth.example.com/token".to_string(),
            redirect_uri: "http://127.0.0.1:8400/callback".to_string(),
            scopes: vec!["read".to_string(), "write".to_string()],
        }
    }

    #[test]
    fn pkce_verifier_is_correct_length() {
        let verifier = generate_pkce_verifier().unwrap();
        // 32 bytes base64url encoded = 43 characters (no padding).
        assert_eq!(verifier.len(), 43);
    }

    #[test]
    fn pkce_verifier_is_url_safe() {
        let verifier = generate_pkce_verifier().unwrap();
        // base64url characters: A-Z, a-z, 0-9, -, _
        for c in verifier.chars() {
            assert!(
                c.is_ascii_alphanumeric() || c == '-' || c == '_',
                "unexpected character in verifier: {c}"
            );
        }
    }

    #[test]
    fn pkce_challenge_is_deterministic() {
        let verifier = "dBjftJeZ4CVP-mB92K27uhbUJU1p1r_wW1gFWFOEjXk";
        let challenge = pkce_challenge(verifier);
        let challenge2 = pkce_challenge(verifier);
        assert_eq!(challenge, challenge2);
    }

    #[test]
    fn pkce_challenge_is_base64url_sha256() {
        // RFC 7636 Appendix B test vector:
        // verifier = "dBjftJeZ4CVP-mB92K27uhbUJU1p1r_wW1gFWFOEjXk"
        // challenge = "E9Melhoa2OwvFrEMTJguCHaoeK1t8URWbuGJSstw-cM"
        let verifier = "dBjftJeZ4CVP-mB92K27uhbUJU1p1r_wW1gFWFOEjXk";
        let challenge = pkce_challenge(verifier);
        assert_eq!(challenge, "E9Melhoa2OwvFrEMTJguCHaoeK1t8URWbuGJSstw-cM");
    }

    #[test]
    fn pkce_different_verifiers_give_different_challenges() {
        let v1 = generate_pkce_verifier().unwrap();
        let v2 = generate_pkce_verifier().unwrap();
        assert_ne!(v1, v2);
        assert_ne!(pkce_challenge(&v1), pkce_challenge(&v2));
    }

    #[test]
    fn authorization_url_includes_all_params() {
        let flow = OAuthFlow::new(test_config());
        let challenge = pkce_challenge("test-verifier");
        let url_str = flow.authorization_url("random-state", &challenge).unwrap();

        let url = Url::parse(&url_str).unwrap();
        let params: std::collections::HashMap<_, _> = url.query_pairs().collect();

        assert_eq!(params.get("response_type").unwrap(), "code");
        assert_eq!(params.get("client_id").unwrap(), "test-client-id");
        assert_eq!(
            params.get("redirect_uri").unwrap(),
            "http://127.0.0.1:8400/callback"
        );
        assert_eq!(params.get("state").unwrap(), "random-state");
        assert_eq!(params.get("code_challenge").unwrap(), challenge.as_str());
        assert_eq!(params.get("code_challenge_method").unwrap(), "S256");
        assert_eq!(params.get("scope").unwrap(), "read write");
    }

    #[test]
    fn authorization_url_without_scopes() {
        let mut config = test_config();
        config.scopes = vec![];
        let flow = OAuthFlow::new(config);
        let challenge = pkce_challenge("test-verifier");
        let url_str = flow.authorization_url("state", &challenge).unwrap();

        let url = Url::parse(&url_str).unwrap();
        let params: std::collections::HashMap<_, _> = url.query_pairs().collect();

        assert!(!params.contains_key("scope"));
    }

    #[test]
    fn authorization_url_preserves_existing_query_params() {
        let mut config = test_config();
        config.auth_url = "https://auth.example.com/authorize?custom=value".to_string();
        let flow = OAuthFlow::new(config);
        let challenge = pkce_challenge("test-verifier");
        let url_str = flow.authorization_url("state", &challenge).unwrap();

        let url = Url::parse(&url_str).unwrap();
        let params: std::collections::HashMap<_, _> = url.query_pairs().collect();

        assert_eq!(params.get("custom").unwrap(), "value");
        assert_eq!(params.get("response_type").unwrap(), "code");
    }

    #[test]
    fn token_response_parsing() {
        let json = r#"{
            "access_token": "gho_abc123",
            "refresh_token": "ghr_def456",
            "expires_in": 3600,
            "token_type": "Bearer",
            "scope": "read write"
        }"#;

        let response: TokenResponse = serde_json::from_str(json).unwrap();
        let tokens = response.into_tokens();

        assert_eq!(tokens.access_token, "gho_abc123");
        assert_eq!(tokens.refresh_token.as_deref(), Some("ghr_def456"));
        assert!(tokens.expires_at.is_some());
        assert_eq!(tokens.token_type, "Bearer");
        assert_eq!(tokens.scopes, vec!["read", "write"]);
    }

    #[test]
    fn token_response_minimal() {
        let json = r#"{ "access_token": "tok_minimal" }"#;

        let response: TokenResponse = serde_json::from_str(json).unwrap();
        let tokens = response.into_tokens();

        assert_eq!(tokens.access_token, "tok_minimal");
        assert!(tokens.refresh_token.is_none());
        assert!(tokens.expires_at.is_none());
        assert_eq!(tokens.token_type, "Bearer");
        assert!(tokens.scopes.is_empty());
    }

    #[test]
    fn is_expired_with_future_expiry() {
        let tokens = OAuthTokens {
            access_token: "tok".to_string(),
            refresh_token: None,
            expires_at: Some(chrono::Utc::now().timestamp() + 3600),
            token_type: "Bearer".to_string(),
            scopes: vec![],
        };
        assert!(!OAuthFlow::is_expired(&tokens));
    }

    #[test]
    fn is_expired_with_past_expiry() {
        let tokens = OAuthTokens {
            access_token: "tok".to_string(),
            refresh_token: None,
            expires_at: Some(chrono::Utc::now().timestamp() - 100),
            token_type: "Bearer".to_string(),
            scopes: vec![],
        };
        assert!(OAuthFlow::is_expired(&tokens));
    }

    #[test]
    fn is_expired_within_safety_margin() {
        let tokens = OAuthTokens {
            access_token: "tok".to_string(),
            refresh_token: None,
            // 30 seconds from now is within the 60-second safety margin.
            expires_at: Some(chrono::Utc::now().timestamp() + 30),
            token_type: "Bearer".to_string(),
            scopes: vec![],
        };
        assert!(OAuthFlow::is_expired(&tokens));
    }

    #[test]
    fn is_expired_with_no_expiry() {
        let tokens = OAuthTokens {
            access_token: "tok".to_string(),
            refresh_token: None,
            expires_at: None,
            token_type: "Bearer".to_string(),
            scopes: vec![],
        };
        assert!(!OAuthFlow::is_expired(&tokens));
    }

    #[test]
    fn oauth_flow_is_send_sync() {
        fn assert_send_sync<T: Send + Sync>() {}
        assert_send_sync::<OAuthFlow>();
        assert_send_sync::<OAuthConfig>();
        assert_send_sync::<OAuthTokens>();
    }

    #[test]
    fn token_error_response_parsing() {
        let json = r#"{
            "error": "invalid_grant",
            "error_description": "The code has expired"
        }"#;

        let err: TokenErrorResponse = serde_json::from_str(json).unwrap();
        assert_eq!(err.error, "invalid_grant");
        assert_eq!(
            err.error_description.as_deref(),
            Some("The code has expired")
        );
    }

    #[test]
    fn token_error_response_without_description() {
        let json = r#"{ "error": "access_denied" }"#;

        let err: TokenErrorResponse = serde_json::from_str(json).unwrap();
        assert_eq!(err.error, "access_denied");
        assert!(err.error_description.is_none());
    }
}
