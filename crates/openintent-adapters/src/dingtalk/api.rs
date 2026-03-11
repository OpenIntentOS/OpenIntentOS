//! DingTalk low-level API helpers.

use serde::Deserialize;
use tracing::debug;

use crate::error::{AdapterError, Result};

/// Response from the DingTalk token endpoint.
#[derive(Debug, Deserialize)]
pub struct AccessTokenResponse {
    pub access_token: String,
    pub expires_in: u64,
}

/// Fetch an app access token from DingTalk.
pub async fn fetch_access_token(
    client: &reqwest::Client,
    api_base: &str,
    app_key: &str,
    app_secret: &str,
) -> Result<AccessTokenResponse> {
    let url = format!("{api_base}/gettoken?appkey={app_key}&appsecret={app_secret}");
    debug!("fetching DingTalk access token");

    let resp: serde_json::Value = client
        .get(&url)
        .send()
        .await
        .map_err(|e| AdapterError::Internal(format!("DingTalk connect: {e}")))?
        .json()
        .await
        .map_err(|e| AdapterError::Internal(format!("DingTalk JSON parse: {e}")))?;

    let errcode = resp.get("errcode").and_then(|v| v.as_i64()).unwrap_or(-1);
    if errcode != 0 {
        let errmsg = resp
            .get("errmsg")
            .and_then(|v| v.as_str())
            .unwrap_or("unknown error");
        return Err(AdapterError::ExecutionFailed {
            tool_name: "fetch_access_token".to_string(),
            reason: format!("DingTalk API error {errcode}: {errmsg}"),
        });
    }

    let token = resp
        .get("access_token")
        .and_then(|v| v.as_str())
        .ok_or_else(|| AdapterError::ExecutionFailed {
            tool_name: "fetch_access_token".to_string(),
            reason: "missing access_token in DingTalk response".to_string(),
        })?
        .to_string();

    let expires_in = resp
        .get("expires_in")
        .and_then(|v| v.as_u64())
        .unwrap_or(7200);

    Ok(AccessTokenResponse { access_token: token, expires_in })
}

/// Check a DingTalk API response and return error if errcode != 0.
pub fn check_dingtalk_error(resp: &serde_json::Value, tool: &str) -> Result<()> {
    let errcode = resp.get("errcode").and_then(|v| v.as_i64()).unwrap_or(0);
    if errcode != 0 {
        let errmsg = resp
            .get("errmsg")
            .and_then(|v| v.as_str())
            .unwrap_or("unknown error");
        return Err(AdapterError::ExecutionFailed {
            tool_name: tool.to_string(),
            reason: format!("DingTalk API error {errcode}: {errmsg}"),
        });
    }
    Ok(())
}
