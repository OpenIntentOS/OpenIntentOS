//! XiaoHongShu note API functions.

use serde_json::{Value, json};
use tracing::{debug, info};

use crate::error::{AdapterError, Result};

use super::auth::build_signed_headers;

/// Publish a note to XiaoHongShu.
///
/// # Endpoint
/// `POST https://api.xiaohongshu.com/v2/notes/`
pub async fn publish_note(
    client: &reqwest::Client,
    api_base: &str,
    app_key: &str,
    app_secret: &str,
    params: &Value,
) -> Result<Value> {
    let path = "/v2/notes/";
    let body = serde_json::to_string(params).map_err(|e| AdapterError::ExecutionFailed {
        tool_name: "xhs_publish_note".to_string(),
        reason: format!("serialize params: {e}"),
    })?;

    let headers = build_signed_headers(app_key, app_secret, path, &[], &body);

    let url = format!("{api_base}{path}");
    debug!("publishing XHS note");

    let mut req = client.post(&url).body(body);
    for (key, val) in &headers {
        req = req.header(key.as_str(), val.as_str());
    }

    let resp: Value = req
        .send()
        .await
        .map_err(|e| AdapterError::ExecutionFailed {
            tool_name: "xhs_publish_note".to_string(),
            reason: format!("publish_note request: {e}"),
        })?
        .json()
        .await
        .map_err(|e| AdapterError::ExecutionFailed {
            tool_name: "xhs_publish_note".to_string(),
            reason: format!("publish_note parse: {e}"),
        })?;

    check_xhs_error(&resp, "xhs_publish_note")?;
    info!("XHS note published");
    Ok(resp)
}

/// Search notes by keyword.
///
/// # Endpoint
/// `GET https://api.xiaohongshu.com/v2/notes/search?keyword={}&count={}`
pub async fn search_notes(
    client: &reqwest::Client,
    api_base: &str,
    app_key: &str,
    app_secret: &str,
    keyword: &str,
    count: u32,
) -> Result<Value> {
    let path = "/v2/notes/search";
    let query_params = vec![
        ("keyword".to_string(), keyword.to_string()),
        ("count".to_string(), count.to_string()),
    ];

    let headers = build_signed_headers(app_key, app_secret, path, &query_params, "");

    let url = format!("{api_base}{path}?keyword={keyword}&count={count}");
    debug!(keyword, count, "searching XHS notes");

    let mut req = client.get(&url);
    for (key, val) in &headers {
        req = req.header(key.as_str(), val.as_str());
    }

    let resp: Value = req
        .send()
        .await
        .map_err(|e| AdapterError::ExecutionFailed {
            tool_name: "xhs_search_notes".to_string(),
            reason: format!("search_notes request: {e}"),
        })?
        .json()
        .await
        .map_err(|e| AdapterError::ExecutionFailed {
            tool_name: "xhs_search_notes".to_string(),
            reason: format!("search_notes parse: {e}"),
        })?;

    check_xhs_error(&resp, "xhs_search_notes")?;
    Ok(resp)
}

/// Get a note's stats and details by note_id.
///
/// # Endpoint
/// `GET https://api.xiaohongshu.com/v2/notes/{note_id}`
pub async fn get_note_stats(
    client: &reqwest::Client,
    api_base: &str,
    app_key: &str,
    app_secret: &str,
    note_id: &str,
) -> Result<Value> {
    let path = format!("/v2/notes/{note_id}");
    let headers = build_signed_headers(app_key, app_secret, &path, &[], "");

    let url = format!("{api_base}{path}");
    debug!(note_id, "fetching XHS note stats");

    let mut req = client.get(&url);
    for (key, val) in &headers {
        req = req.header(key.as_str(), val.as_str());
    }

    let resp: Value = req
        .send()
        .await
        .map_err(|e| AdapterError::ExecutionFailed {
            tool_name: "xhs_get_note_stats".to_string(),
            reason: format!("get_note_stats request: {e}"),
        })?
        .json()
        .await
        .map_err(|e| AdapterError::ExecutionFailed {
            tool_name: "xhs_get_note_stats".to_string(),
            reason: format!("get_note_stats parse: {e}"),
        })?;

    check_xhs_error(&resp, "xhs_get_note_stats")?;
    Ok(resp)
}

/// Get comments for a note.
///
/// # Endpoint
/// `GET https://api.xiaohongshu.com/v2/notes/{note_id}/comments`
pub async fn get_comments(
    client: &reqwest::Client,
    api_base: &str,
    app_key: &str,
    app_secret: &str,
    note_id: &str,
) -> Result<Value> {
    let path = format!("/v2/notes/{note_id}/comments");
    let headers = build_signed_headers(app_key, app_secret, &path, &[], "");

    let url = format!("{api_base}{path}");
    debug!(note_id, "fetching XHS note comments");

    let mut req = client.get(&url);
    for (key, val) in &headers {
        req = req.header(key.as_str(), val.as_str());
    }

    let resp: Value = req
        .send()
        .await
        .map_err(|e| AdapterError::ExecutionFailed {
            tool_name: "xhs_get_comments".to_string(),
            reason: format!("get_comments request: {e}"),
        })?
        .json()
        .await
        .map_err(|e| AdapterError::ExecutionFailed {
            tool_name: "xhs_get_comments".to_string(),
            reason: format!("get_comments parse: {e}"),
        })?;

    check_xhs_error(&resp, "xhs_get_comments")?;
    Ok(resp)
}

/// Check an XHS API response for error status.
fn check_xhs_error(resp: &Value, tool: &str) -> Result<()> {
    // XHS uses a `code` field — 0 or "0" or "success" means OK.
    if let Some(code) = resp.get("code") {
        let is_ok = code.as_i64().map(|n| n == 0).unwrap_or(false)
            || code.as_str().map(|s| s == "0" || s == "success").unwrap_or(false);
        if !is_ok {
            let msg = resp
                .get("message")
                .or_else(|| resp.get("msg"))
                .and_then(|v| v.as_str())
                .unwrap_or("unknown error");
            return Err(AdapterError::ExecutionFailed {
                tool_name: tool.to_string(),
                reason: format!("XHS API error: {code} — {msg}"),
            });
        }
    }
    Ok(())
}
