//! QQ Official Bot low-level API helpers.

use serde_json::Value;
use tracing::debug;

use crate::error::{AdapterError, Result};

/// Fetch a QQ Bot access token using AppID + AppSecret.
///
/// Calls `POST https://bots.qq.com/app/getAppAccessToken`.
/// Returns `(access_token, expires_in_seconds)`.
pub async fn fetch_access_token(
    client: &reqwest::Client,
    app_id: &str,
    app_secret: &str,
) -> Result<(String, u64)> {
    const URL: &str = "https://bots.qq.com/app/getAppAccessToken";
    debug!("fetching QQ Bot access token");

    let body = serde_json::json!({
        "appId": app_id,
        "clientSecret": app_secret
    });

    let resp: Value = client
        .post(URL)
        .json(&body)
        .send()
        .await
        .map_err(|e| AdapterError::Internal(format!("QQ Bot token request: {e}")))?
        .json()
        .await
        .map_err(|e| AdapterError::Internal(format!("QQ Bot token JSON parse: {e}")))?;

    let token = resp
        .get("access_token")
        .and_then(|v| v.as_str())
        .ok_or_else(|| AdapterError::ExecutionFailed {
            tool_name: "fetch_access_token".to_string(),
            reason: "missing access_token in QQ Bot response".to_string(),
        })?
        .to_string();

    // expires_in is returned as a string by the QQ Bot API.
    let expires_in = resp
        .get("expires_in")
        .and_then(|v| {
            if let Some(s) = v.as_str() {
                s.parse::<u64>().ok()
            } else {
                v.as_u64()
            }
        })
        .unwrap_or(7200);

    Ok((token, expires_in))
}

/// Check a QQ Bot API response for error fields.
///
/// The QQ Bot API uses HTTP status codes for errors; a successful JSON body
/// typically does not contain an "code" error field unless it is a platform
/// error envelope. This function checks for a non-zero `code` field.
pub fn check_qq_error(resp: &Value, tool: &str) -> Result<()> {
    // Some endpoints wrap errors as { "code": 304023, "message": "..." }
    if let Some(code) = resp.get("code").and_then(|v| v.as_i64()) {
        if code != 0 {
            let message = resp
                .get("message")
                .and_then(|v| v.as_str())
                .unwrap_or("unknown error");
            return Err(AdapterError::ExecutionFailed {
                tool_name: tool.to_string(),
                reason: format!("QQ Bot API error {code}: {message}"),
            });
        }
    }
    Ok(())
}
