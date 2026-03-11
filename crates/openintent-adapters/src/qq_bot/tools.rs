//! QQ Official Bot tool implementations.

use serde_json::{Value, json};
use tracing::info;

use crate::error::{AdapterError, Result};
use crate::traits::ToolDefinition;

use super::api::check_qq_error;

const QQ_API_BASE: &str = "https://api.sgroup.qq.com";

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

/// Send a text message to a QQ channel.
///
/// `POST https://api.sgroup.qq.com/channels/{channel_id}/messages`
pub async fn tool_send_group_message(
    client: &reqwest::Client,
    token: &str,
    params: &Value,
) -> Result<Value> {
    const TOOL: &str = "qq_send_group_message";
    let channel_id = required_str!(params, "channel_id", TOOL);
    let content = required_str!(params, "content", TOOL);

    let url = format!("{QQ_API_BASE}/channels/{channel_id}/messages");
    let body = json!({ "content": content });

    let resp: Value = client
        .post(&url)
        .header("Authorization", format!("QQBot {token}"))
        .json(&body)
        .send()
        .await
        .map_err(|e| AdapterError::ExecutionFailed {
            tool_name: TOOL.to_string(),
            reason: e.to_string(),
        })?
        .json()
        .await
        .map_err(|e| AdapterError::ExecutionFailed {
            tool_name: TOOL.to_string(),
            reason: format!("JSON parse: {e}"),
        })?;

    check_qq_error(&resp, TOOL)?;
    info!(tool = TOOL, channel_id = channel_id, "QQ Bot group message sent");
    Ok(json!({ "success": true, "channel_id": channel_id }))
}

/// Send a C2C (direct) text message to a user by openid.
///
/// `POST https://api.sgroup.qq.com/v2/users/{openid}/messages`
pub async fn tool_send_c2c_message(
    client: &reqwest::Client,
    token: &str,
    params: &Value,
) -> Result<Value> {
    const TOOL: &str = "qq_send_c2c_message";
    let openid = required_str!(params, "openid", TOOL);
    let content = required_str!(params, "content", TOOL);

    let url = format!("{QQ_API_BASE}/v2/users/{openid}/messages");
    let body = json!({ "content": content, "msg_type": 0 });

    let resp: Value = client
        .post(&url)
        .header("Authorization", format!("QQBot {token}"))
        .json(&body)
        .send()
        .await
        .map_err(|e| AdapterError::ExecutionFailed {
            tool_name: TOOL.to_string(),
            reason: e.to_string(),
        })?
        .json()
        .await
        .map_err(|e| AdapterError::ExecutionFailed {
            tool_name: TOOL.to_string(),
            reason: format!("JSON parse: {e}"),
        })?;

    check_qq_error(&resp, TOOL)?;
    info!(tool = TOOL, openid = openid, "QQ Bot C2C message sent");
    Ok(json!({ "success": true, "openid": openid }))
}

/// Send an image message to a QQ channel.
///
/// `POST https://api.sgroup.qq.com/channels/{channel_id}/messages`
/// with `msg_type: 1` and an image URL.
pub async fn tool_send_image_message(
    client: &reqwest::Client,
    token: &str,
    params: &Value,
) -> Result<Value> {
    const TOOL: &str = "qq_send_image";
    let channel_id = required_str!(params, "channel_id", TOOL);
    let image_url = required_str!(params, "image_url", TOOL);

    let url = format!("{QQ_API_BASE}/channels/{channel_id}/messages");
    let body = json!({ "image": image_url, "msg_type": 1 });

    let resp: Value = client
        .post(&url)
        .header("Authorization", format!("QQBot {token}"))
        .json(&body)
        .send()
        .await
        .map_err(|e| AdapterError::ExecutionFailed {
            tool_name: TOOL.to_string(),
            reason: e.to_string(),
        })?
        .json()
        .await
        .map_err(|e| AdapterError::ExecutionFailed {
            tool_name: TOOL.to_string(),
            reason: format!("JSON parse: {e}"),
        })?;

    check_qq_error(&resp, TOOL)?;
    info!(tool = TOOL, channel_id = channel_id, "QQ Bot image message sent");
    Ok(json!({ "success": true, "channel_id": channel_id }))
}

/// Build the static tool definitions for QQ Bot.
pub fn build_tool_definitions() -> Vec<ToolDefinition> {
    vec![
        ToolDefinition {
            name: "qq_send_group_message".to_string(),
            description: "Send a text message to a QQ Official Bot channel.".to_string(),
            parameters: json!({
                "type": "object",
                "properties": {
                    "channel_id": { "type": "string", "description": "Target channel ID" },
                    "content": { "type": "string", "description": "Message text content" }
                },
                "required": ["channel_id", "content"]
            }),
        },
        ToolDefinition {
            name: "qq_send_c2c_message".to_string(),
            description: "Send a direct (C2C) text message to a QQ user by openid.".to_string(),
            parameters: json!({
                "type": "object",
                "properties": {
                    "openid": { "type": "string", "description": "Target user openid" },
                    "content": { "type": "string", "description": "Message text content" }
                },
                "required": ["openid", "content"]
            }),
        },
        ToolDefinition {
            name: "qq_send_image".to_string(),
            description: "Send an image message to a QQ Official Bot channel.".to_string(),
            parameters: json!({
                "type": "object",
                "properties": {
                    "channel_id": { "type": "string", "description": "Target channel ID" },
                    "image_url": { "type": "string", "description": "Public URL of the image to send" }
                },
                "required": ["channel_id", "image_url"]
            }),
        },
    ]
}
