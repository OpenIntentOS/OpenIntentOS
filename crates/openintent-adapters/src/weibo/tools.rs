//! Weibo tool implementations and definitions.

use serde_json::{Value, json};
use tracing::info;

use crate::error::{AdapterError, Result};
use crate::traits::ToolDefinition;

use super::oauth::build_auth_url;

macro_rules! required_str {
    ($params:expr, $field:expr, $tool:expr) => {
        $params
            .get($field)
            .and_then(|v| v.as_str())
            .ok_or_else(|| AdapterError::InvalidParams {
                tool_name: $tool.to_string(),
                reason: format!("missing required field `{}`", $field),
            })?
    };
}

macro_rules! exec_err {
    ($tool:expr, $e:expr) => {
        AdapterError::ExecutionFailed {
            tool_name: $tool.to_string(),
            reason: $e.to_string(),
        }
    };
}

/// Post a status update to Weibo (weibo_post).
///
/// # Endpoint
/// `POST https://api.weibo.com/2/statuses/share.json`
pub async fn tool_post(
    client: &reqwest::Client,
    api_base: &str,
    access_token: &str,
    params: &Value,
) -> Result<Value> {
    let content = required_str!(params, "content", "weibo_post");

    if content.len() > 140 {
        return Err(AdapterError::InvalidParams {
            tool_name: "weibo_post".to_string(),
            reason: format!("content exceeds 140 characters (got {})", content.len()),
        });
    }

    let url = format!("{api_base}/statuses/share.json");
    let form_params = [("access_token", access_token), ("status", content)];

    let resp: Value = client
        .post(&url)
        .form(&form_params)
        .send()
        .await
        .map_err(|e| exec_err!("weibo_post", e))?
        .json()
        .await
        .map_err(|e| exec_err!("weibo_post", e))?;

    check_weibo_error(&resp, "weibo_post")?;

    let id = resp.get("id").and_then(|v| v.as_u64()).unwrap_or(0);
    info!(id, "Weibo status posted");
    Ok(json!({ "success": true, "id": id }))
}

/// Get mentions timeline (weibo_get_mentions).
///
/// # Endpoint
/// `GET https://api.weibo.com/2/statuses/mentions.json`
pub async fn tool_get_mentions(
    client: &reqwest::Client,
    api_base: &str,
    access_token: &str,
    params: &Value,
) -> Result<Value> {
    let count_str = params
        .get("count")
        .and_then(|v| v.as_str())
        .unwrap_or("10");
    let count: u32 = count_str.parse().map_err(|_| AdapterError::InvalidParams {
        tool_name: "weibo_get_mentions".to_string(),
        reason: format!("invalid count value: '{count_str}'"),
    })?;

    let url = format!(
        "{api_base}/statuses/mentions.json?access_token={access_token}&count={count}"
    );

    let resp: Value = client
        .get(&url)
        .send()
        .await
        .map_err(|e| exec_err!("weibo_get_mentions", e))?
        .json()
        .await
        .map_err(|e| exec_err!("weibo_get_mentions", e))?;

    check_weibo_error(&resp, "weibo_get_mentions")?;
    Ok(resp)
}

/// Reply to a comment on a Weibo post (weibo_reply_comment).
///
/// # Endpoint
/// `POST https://api.weibo.com/2/comments/create.json`
pub async fn tool_reply_comment(
    client: &reqwest::Client,
    api_base: &str,
    access_token: &str,
    params: &Value,
) -> Result<Value> {
    let weibo_id = required_str!(params, "weibo_id", "weibo_reply_comment");
    let comment = required_str!(params, "comment", "weibo_reply_comment");

    let url = format!("{api_base}/comments/create.json");
    let form_params = [
        ("access_token", access_token),
        ("id", weibo_id),
        ("comment", comment),
    ];

    let resp: Value = client
        .post(&url)
        .form(&form_params)
        .send()
        .await
        .map_err(|e| exec_err!("weibo_reply_comment", e))?
        .json()
        .await
        .map_err(|e| exec_err!("weibo_reply_comment", e))?;

    check_weibo_error(&resp, "weibo_reply_comment")?;

    let id = resp.get("id").and_then(|v| v.as_u64()).unwrap_or(0);
    info!(id, weibo_id, "Weibo comment posted");
    Ok(json!({ "success": true, "comment_id": id }))
}

/// Return the OAuth2 authorization URL for the user to grant Weibo access.
pub fn tool_get_auth_url(app_key: Option<&str>, params: &Value) -> Result<Value> {
    let redirect_uri = params
        .get("redirect_uri")
        .and_then(|v| v.as_str())
        .ok_or_else(|| AdapterError::InvalidParams {
            tool_name: "weibo_get_auth_url".to_string(),
            reason: "missing required field `redirect_uri`".to_string(),
        })?;

    let key = app_key.ok_or_else(|| AdapterError::AuthRequired {
        adapter_id: "weibo".to_string(),
        provider: "weibo".to_string(),
    })?;

    let url = build_auth_url(key, redirect_uri);
    Ok(json!({ "auth_url": url }))
}

/// Check a Weibo API response for error fields.
fn check_weibo_error(resp: &Value, tool: &str) -> Result<()> {
    if let Some(error) = resp.get("error") {
        let description = resp
            .get("error_description")
            .and_then(|v| v.as_str())
            .unwrap_or("unknown error");
        return Err(AdapterError::ExecutionFailed {
            tool_name: tool.to_string(),
            reason: format!("Weibo API error: {error} — {description}"),
        });
    }
    // Also check numeric error code.
    if let Some(code) = resp.get("error_code").and_then(|v| v.as_i64()) {
        if code != 0 {
            let msg = resp
                .get("error")
                .and_then(|v| v.as_str())
                .unwrap_or("unknown error");
            return Err(AdapterError::ExecutionFailed {
                tool_name: tool.to_string(),
                reason: format!("Weibo API error {code}: {msg}"),
            });
        }
    }
    Ok(())
}

/// Build tool definitions for the Weibo adapter.
pub fn build_tool_definitions() -> Vec<ToolDefinition> {
    vec![
        ToolDefinition {
            name: "weibo_get_auth_url".to_string(),
            description: "Generate the Weibo OAuth2 authorization URL. Direct the user to this URL to grant access. No token required.".to_string(),
            parameters: json!({
                "type": "object",
                "properties": {
                    "redirect_uri": {
                        "type": "string",
                        "description": "The redirect URI registered in your Weibo app settings"
                    }
                },
                "required": ["redirect_uri"]
            }),
        },
        ToolDefinition {
            name: "weibo_post".to_string(),
            description: "Post a status update to Weibo (maximum 140 characters).".to_string(),
            parameters: json!({
                "type": "object",
                "properties": {
                    "content": {
                        "type": "string",
                        "description": "Status text to post (max 140 characters)"
                    }
                },
                "required": ["content"]
            }),
        },
        ToolDefinition {
            name: "weibo_get_mentions".to_string(),
            description: "Get recent Weibo posts that mention the authenticated user.".to_string(),
            parameters: json!({
                "type": "object",
                "properties": {
                    "count": {
                        "type": "string",
                        "description": "Number of mentions to return (default: '10')",
                        "default": "10"
                    }
                }
            }),
        },
        ToolDefinition {
            name: "weibo_reply_comment".to_string(),
            description: "Post a reply comment to a specific Weibo post.".to_string(),
            parameters: json!({
                "type": "object",
                "properties": {
                    "weibo_id": {
                        "type": "string",
                        "description": "The ID of the Weibo post to comment on"
                    },
                    "comment": {
                        "type": "string",
                        "description": "Comment text to post"
                    }
                },
                "required": ["weibo_id", "comment"]
            }),
        },
    ]
}
