//! WeCom (企业微信) tool implementations.

use serde_json::{Value, json};
use tracing::info;

use crate::error::{AdapterError, Result};
use crate::traits::ToolDefinition;

use super::api::check_wecom_error;

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

/// WeCom integration mode, passed into tool functions.
#[derive(Debug, Clone, PartialEq)]
pub enum WeComMode {
    App { agent_id: u64 },
    Webhook,
}

/// Send a text message.
///
/// App mode: POST to `/cgi-bin/message/send` with access_token.
/// Webhook mode: POST to webhook_url.
pub async fn tool_send_text(
    client: &reqwest::Client,
    endpoint: &str,
    mode: &WeComMode,
    token: Option<&str>,
    params: &Value,
) -> Result<Value> {
    const TOOL: &str = "wecom_send_text";
    let content = required_str!(params, "content", TOOL).to_string();

    let (url, body) = match mode {
        WeComMode::App { agent_id } => {
            let token = token.ok_or_else(|| AdapterError::AuthRequired {
                adapter_id: "wecom".to_string(),
                provider: "wecom".to_string(),
            })?;
            let url = format!("{endpoint}/cgi-bin/message/send?access_token={token}");
            let body = json!({
                "touser": "@all",
                "msgtype": "text",
                "agentid": agent_id,
                "text": { "content": content }
            });
            (url, body)
        }
        WeComMode::Webhook => {
            let body = json!({
                "msgtype": "text",
                "text": { "content": content }
            });
            (endpoint.to_string(), body)
        }
    };

    let resp: Value = client
        .post(&url)
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

    check_wecom_error(&resp, TOOL)?;
    info!(tool = TOOL, "WeCom text message sent");
    Ok(json!({ "success": true }))
}

/// Send a markdown message.
pub async fn tool_send_markdown(
    client: &reqwest::Client,
    endpoint: &str,
    mode: &WeComMode,
    token: Option<&str>,
    params: &Value,
) -> Result<Value> {
    const TOOL: &str = "wecom_send_markdown";
    let content = required_str!(params, "content", TOOL).to_string();

    let (url, body) = match mode {
        WeComMode::App { agent_id } => {
            let token = token.ok_or_else(|| AdapterError::AuthRequired {
                adapter_id: "wecom".to_string(),
                provider: "wecom".to_string(),
            })?;
            let url = format!("{endpoint}/cgi-bin/message/send?access_token={token}");
            let body = json!({
                "touser": "@all",
                "msgtype": "markdown",
                "agentid": agent_id,
                "markdown": { "content": content }
            });
            (url, body)
        }
        WeComMode::Webhook => {
            let body = json!({
                "msgtype": "markdown",
                "markdown": { "content": content }
            });
            (endpoint.to_string(), body)
        }
    };

    let resp: Value = client
        .post(&url)
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

    check_wecom_error(&resp, TOOL)?;
    info!(tool = TOOL, "WeCom markdown message sent");
    Ok(json!({ "success": true }))
}

/// Send a file notice (text message describing the file).
///
/// Direct file upload to WeCom requires pre-uploading the file for a media_id.
/// This tool sends a text message noting the file instead.
pub async fn tool_send_file(
    client: &reqwest::Client,
    endpoint: &str,
    mode: &WeComMode,
    token: Option<&str>,
    params: &Value,
) -> Result<Value> {
    const TOOL: &str = "wecom_send_file";
    let content = required_str!(params, "content", TOOL).to_string();

    // Delegate to text send with a note prefix.
    let text_params = json!({ "content": content });
    tool_send_text(client, endpoint, mode, token, &text_params).await
}

/// Get department member list.
///
/// Requires App mode and a valid access token.
/// Calls `GET /cgi-bin/user/simplelist?access_token={}&department_id={id}`.
pub async fn tool_get_members(
    client: &reqwest::Client,
    endpoint: &str,
    token: &str,
    params: &Value,
) -> Result<Value> {
    const TOOL: &str = "wecom_get_members";
    let department_id = params
        .get("department_id")
        .and_then(|v| v.as_str())
        .unwrap_or("1");

    let url = format!(
        "{endpoint}/cgi-bin/user/simplelist?access_token={token}&department_id={department_id}"
    );

    let resp: Value = client
        .get(&url)
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

    check_wecom_error(&resp, TOOL)?;
    info!(tool = TOOL, department_id = department_id, "WeCom members fetched");
    Ok(resp)
}

/// Build the static tool definitions for WeCom.
pub fn build_tool_definitions() -> Vec<ToolDefinition> {
    vec![
        ToolDefinition {
            name: "wecom_send_text".to_string(),
            description: "Send a text message to a WeCom group or all members.".to_string(),
            parameters: json!({
                "type": "object",
                "properties": {
                    "content": { "type": "string", "description": "Message text" }
                },
                "required": ["content"]
            }),
        },
        ToolDefinition {
            name: "wecom_send_markdown".to_string(),
            description: "Send a Markdown message to WeCom. Supports **bold**, # headers, [links](url), > quotes.".to_string(),
            parameters: json!({
                "type": "object",
                "properties": {
                    "content": {
                        "type": "string",
                        "description": "Markdown text, supports **bold**, # headers, [links](url), > quotes"
                    }
                },
                "required": ["content"]
            }),
        },
        ToolDefinition {
            name: "wecom_send_file".to_string(),
            description: "Send a text message describing a file. Note: direct file upload requires media_id pre-upload.".to_string(),
            parameters: json!({
                "type": "object",
                "properties": {
                    "content": {
                        "type": "string",
                        "description": "Text message describing the file (direct file upload requires media_id pre-upload)"
                    }
                },
                "required": ["content"]
            }),
        },
        ToolDefinition {
            name: "wecom_get_members".to_string(),
            description: "Get the simplified member list for a WeCom department (App mode only).".to_string(),
            parameters: json!({
                "type": "object",
                "properties": {
                    "department_id": {
                        "type": "string",
                        "description": "Department ID, default 1 for root"
                    }
                },
                "required": []
            }),
        },
    ]
}
