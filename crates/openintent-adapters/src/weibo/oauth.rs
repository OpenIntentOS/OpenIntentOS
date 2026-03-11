//! Weibo OAuth2 helpers.
//!
//! Implements the Weibo authorization code flow:
//!   1. Direct user to `build_auth_url()`.
//!   2. Exchange the returned code via `exchange_code()`.

use tracing::debug;

use crate::error::{AdapterError, Result};

use super::types::WeiboTokenResponse;

/// Build the Weibo OAuth2 authorization URL.
///
/// Returns a URL the user should open in their browser.
///
/// # Endpoint
/// `https://api.weibo.com/oauth2/authorize`
pub fn build_auth_url(app_key: &str, redirect_uri: &str) -> String {
    format!(
        "https://api.weibo.com/oauth2/authorize?client_id={app_key}&response_type=code&redirect_uri={redirect_uri}"
    )
}

/// Exchange an authorization code for a Weibo access token.
///
/// # Endpoint
/// `POST https://api.weibo.com/oauth2/access_token`
pub async fn exchange_code(
    client: &reqwest::Client,
    app_key: &str,
    app_secret: &str,
    code: &str,
    redirect_uri: &str,
) -> Result<WeiboTokenResponse> {
    debug!("exchanging Weibo authorization code for access token");

    let params = [
        ("client_id", app_key),
        ("client_secret", app_secret),
        ("grant_type", "authorization_code"),
        ("code", code),
        ("redirect_uri", redirect_uri),
    ];

    let resp: serde_json::Value = client
        .post("https://api.weibo.com/oauth2/access_token")
        .form(&params)
        .send()
        .await
        .map_err(|e| AdapterError::Internal(format!("Weibo exchange_code request: {e}")))?
        .json()
        .await
        .map_err(|e| AdapterError::Internal(format!("Weibo exchange_code parse: {e}")))?;

    // Check for error field in response.
    if let Some(error) = resp.get("error") {
        let description = resp
            .get("error_description")
            .and_then(|v| v.as_str())
            .unwrap_or("unknown error");
        return Err(AdapterError::ExecutionFailed {
            tool_name: "weibo_exchange_code".to_string(),
            reason: format!("Weibo OAuth error: {error} — {description}"),
        });
    }

    let token_resp: WeiboTokenResponse =
        serde_json::from_value(resp).map_err(|e| AdapterError::ExecutionFailed {
            tool_name: "weibo_exchange_code".to_string(),
            reason: format!("Weibo token deserialize: {e}"),
        })?;

    Ok(token_resp)
}
