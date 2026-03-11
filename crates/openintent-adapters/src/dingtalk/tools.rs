//! DingTalk tool implementations.

use std::time::{SystemTime, UNIX_EPOCH};

use base64::Engine;
use hmac::{Hmac, Mac};
use serde_json::{Value, json};
use sha2::Sha256;
use tracing::info;

use crate::error::{AdapterError, Result};

use super::api::check_dingtalk_error;
use super::types::{
    ActionButton, ActionCardContent, AtConfig, MarkdownContent, TextContent, WebhookActionCardMsg,
    WebhookMarkdownMsg, WebhookTextMsg,
};

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

/// Build a signed webhook URL using HMAC-SHA256.
fn sign_webhook_url(webhook_url: &str, secret: &str) -> String {
    let timestamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as u64;

    let sign_string = format!("{timestamp}\n{secret}");
    type HmacSha256 = Hmac<Sha256>;
    let mut mac = HmacSha256::new_from_slice(secret.as_bytes()).expect("HMAC key error");
    mac.update(sign_string.as_bytes());
    let signature = base64::engine::general_purpose::STANDARD.encode(mac.finalize().into_bytes());
    let encoded_sig = urlencoding::encode(&signature);

    format!("{webhook_url}&timestamp={timestamp}&sign={encoded_sig}")
}

/// Dispatch a webhook send based on tool_name.
pub async fn tool_webhook_send(
    client: &reqwest::Client,
    webhook_url: &str,
    secret: Option<&str>,
    tool_name: &str,
    params: &Value,
) -> Result<Value> {
    let url = if let Some(s) = secret {
        sign_webhook_url(webhook_url, s)
    } else {
        webhook_url.to_string()
    };

    let empty_vec: Vec<String> = vec![];
    let body = match tool_name {
        "dingtalk_send_text" => {
            let content = required_str!(params, "content", tool_name);
            let at_all = params.get("at_all").and_then(|v| v.as_bool()).unwrap_or(false);
            let mobiles: Vec<String> = params
                .get("at_mobiles")
                .and_then(|v| v.as_array())
                .map(|arr| arr.iter().filter_map(|v| v.as_str().map(str::to_string)).collect())
                .unwrap_or_default();

            let msg = WebhookTextMsg {
                msgtype: "text",
                text: TextContent { content },
                at: AtConfig {
                    at_mobiles: if mobiles.is_empty() { &empty_vec } else { &mobiles },
                    is_at_all: at_all,
                },
            };
            serde_json::to_value(&msg).unwrap_or_default()
        }
        "dingtalk_send_markdown" => {
            let title = required_str!(params, "title", tool_name);
            let text = required_str!(params, "text", tool_name);
            let at_all = params.get("at_all").and_then(|v| v.as_bool()).unwrap_or(false);

            let msg = WebhookMarkdownMsg {
                msgtype: "markdown",
                markdown: MarkdownContent { title, text },
                at: AtConfig { at_mobiles: &empty_vec, is_at_all: at_all },
            };
            serde_json::to_value(&msg).unwrap_or_default()
        }
        "dingtalk_send_card" => {
            let title = required_str!(params, "title", tool_name);
            let text = required_str!(params, "text", tool_name);
            let buttons: Vec<ActionButton> = params
                .get("buttons")
                .and_then(|v| v.as_array())
                .map(|arr| {
                    arr.iter()
                        .filter_map(|b| {
                            Some(ActionButton {
                                title: b.get("title")?.as_str()?.to_string(),
                                action_url: b.get("action_url")?.as_str()?.to_string(),
                            })
                        })
                        .collect()
                })
                .unwrap_or_default();

            let msg = WebhookActionCardMsg {
                msgtype: "actionCard",
                action_card: ActionCardContent {
                    title,
                    text,
                    btn_orientation: "0",
                    btns: buttons,
                },
            };
            serde_json::to_value(&msg).unwrap_or_default()
        }
        _ => {
            return Err(AdapterError::ToolNotFound {
                tool_name: tool_name.to_string(),
                adapter_id: "dingtalk".to_string(),
            });
        }
    };

    let resp: Value = client
        .post(&url)
        .json(&body)
        .send()
        .await
        .map_err(|e| AdapterError::ExecutionFailed {
            tool_name: tool_name.to_string(),
            reason: e.to_string(),
        })?
        .json()
        .await
        .map_err(|e| AdapterError::ExecutionFailed {
            tool_name: tool_name.to_string(),
            reason: format!("JSON parse: {e}"),
        })?;

    check_dingtalk_error(&resp, tool_name)?;
    info!(tool = tool_name, "DingTalk message sent");
    Ok(json!({ "success": true }))
}

/// Send text (App mode — returns content for workflow use).
pub async fn tool_send_text(
    _client: &reqwest::Client,
    _api_base: &str,
    _token: &str,
    params: &Value,
) -> Result<Value> {
    let content = required_str!(params, "content", "dingtalk_send_text");
    Ok(json!({
        "success": true,
        "note": "App mode: deliver via robot/sendByOpenConversationId with open_conversation_id",
        "content": content
    }))
}

/// Send markdown (App mode stub).
pub async fn tool_send_markdown(
    _client: &reqwest::Client,
    _api_base: &str,
    _token: &str,
    params: &Value,
) -> Result<Value> {
    let title = params.get("title").and_then(|v| v.as_str()).unwrap_or("");
    let text = params.get("text").and_then(|v| v.as_str()).unwrap_or("");
    Ok(json!({ "success": true, "title": title, "text": text }))
}

/// Send card (App mode stub).
pub async fn tool_send_card(
    _client: &reqwest::Client,
    _api_base: &str,
    _token: &str,
    params: &Value,
) -> Result<Value> {
    Ok(json!({ "success": true, "params": params }))
}
