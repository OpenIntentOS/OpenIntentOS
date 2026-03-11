//! Douyin tool implementations and definitions.

use serde_json::{Value, json};
use tracing::info;

use crate::error::{AdapterError, Result};
use crate::traits::ToolDefinition;

use super::content::{create_video, get_video_list, get_video_stats, upload_video};
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

/// Publish a video to Douyin (upload + create in one step).
pub async fn tool_publish_video(
    client: &reqwest::Client,
    api_base: &str,
    access_token: &str,
    open_id: &str,
    params: &Value,
) -> Result<Value> {
    let file_path = required_str!(params, "file_path", "douyin_publish_video");
    let caption = required_str!(params, "caption", "douyin_publish_video");

    let video_id = upload_video(client, api_base, access_token, open_id, file_path).await?;
    let resp = create_video(client, api_base, access_token, open_id, &video_id, caption).await?;

    let item_id = resp
        .get("data")
        .and_then(|d| d.get("item_id"))
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .to_string();

    info!(item_id, "Douyin video published");
    Ok(json!({
        "success": true,
        "item_id": item_id,
        "video_id": video_id
    }))
}

/// Get the authenticated user's video list.
pub async fn tool_get_video_list(
    client: &reqwest::Client,
    api_base: &str,
    access_token: &str,
    open_id: &str,
    params: &Value,
) -> Result<Value> {
    let count_str = params
        .get("count")
        .and_then(|v| v.as_str())
        .unwrap_or("10");
    let count: u32 = count_str.parse().map_err(|_| AdapterError::InvalidParams {
        tool_name: "douyin_get_video_list".to_string(),
        reason: format!("invalid count value: '{count_str}'"),
    })?;

    get_video_list(client, api_base, access_token, open_id, count).await
}

/// Get stats for a specific video by item_id.
pub async fn tool_get_video_stats(
    client: &reqwest::Client,
    api_base: &str,
    access_token: &str,
    open_id: &str,
    params: &Value,
) -> Result<Value> {
    let item_id = required_str!(params, "item_id", "douyin_get_video_stats");
    get_video_stats(client, api_base, access_token, open_id, &[item_id]).await
}

/// Return the OAuth2 authorization URL for the user to grant access.
pub fn tool_get_auth_url(client_key: &str, params: &Value) -> Result<Value> {
    let redirect_uri = params
        .get("redirect_uri")
        .and_then(|v| v.as_str())
        .ok_or_else(|| AdapterError::InvalidParams {
            tool_name: "douyin_get_auth_url".to_string(),
            reason: "missing required field `redirect_uri`".to_string(),
        })?;

    let url = build_auth_url(
        client_key,
        redirect_uri,
        &["video.create", "video.list", "video.data.bind"],
    );

    Ok(json!({ "auth_url": url }))
}

/// Build the list of tool definitions exposed by the Douyin adapter.
pub fn build_tool_definitions() -> Vec<ToolDefinition> {
    vec![
        ToolDefinition {
            name: "douyin_get_auth_url".to_string(),
            description: "Generate the Douyin OAuth2 authorization URL. Direct the user to this URL to grant access. No token required.".to_string(),
            parameters: json!({
                "type": "object",
                "properties": {
                    "redirect_uri": {
                        "type": "string",
                        "description": "The redirect URI registered in your Douyin app settings"
                    }
                },
                "required": ["redirect_uri"]
            }),
        },
        ToolDefinition {
            name: "douyin_publish_video".to_string(),
            description: "Upload and publish a video to Douyin. Returns the item_id of the published post.".to_string(),
            parameters: json!({
                "type": "object",
                "properties": {
                    "file_path": {
                        "type": "string",
                        "description": "Absolute local path to the video file to upload"
                    },
                    "caption": {
                        "type": "string",
                        "description": "Video caption/description text"
                    }
                },
                "required": ["file_path", "caption"]
            }),
        },
        ToolDefinition {
            name: "douyin_get_video_list".to_string(),
            description: "Get the list of videos for the authenticated Douyin user.".to_string(),
            parameters: json!({
                "type": "object",
                "properties": {
                    "count": {
                        "type": "string",
                        "description": "Number of videos to return (default: 10)",
                        "default": "10"
                    }
                }
            }),
        },
        ToolDefinition {
            name: "douyin_get_video_stats".to_string(),
            description: "Get performance statistics (plays, likes, comments, shares) for a specific Douyin video by item_id.".to_string(),
            parameters: json!({
                "type": "object",
                "properties": {
                    "item_id": {
                        "type": "string",
                        "description": "The item_id of the Douyin video to get stats for"
                    }
                },
                "required": ["item_id"]
            }),
        },
    ]
}
