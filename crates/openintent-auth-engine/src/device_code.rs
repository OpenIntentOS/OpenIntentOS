//! RFC 8628 Device Authorization Grant.
//!
//! This module implements the OAuth 2.0 Device Authorization Grant, which
//! allows devices with limited input capabilities (CLIs, IoT, TVs) to
//! authenticate users by displaying a short code that the user enters on
//! a separate device (phone, laptop).
//!
//! # Flow Overview
//!
//! 1. The client requests a device code from the authorization server.
//! 2. The server returns a `user_code` and `verification_uri`.
//! 3. The user visits the URI and enters the code on their browser.
//! 4. The client polls the token endpoint until the user completes auth.

use serde::{Deserialize, Serialize};

use crate::error::{AuthEngineError, Result};
use crate::oauth::OAuthTokens;

// ---------------------------------------------------------------------------
// Configuration
// ---------------------------------------------------------------------------

/// Configuration for an OAuth 2.0 device authorization grant flow.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DeviceCodeConfig {
    /// The OAuth client ID.
    pub client_id: String,

    /// The device authorization endpoint URL.
    pub device_auth_url: String,

    /// The token endpoint URL.
    pub token_url: String,

    /// The scopes to request.
    pub scopes: Vec<String>,
}

// ---------------------------------------------------------------------------
// Response types
// ---------------------------------------------------------------------------

/// Response from the device authorization endpoint.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DeviceCodeResponse {
    /// The device verification code.
    pub device_code: String,

    /// The end-user verification code to display to the user.
    pub user_code: String,

    /// The URI the user should visit to enter the code.
    pub verification_uri: String,

    /// Optional complete URI with the user code pre-filled.
    pub verification_uri_complete: Option<String>,

    /// Lifetime of the device_code and user_code in seconds.
    pub expires_in: u64,

    /// The minimum polling interval in seconds.
    pub interval: u64,
}

/// Raw device authorization response from the server.
///
/// Some servers use `verification_url` instead of `verification_uri`.
#[derive(Debug, Deserialize)]
struct RawDeviceCodeResponse {
    device_code: String,
    user_code: String,
    verification_uri: Option<String>,
    verification_url: Option<String>,
    verification_uri_complete: Option<String>,
    expires_in: u64,
    #[serde(default = "default_interval")]
    interval: u64,
}

fn default_interval() -> u64 {
    5
}

/// Raw token response from the token endpoint (same shape as OAuth).
#[derive(Debug, Deserialize)]
struct TokenResponse {
    access_token: String,
    refresh_token: Option<String>,
    expires_in: Option<i64>,
    token_type: Option<String>,
    scope: Option<String>,
}

/// Error response from the token endpoint during device code polling.
#[derive(Debug, Deserialize)]
struct PollErrorResponse {
    error: String,
    #[allow(dead_code)]
    error_description: Option<String>,
}

// ---------------------------------------------------------------------------
// Device code flow
// ---------------------------------------------------------------------------

/// Manages an RFC 8628 device authorization grant flow.
pub struct DeviceCodeFlow {
    config: DeviceCodeConfig,
    client: reqwest::Client,
}

impl DeviceCodeFlow {
    /// Create a new device code flow with the given configuration.
    pub fn new(config: DeviceCodeConfig) -> Self {
        Self {
            config,
            client: reqwest::Client::new(),
        }
    }

    /// Request a device code from the authorization server.
    ///
    /// # Errors
    ///
    /// Returns [`AuthEngineError::NetworkError`] on transport failure, or
    /// [`AuthEngineError::FlowFailed`] if the server returns an error.
    pub async fn request_device_code(&self) -> Result<DeviceCodeResponse> {
        let mut params = vec![("client_id", self.config.client_id.as_str())];

        let scopes_joined;
        if !self.config.scopes.is_empty() {
            scopes_joined = self.config.scopes.join(" ");
            params.push(("scope", &scopes_joined));
        }

        tracing::debug!(
            device_auth_url = %self.config.device_auth_url,
            "requesting device code"
        );

        let response = self
            .client
            .post(&self.config.device_auth_url)
            .form(&params)
            .send()
            .await?;

        let status = response.status();
        if !status.is_success() {
            let body = response.text().await.unwrap_or_default();
            return Err(AuthEngineError::FlowFailed {
                reason: format!("device code request failed: HTTP {status}: {body}"),
            });
        }

        let raw: RawDeviceCodeResponse = response.json().await?;

        // Some providers use `verification_url` instead of `verification_uri`.
        let verification_uri = raw
            .verification_uri
            .or(raw.verification_url)
            .ok_or_else(|| AuthEngineError::FlowFailed {
                reason: "device code response missing verification_uri".to_string(),
            })?;

        Ok(DeviceCodeResponse {
            device_code: raw.device_code,
            user_code: raw.user_code,
            verification_uri,
            verification_uri_complete: raw.verification_uri_complete,
            expires_in: raw.expires_in,
            interval: raw.interval,
        })
    }

    /// Poll the token endpoint until the user completes authorization.
    ///
    /// Polls every `interval` seconds (increasing on `slow_down` responses)
    /// and gives up after `timeout` seconds.
    ///
    /// # Errors
    ///
    /// Returns [`AuthEngineError::FlowFailed`] if the user denies access or
    /// the device code expires, or [`AuthEngineError::CallbackTimeout`] if
    /// `timeout` seconds elapse.
    pub async fn poll_for_token(
        &self,
        device_code: &str,
        interval: u64,
        timeout: u64,
    ) -> Result<OAuthTokens> {
        let deadline = tokio::time::Instant::now() + tokio::time::Duration::from_secs(timeout);
        let mut current_interval = interval;

        tracing::debug!(
            interval = current_interval,
            timeout = timeout,
            "polling for device code token"
        );

        loop {
            // Sleep before polling (first poll also waits).
            tokio::time::sleep(tokio::time::Duration::from_secs(current_interval)).await;

            // Check if we have exceeded the timeout.
            if tokio::time::Instant::now() >= deadline {
                return Err(AuthEngineError::CallbackTimeout {
                    timeout_secs: timeout,
                });
            }

            let params = [
                ("grant_type", "urn:ietf:params:oauth:grant-type:device_code"),
                ("device_code", device_code),
                ("client_id", self.config.client_id.as_str()),
            ];

            let response = self
                .client
                .post(&self.config.token_url)
                .form(&params)
                .send()
                .await?;

            let status = response.status();

            if status.is_success() {
                let token: TokenResponse = response.json().await?;

                let expires_at = token
                    .expires_in
                    .map(|secs| chrono::Utc::now().timestamp() + secs);

                let scopes = token
                    .scope
                    .map(|s| s.split_whitespace().map(String::from).collect())
                    .unwrap_or_default();

                tracing::info!("device code flow completed successfully");

                return Ok(OAuthTokens {
                    access_token: token.access_token,
                    refresh_token: token.refresh_token,
                    expires_at,
                    token_type: token.token_type.unwrap_or_else(|| "Bearer".to_string()),
                    scopes,
                });
            }

            // Parse the error to decide whether to keep polling.
            let body = response.text().await.unwrap_or_default();

            let poll_error = serde_json::from_str::<PollErrorResponse>(&body).map_err(|_| {
                AuthEngineError::FlowFailed {
                    reason: format!("unexpected token response: HTTP {status}: {body}"),
                }
            })?;

            match poll_error.error.as_str() {
                "authorization_pending" => {
                    tracing::trace!("authorization pending, will retry");
                    // Continue polling at the same interval.
                }
                "slow_down" => {
                    // Increase interval by 5 seconds per RFC 8628 section 3.5.
                    current_interval += 5;
                    tracing::debug!(
                        new_interval = current_interval,
                        "slow_down received, increasing poll interval"
                    );
                }
                "access_denied" => {
                    return Err(AuthEngineError::FlowFailed {
                        reason: "user denied authorization".to_string(),
                    });
                }
                "expired_token" => {
                    return Err(AuthEngineError::FlowFailed {
                        reason: "device code expired before user completed authorization"
                            .to_string(),
                    });
                }
                other => {
                    return Err(AuthEngineError::FlowFailed {
                        reason: format!("device code poll error: {other}"),
                    });
                }
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

    fn test_config() -> DeviceCodeConfig {
        DeviceCodeConfig {
            client_id: "test-client".to_string(),
            device_auth_url: "https://auth.example.com/device/code".to_string(),
            token_url: "https://auth.example.com/token".to_string(),
            scopes: vec!["read".to_string(), "write".to_string()],
        }
    }

    #[test]
    fn device_code_response_parsing() {
        let json = r#"{
            "device_code": "dev_code_123",
            "user_code": "ABCD-1234",
            "verification_uri": "https://auth.example.com/device",
            "verification_uri_complete": "https://auth.example.com/device?user_code=ABCD-1234",
            "expires_in": 900,
            "interval": 5
        }"#;

        let raw: RawDeviceCodeResponse = serde_json::from_str(json).unwrap();
        assert_eq!(raw.device_code, "dev_code_123");
        assert_eq!(raw.user_code, "ABCD-1234");
        assert_eq!(
            raw.verification_uri.as_deref(),
            Some("https://auth.example.com/device")
        );
        assert!(raw.verification_uri_complete.is_some());
        assert_eq!(raw.expires_in, 900);
        assert_eq!(raw.interval, 5);
    }

    #[test]
    fn device_code_response_with_verification_url() {
        // Some providers use `verification_url` instead of `verification_uri`.
        let json = r#"{
            "device_code": "dev_xyz",
            "user_code": "WXYZ",
            "verification_url": "https://github.com/login/device",
            "expires_in": 600,
            "interval": 10
        }"#;

        let raw: RawDeviceCodeResponse = serde_json::from_str(json).unwrap();
        assert!(raw.verification_uri.is_none());
        assert_eq!(
            raw.verification_url.as_deref(),
            Some("https://github.com/login/device")
        );
    }

    #[test]
    fn device_code_response_default_interval() {
        let json = r#"{
            "device_code": "dev_abc",
            "user_code": "TEST",
            "verification_uri": "https://example.com/device",
            "expires_in": 300
        }"#;

        let raw: RawDeviceCodeResponse = serde_json::from_str(json).unwrap();
        assert_eq!(raw.interval, 5);
    }

    #[test]
    fn poll_error_response_parsing() {
        let json = r#"{
            "error": "authorization_pending",
            "error_description": "User has not yet authorized"
        }"#;

        let err: PollErrorResponse = serde_json::from_str(json).unwrap();
        assert_eq!(err.error, "authorization_pending");
        assert_eq!(
            err.error_description.as_deref(),
            Some("User has not yet authorized")
        );
    }

    #[test]
    fn poll_error_slow_down() {
        let json = r#"{ "error": "slow_down" }"#;

        let err: PollErrorResponse = serde_json::from_str(json).unwrap();
        assert_eq!(err.error, "slow_down");
    }

    #[test]
    fn poll_error_access_denied() {
        let json = r#"{ "error": "access_denied" }"#;

        let err: PollErrorResponse = serde_json::from_str(json).unwrap();
        assert_eq!(err.error, "access_denied");
    }

    #[test]
    fn poll_error_expired_token() {
        let json = r#"{ "error": "expired_token" }"#;

        let err: PollErrorResponse = serde_json::from_str(json).unwrap();
        assert_eq!(err.error, "expired_token");
    }

    #[test]
    fn token_response_parsing() {
        let json = r#"{
            "access_token": "access_tok",
            "refresh_token": "refresh_tok",
            "expires_in": 7200,
            "token_type": "Bearer",
            "scope": "read write"
        }"#;

        let token: TokenResponse = serde_json::from_str(json).unwrap();
        assert_eq!(token.access_token, "access_tok");
        assert_eq!(token.refresh_token.as_deref(), Some("refresh_tok"));
        assert_eq!(token.expires_in, Some(7200));
        assert_eq!(token.token_type.as_deref(), Some("Bearer"));
        assert_eq!(token.scope.as_deref(), Some("read write"));
    }

    #[test]
    fn device_code_flow_is_send_sync() {
        fn assert_send_sync<T: Send + Sync>() {}
        assert_send_sync::<DeviceCodeFlow>();
        assert_send_sync::<DeviceCodeConfig>();
        assert_send_sync::<DeviceCodeResponse>();
    }

    #[test]
    fn device_code_config_serialization_roundtrip() {
        let config = test_config();
        let json = serde_json::to_string(&config).unwrap();
        let deserialized: DeviceCodeConfig = serde_json::from_str(&json).unwrap();
        assert_eq!(deserialized.client_id, config.client_id);
        assert_eq!(deserialized.device_auth_url, config.device_auth_url);
        assert_eq!(deserialized.token_url, config.token_url);
        assert_eq!(deserialized.scopes, config.scopes);
    }
}
