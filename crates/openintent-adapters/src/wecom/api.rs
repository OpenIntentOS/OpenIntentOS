//! WeCom (企业微信) low-level API helpers.

use serde_json::Value;
use tracing::debug;

use crate::error::{AdapterError, Result};

/// Fetch a WeCom access token using CorpID + CorpSecret.
///
/// Returns `(access_token, expires_in_seconds)`.
pub async fn fetch_access_token(
    client: &reqwest::Client,
    corp_id: &str,
    corp_secret: &str,
) -> Result<(String, u64)> {
    let url = format!(
        "https://qyapi.weixin.qq.com/cgi-bin/gettoken?corpid={corp_id}&corpsecret={corp_secret}"
    );
    debug!("fetching WeCom access token");

    let resp: Value = client
        .get(&url)
        .send()
        .await
        .map_err(|e| AdapterError::Internal(format!("WeCom token request: {e}")))?
        .json()
        .await
        .map_err(|e| AdapterError::Internal(format!("WeCom token JSON parse: {e}")))?;

    let errcode = resp.get("errcode").and_then(|v| v.as_i64()).unwrap_or(-1);
    if errcode != 0 {
        let errmsg = resp
            .get("errmsg")
            .and_then(|v| v.as_str())
            .unwrap_or("unknown error");
        return Err(AdapterError::ExecutionFailed {
            tool_name: "fetch_access_token".to_string(),
            reason: format!("WeCom API error {errcode}: {errmsg}"),
        });
    }

    let token = resp
        .get("access_token")
        .and_then(|v| v.as_str())
        .ok_or_else(|| AdapterError::ExecutionFailed {
            tool_name: "fetch_access_token".to_string(),
            reason: "missing access_token in WeCom response".to_string(),
        })?
        .to_string();

    let expires_in = resp
        .get("expires_in")
        .and_then(|v| v.as_u64())
        .unwrap_or(7200);

    Ok((token, expires_in))
}

/// Check a WeCom API response for errcode == 0.
pub fn check_wecom_error(resp: &Value, tool: &str) -> Result<()> {
    let errcode = resp.get("errcode").and_then(|v| v.as_i64()).unwrap_or(0);
    if errcode != 0 {
        let errmsg = resp
            .get("errmsg")
            .and_then(|v| v.as_str())
            .unwrap_or("unknown error");
        return Err(AdapterError::ExecutionFailed {
            tool_name: tool.to_string(),
            reason: format!("WeCom API error {errcode}: {errmsg}"),
        });
    }
    Ok(())
}
