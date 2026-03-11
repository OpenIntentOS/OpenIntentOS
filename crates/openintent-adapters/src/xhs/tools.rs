//! XiaoHongShu tool implementations and definitions.

use serde_json::{Value, json};

use crate::error::{AdapterError, Result};
use crate::traits::ToolDefinition;

use super::notes::{get_comments, get_note_stats, publish_note, search_notes};

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

/// Publish a note to XiaoHongShu.
pub async fn tool_publish_note(
    client: &reqwest::Client,
    api_base: &str,
    app_key: &str,
    app_secret: &str,
    params: &Value,
) -> Result<Value> {
    let title = required_str!(params, "title", "xhs_publish_note");
    let content = required_str!(params, "content", "xhs_publish_note");
    let note_type = params
        .get("type")
        .and_then(|v| v.as_str())
        .unwrap_or("normal");

    let body = json!({
        "title": title,
        "desc": content,
        "type": note_type
    });

    publish_note(client, api_base, app_key, app_secret, &body).await
}

/// Search notes by keyword.
pub async fn tool_search_notes(
    client: &reqwest::Client,
    api_base: &str,
    app_key: &str,
    app_secret: &str,
    params: &Value,
) -> Result<Value> {
    let keyword = required_str!(params, "keyword", "xhs_search_notes");
    let count_str = params
        .get("count")
        .and_then(|v| v.as_str())
        .unwrap_or("10");
    let count: u32 = count_str.parse().map_err(|_| AdapterError::InvalidParams {
        tool_name: "xhs_search_notes".to_string(),
        reason: format!("invalid count value: '{count_str}'"),
    })?;

    search_notes(client, api_base, app_key, app_secret, keyword, count).await
}

/// Get stats for a note by note_id.
pub async fn tool_get_note_stats(
    client: &reqwest::Client,
    api_base: &str,
    app_key: &str,
    app_secret: &str,
    params: &Value,
) -> Result<Value> {
    let note_id = required_str!(params, "note_id", "xhs_get_note_stats");
    get_note_stats(client, api_base, app_key, app_secret, note_id).await
}

/// Get comments for a note by note_id.
pub async fn tool_get_comments(
    client: &reqwest::Client,
    api_base: &str,
    app_key: &str,
    app_secret: &str,
    params: &Value,
) -> Result<Value> {
    let note_id = required_str!(params, "note_id", "xhs_get_comments");
    get_comments(client, api_base, app_key, app_secret, note_id).await
}

/// Build tool definitions for the XHS adapter.
pub fn build_tool_definitions() -> Vec<ToolDefinition> {
    vec![
        ToolDefinition {
            name: "xhs_publish_note".to_string(),
            description: "Publish a note (post) to XiaoHongShu. Supports image and normal text types.".to_string(),
            parameters: json!({
                "type": "object",
                "properties": {
                    "title": {
                        "type": "string",
                        "description": "Note title"
                    },
                    "content": {
                        "type": "string",
                        "description": "Note body / description text"
                    },
                    "type": {
                        "type": "string",
                        "description": "Note type: 'image' or 'normal' (default: 'normal')",
                        "default": "normal",
                        "enum": ["image", "normal"]
                    }
                },
                "required": ["title", "content"]
            }),
        },
        ToolDefinition {
            name: "xhs_search_notes".to_string(),
            description: "Search XiaoHongShu notes by keyword.".to_string(),
            parameters: json!({
                "type": "object",
                "properties": {
                    "keyword": {
                        "type": "string",
                        "description": "Search keyword"
                    },
                    "count": {
                        "type": "string",
                        "description": "Number of results to return (default: '10')",
                        "default": "10"
                    }
                },
                "required": ["keyword"]
            }),
        },
        ToolDefinition {
            name: "xhs_get_note_stats".to_string(),
            description: "Get statistics and details for a XiaoHongShu note by its note_id.".to_string(),
            parameters: json!({
                "type": "object",
                "properties": {
                    "note_id": {
                        "type": "string",
                        "description": "The note_id of the XiaoHongShu note"
                    }
                },
                "required": ["note_id"]
            }),
        },
        ToolDefinition {
            name: "xhs_get_comments".to_string(),
            description: "Get comments for a XiaoHongShu note by its note_id.".to_string(),
            parameters: json!({
                "type": "object",
                "properties": {
                    "note_id": {
                        "type": "string",
                        "description": "The note_id of the XiaoHongShu note"
                    }
                },
                "required": ["note_id"]
            }),
        },
    ]
}
