//! Douyin OAuth2 helpers.
//!
//! Implements the authorization code flow for the Douyin Open Platform:
//!   1. Direct user to `build_auth_url()`.
//!   2. Exchange the returned code via `exchange_code()`.
//!   3. Refresh expiring tokens via `refresh_token()`.

use serde_json::Value;
use tracing::debug;

use crate::error::{AdapterError, Result};

use super::types::DouyinTokenResponse;

/// Build the authorization URL for user OAuth2 flow.
///
/// Returns a URL the user should open in their browser.
pub fn build_auth_url(client_key: &str, redirect_uri: &str, scopes: &[&str]) -> String {
    let scope = scopes.join(",");
    format!(
        "https://open.douyin.com/platform/oauth/connect/?client_key={client_key}&response_type=code&scope={scope}&redirect_uri={redirect_uri}"
    )
}

/// Exchange an authorization code for an access token.
///
/// # Endpoint
/// `POST https://open.douyin.com/oauth/access_token/`
pub async fn exchange_code(
    client: &reqwest::Client,
    api_base: &str,
    client_key: &str,
    client_secret: &str,
    code: &str,
) -> Result<DouyinTokenResponse> {
    debug!("exchanging Douyin authorization code for access token");

    let url = format!("{api_base}/oauth/access_token/");
    let body = serde_json::json!({
        "client_key": client_key,
        "client_secret": client_secret,
        "code": code,
        "grant_type": "authorization_code"
    });

    let resp: Value = client
        .post(&url)
        .json(&body)
        .send()
        .await
        .map_err(|e| AdapterError::Internal(format!("Douyin exchange_code request: {e}")))?
        .json()
        .await
        .map_err(|e| AdapterError::Internal(format!("Douyin exchange_code parse: {e}")))?;

    parse_token_response(resp, "exchange_code")
}

/// Refresh an expired Douyin user access token.
///
/// # Endpoint
/// `POST https://open.douyin.com/oauth/refresh_token/`
pub async fn refresh_token(
    client: &reqwest::Client,
    api_base: &str,
    client_key: &str,
    refresh_token_value: &str,
) -> Result<DouyinTokenResponse> {
    debug!("refreshing Douyin access token");

    let url = format!("{api_base}/oauth/refresh_token/");
    let body = serde_json::json!({
        "client_key": client_key,
        "grant_type": "refresh_token",
        "refresh_token": refresh_token_value
    });

    let resp: Value = client
        .post(&url)
        .json(&body)
        .send()
        .await
        .map_err(|e| AdapterError::Internal(format!("Douyin refresh_token request: {e}")))?
        .json()
        .await
        .map_err(|e| AdapterError::Internal(format!("Douyin refresh_token parse: {e}")))?;

    parse_token_response(resp, "refresh_token")
}

/// Parse a Douyin token API response into `DouyinTokenResponse`.
fn parse_token_response(resp: Value, context: &str) -> Result<DouyinTokenResponse> {
    let message = resp
        .get("message")
        .and_then(|v| v.as_str())
        .unwrap_or("unknown");

    let data = resp.get("data").ok_or_else(|| AdapterError::ExecutionFailed {
        tool_name: context.to_string(),
        reason: format!("Douyin token API missing 'data': message={message}"),
    })?;

    // Check for error_code inside data
    if let Some(code) = data.get("error_code").and_then(|v| v.as_i64()) {
        if code != 0 {
            let description = data
                .get("description")
                .and_then(|v| v.as_str())
                .unwrap_or("unknown error");
            return Err(AdapterError::ExecutionFailed {
                tool_name: context.to_string(),
                reason: format!("Douyin API error {code}: {description}"),
            });
        }
    }

    let token_resp: DouyinTokenResponse =
        serde_json::from_value(data.clone()).map_err(|e| AdapterError::ExecutionFailed {
            tool_name: context.to_string(),
            reason: format!("Douyin token deserialize: {e}"),
        })?;

    Ok(token_resp)
}
