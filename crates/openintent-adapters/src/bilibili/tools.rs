//! Bilibili Open Live Platform tool implementations.

use serde_json::{Value, json};
use tracing::info;

use crate::error::{AdapterError, Result};
use crate::traits::ToolDefinition;

use super::api::bili_api_post;

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

/// Get live room information.
///
/// Uses the public Bilibili API as a fallback (no auth required).
/// `room_id` is read from params or defaults to fetching via Open Live.
pub async fn tool_get_live_info(
    client: &reqwest::Client,
    key_id: &str,
    key_secret: &str,
    params: &Value,
) -> Result<Value> {
    const TOOL: &str = "bili_get_live_info";

    // Try to get room_id from params; fall through to a generic info call.
    let room_id = params
        .get("room_id")
        .and_then(|v| v.as_str())
        .unwrap_or("0");

    if room_id != "0" {
        // Public API fallback — no auth needed.
        let url = format!(
            "https://api.live.bilibili.com/room/v1/Room/get_info?room_id={room_id}"
        );
        let resp: Value = client
            .get(&url)
            .header("User-Agent", "OpenIntentOS/0.1")
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

        info!(tool = TOOL, room_id = room_id, "Bilibili live info fetched");
        return Ok(resp);
    }

    // If no room_id, call the Open Live start endpoint to discover the room.
    let body = json!({ "code": 0 });
    let resp = bili_api_post(client, key_id, key_secret, "/v2/app/start", &body).await?;
    info!(tool = TOOL, "Bilibili Open Live info fetched");
    Ok(resp)
}

/// Send a danmaku (弹幕) to a live room.
///
/// `POST https://live-open.biliapi.com/v2/app/send_msg`
pub async fn tool_send_danmaku(
    client: &reqwest::Client,
    key_id: &str,
    key_secret: &str,
    app_id: &str,
    params: &Value,
) -> Result<Value> {
    const TOOL: &str = "bili_send_danmaku";
    let room_id_str = required_str!(params, "room_id", TOOL);
    let content = required_str!(params, "content", TOOL);

    // Bilibili danmaku max length is 30 characters.
    if content.chars().count() > 30 {
        return Err(AdapterError::InvalidParams {
            tool_name: TOOL.to_string(),
            reason: "danmaku content exceeds 30 characters".to_string(),
        });
    }

    let room_id: u64 = room_id_str.parse().map_err(|_| AdapterError::InvalidParams {
        tool_name: TOOL.to_string(),
        reason: "room_id must be a numeric string".to_string(),
    })?;

    use std::time::{SystemTime, UNIX_EPOCH};
    let timestamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs();

    let body = json!({
        "room_id": room_id,
        "app_id": app_id.parse::<u64>().unwrap_or(0),
        "dm": {
            "color": 16777215,
            "font_size": 25,
            "mode": 0,
            "msg": content,
            "timestamp": timestamp,
            "emoji_unique_id": ""
        }
    });

    let resp = bili_api_post(client, key_id, key_secret, "/v2/app/send_msg", &body).await?;

    let code = resp.get("code").and_then(|v| v.as_i64()).unwrap_or(-1);
    if code != 0 {
        let message = resp
            .get("message")
            .and_then(|v| v.as_str())
            .unwrap_or("unknown error");
        return Err(AdapterError::ExecutionFailed {
            tool_name: TOOL.to_string(),
            reason: format!("Bilibili API error {code}: {message}"),
        });
    }

    info!(tool = TOOL, room_id = room_id, "Bilibili danmaku sent");
    Ok(json!({ "success": true, "room_id": room_id }))
}

/// Get recent danmaku history for a live room.
///
/// Uses the public Bilibili API: `GET /xlive/web-room/v1/dM/gethistory?roomid={room_id}`
pub async fn tool_get_danmaku_history(
    client: &reqwest::Client,
    params: &Value,
) -> Result<Value> {
    const TOOL: &str = "bili_get_danmaku_history";
    let room_id = required_str!(params, "room_id", TOOL);
    let count = params
        .get("count")
        .and_then(|v| v.as_str())
        .unwrap_or("20");

    let url = format!(
        "https://api.live.bilibili.com/xlive/web-room/v1/dM/gethistory?roomid={room_id}&count={count}"
    );

    let resp: Value = client
        .get(&url)
        .header("User-Agent", "OpenIntentOS/0.1")
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

    info!(tool = TOOL, room_id = room_id, "Bilibili danmaku history fetched");
    Ok(resp)
}

/// Build the static tool definitions for Bilibili.
pub fn build_tool_definitions() -> Vec<ToolDefinition> {
    vec![
        ToolDefinition {
            name: "bili_get_live_info".to_string(),
            description: "Get Bilibili live room status and information.".to_string(),
            parameters: json!({
                "type": "object",
                "properties": {
                    "room_id": {
                        "type": "string",
                        "description": "Live room ID. If omitted, uses the Open Live platform default."
                    }
                },
                "required": []
            }),
        },
        ToolDefinition {
            name: "bili_send_danmaku".to_string(),
            description: "Send a danmaku (弹幕) comment to a Bilibili live room via the Open Live Platform.".to_string(),
            parameters: json!({
                "type": "object",
                "properties": {
                    "room_id": { "type": "string", "description": "Target live room ID" },
                    "content": {
                        "type": "string",
                        "description": "Danmaku text, max 30 chars"
                    }
                },
                "required": ["room_id", "content"]
            }),
        },
        ToolDefinition {
            name: "bili_get_danmaku_history".to_string(),
            description: "Get recent danmaku history for a Bilibili live room.".to_string(),
            parameters: json!({
                "type": "object",
                "properties": {
                    "room_id": { "type": "string", "description": "Target live room ID" },
                    "count": {
                        "type": "string",
                        "description": "Number of danmaku to fetch, default 20"
                    }
                },
                "required": ["room_id"]
            }),
        },
    ]
}
