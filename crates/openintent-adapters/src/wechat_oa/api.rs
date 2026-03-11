//! WeChat OA low-level API helpers.

use serde::Deserialize;
use tracing::debug;

use crate::error::{AdapterError, Result};

/// Response from the `token` endpoint.
#[derive(Debug, Deserialize)]
pub struct AccessTokenResponse {
    pub access_token: String,
    pub expires_in: u64,
}

/// Fetch a new access token from WeChat.
pub async fn fetch_access_token(
    client: &reqwest::Client,
    api_base: &str,
    app_id: &str,
    app_secret: &str,
) -> Result<AccessTokenResponse> {
    let url = format!(
        "{api_base}/token?grant_type=client_credential&appid={app_id}&secret={app_secret}"
    );
    debug!("fetching WeChat OA access token");

    let resp: serde_json::Value = client
        .get(&url)
        .send()
        .await
        .map_err(|e| AdapterError::Internal(format!("WeChat OA connect: {e}")))?
        .json()
        .await
        .map_err(|e| AdapterError::Internal(format!("WeChat OA JSON parse: {e}")))?;

    if let Some(code) = resp.get("errcode").and_then(|v| v.as_i64()) {
        if code != 0 {
            let msg = resp
                .get("errmsg")
                .and_then(|v| v.as_str())
                .unwrap_or("unknown error");
            return Err(AdapterError::ExecutionFailed {
                tool_name: "fetch_access_token".to_string(),
                reason: format!("WeChat API error {code}: {msg}"),
            });
        }
    }

    let token = resp
        .get("access_token")
        .and_then(|v| v.as_str())
        .ok_or_else(|| AdapterError::ExecutionFailed {
            tool_name: "fetch_access_token".to_string(),
            reason: "missing access_token in response".to_string(),
        })?
        .to_string();

    let expires_in = resp
        .get("expires_in")
        .and_then(|v| v.as_u64())
        .unwrap_or(7200);

    Ok(AccessTokenResponse { access_token: token, expires_in })
}

/// Check a WeChat API JSON response for error fields and return an error if present.
pub fn check_wechat_error(resp: &serde_json::Value, tool: &str) -> Result<()> {
    if let Some(code) = resp.get("errcode").and_then(|v| v.as_i64()) {
        if code != 0 {
            let msg = resp
                .get("errmsg")
                .and_then(|v| v.as_str())
                .unwrap_or("unknown error");
            return Err(AdapterError::ExecutionFailed {
                tool_name: tool.to_string(),
                reason: format!("WeChat API error {code}: {msg}"),
            });
        }
    }
    Ok(())
}
