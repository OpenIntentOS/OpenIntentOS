//! Telegram Bot API call functions.

use serde_json::{Value, json};
use tracing::debug;

use crate::error::{AdapterError, Result};

/// Parse a Telegram Bot API response, checking the `ok` field for errors.
pub fn parse_telegram_response(response: &Value, tool_name: &str) -> Result<()> {
    let ok = response
        .get("ok")
        .and_then(|v| v.as_bool())
        .unwrap_or(false);

    if !ok {
        let error_code = response
            .get("error_code")
            .and_then(|v| v.as_i64())
            .unwrap_or(-1);
        let description = response
            .get("description")
            .and_then(|v| v.as_str())
            .unwrap_or("unknown error");
        return Err(AdapterError::ExecutionFailed {
            tool_name: tool_name.to_string(),
            reason: format!("Telegram API error (code {error_code}): {description}"),
        });
    }

    Ok(())
}

/// Send a message to a Telegram chat.
pub async fn api_send_message(
    http: &reqwest::Client,
    url: &str,
    params: Value,
) -> Result<Value> {
    let chat_id = params
        .get("chat_id")
        .and_then(|v| v.as_str())
        .ok_or_else(|| AdapterError::InvalidParams {
            tool_name: "telegram_send_message".into(),
            reason: "missing required string field `chat_id`".into(),
        })?;

    let text = params.get("text").and_then(|v| v.as_str()).ok_or_else(|| {
        AdapterError::InvalidParams {
            tool_name: "telegram_send_message".into(),
            reason: "missing required string field `text`".into(),
        }
    })?;

    let parse_mode = params.get("parse_mode").and_then(|v| v.as_str());

    let mut body = json!({ "chat_id": chat_id, "text": text });
    if let Some(mode) = parse_mode {
        body["parse_mode"] = json!(mode);
    }

    debug!(url = %url, chat_id = %chat_id, "sending Telegram message");

    let response = http.post(url).json(&body).send().await.map_err(|e| {
        AdapterError::ExecutionFailed {
            tool_name: "telegram_send_message".into(),
            reason: format!("failed to send message: {e}"),
        }
    })?;

    let json_resp: Value = response.json().await.map_err(|e| AdapterError::ExecutionFailed {
        tool_name: "telegram_send_message".into(),
        reason: format!("failed to parse response: {e}"),
    })?;

    parse_telegram_response(&json_resp, "telegram_send_message")?;

    Ok(json!({ "success": true, "data": json_resp.get("result").cloned().unwrap_or(json!({})) }))
}

/// Send a photo to a Telegram chat.
pub async fn api_send_photo(
    http: &reqwest::Client,
    url: &str,
    params: Value,
) -> Result<Value> {
    let chat_id = params
        .get("chat_id")
        .and_then(|v| v.as_str())
        .ok_or_else(|| AdapterError::InvalidParams {
            tool_name: "telegram_send_photo".into(),
            reason: "missing required string field `chat_id`".into(),
        })?;

    let photo_url = params
        .get("photo_url")
        .and_then(|v| v.as_str())
        .ok_or_else(|| AdapterError::InvalidParams {
            tool_name: "telegram_send_photo".into(),
            reason: "missing required string field `photo_url`".into(),
        })?;

    let caption = params.get("caption").and_then(|v| v.as_str());

    let mut body = json!({ "chat_id": chat_id, "photo": photo_url });
    if let Some(cap) = caption {
        body["caption"] = json!(cap);
    }

    debug!(url = %url, chat_id = %chat_id, "sending Telegram photo");

    let response = http.post(url).json(&body).send().await.map_err(|e| {
        AdapterError::ExecutionFailed {
            tool_name: "telegram_send_photo".into(),
            reason: format!("failed to send photo: {e}"),
        }
    })?;

    let json_resp: Value = response.json().await.map_err(|e| AdapterError::ExecutionFailed {
        tool_name: "telegram_send_photo".into(),
        reason: format!("failed to parse response: {e}"),
    })?;

    parse_telegram_response(&json_resp, "telegram_send_photo")?;

    Ok(json!({ "success": true, "data": json_resp.get("result").cloned().unwrap_or(json!({})) }))
}

/// Send a local file as a document to a Telegram chat.
pub async fn api_send_document(
    http: &reqwest::Client,
    url: &str,
    params: Value,
) -> Result<Value> {
    let chat_id = params
        .get("chat_id")
        .and_then(|v| v.as_str())
        .ok_or_else(|| AdapterError::InvalidParams {
            tool_name: "telegram_send_document".into(),
            reason: "missing required string field `chat_id`".into(),
        })?
        .to_string();

    let file_path = params
        .get("file_path")
        .and_then(|v| v.as_str())
        .ok_or_else(|| AdapterError::InvalidParams {
            tool_name: "telegram_send_document".into(),
            reason: "missing required string field `file_path`".into(),
        })?;

    let caption = params.get("caption").and_then(|v| v.as_str()).map(String::from);

    let file_bytes = tokio::fs::read(file_path).await.map_err(|e| {
        AdapterError::ExecutionFailed {
            tool_name: "telegram_send_document".into(),
            reason: format!("failed to read file `{file_path}`: {e}"),
        }
    })?;

    let file_name = std::path::Path::new(file_path)
        .file_name()
        .map(|n| n.to_string_lossy().to_string())
        .unwrap_or_else(|| "file".to_string());

    debug!(url = %url, chat_id = %chat_id, file_path = %file_path, "sending Telegram document");

    let part = reqwest::multipart::Part::bytes(file_bytes).file_name(file_name);
    let mut form = reqwest::multipart::Form::new()
        .text("chat_id", chat_id)
        .part("document", part);
    if let Some(cap) = caption {
        form = form.text("caption", cap);
    }

    let response = http.post(url).multipart(form).send().await.map_err(|e| {
        AdapterError::ExecutionFailed {
            tool_name: "telegram_send_document".into(),
            reason: format!("failed to send document: {e}"),
        }
    })?;

    let json_resp: Value = response.json().await.map_err(|e| AdapterError::ExecutionFailed {
        tool_name: "telegram_send_document".into(),
        reason: format!("failed to parse response: {e}"),
    })?;

    parse_telegram_response(&json_resp, "telegram_send_document")?;

    Ok(json!({ "success": true, "data": json_resp.get("result").cloned().unwrap_or(json!({})) }))
}

/// Send a local video file to a Telegram chat.
pub async fn api_send_video(
    http: &reqwest::Client,
    url: &str,
    params: Value,
) -> Result<Value> {
    let chat_id = params
        .get("chat_id")
        .and_then(|v| v.as_str())
        .ok_or_else(|| AdapterError::InvalidParams {
            tool_name: "telegram_send_video".into(),
            reason: "missing required string field `chat_id`".into(),
        })?
        .to_string();

    let file_path = params
        .get("file_path")
        .and_then(|v| v.as_str())
        .ok_or_else(|| AdapterError::InvalidParams {
            tool_name: "telegram_send_video".into(),
            reason: "missing required string field `file_path`".into(),
        })?;

    let caption = params.get("caption").and_then(|v| v.as_str()).map(String::from);

    let file_bytes = tokio::fs::read(file_path).await.map_err(|e| {
        AdapterError::ExecutionFailed {
            tool_name: "telegram_send_video".into(),
            reason: format!("failed to read file `{file_path}`: {e}"),
        }
    })?;

    let file_name = std::path::Path::new(file_path)
        .file_name()
        .map(|n| n.to_string_lossy().to_string())
        .unwrap_or_else(|| "video.mp4".to_string());

    debug!(url = %url, chat_id = %chat_id, file_path = %file_path, "sending Telegram video");

    let part = reqwest::multipart::Part::bytes(file_bytes)
        .file_name(file_name)
        .mime_str("video/mp4")
        .map_err(|e| AdapterError::ExecutionFailed {
            tool_name: "telegram_send_video".into(),
            reason: format!("invalid mime type: {e}"),
        })?;

    let mut form = reqwest::multipart::Form::new()
        .text("chat_id", chat_id)
        .part("video", part);
    if let Some(cap) = caption {
        form = form.text("caption", cap);
    }

    let response = http.post(url).multipart(form).send().await.map_err(|e| {
        AdapterError::ExecutionFailed {
            tool_name: "telegram_send_video".into(),
            reason: format!("failed to send video: {e}"),
        }
    })?;

    let json_resp: Value = response.json().await.map_err(|e| AdapterError::ExecutionFailed {
        tool_name: "telegram_send_video".into(),
        reason: format!("failed to parse response: {e}"),
    })?;

    parse_telegram_response(&json_resp, "telegram_send_video")?;

    Ok(json!({ "success": true, "data": json_resp.get("result").cloned().unwrap_or(json!({})) }))
}

/// Get recent updates from the bot.
pub async fn api_get_updates(
    http: &reqwest::Client,
    url: &str,
    params: Value,
) -> Result<Value> {
    let limit = params.get("limit").and_then(|v| v.as_u64());
    let offset = params.get("offset").and_then(|v| v.as_i64());

    let mut body = json!({});
    if let Some(l) = limit {
        body["limit"] = json!(l);
    }
    if let Some(o) = offset {
        body["offset"] = json!(o);
    }

    debug!(url = %url, "getting Telegram updates");

    let response = http.post(url).json(&body).send().await.map_err(|e| {
        AdapterError::ExecutionFailed {
            tool_name: "telegram_get_updates".into(),
            reason: format!("failed to get updates: {e}"),
        }
    })?;

    let json_resp: Value = response.json().await.map_err(|e| AdapterError::ExecutionFailed {
        tool_name: "telegram_get_updates".into(),
        reason: format!("failed to parse response: {e}"),
    })?;

    parse_telegram_response(&json_resp, "telegram_get_updates")?;

    Ok(json!({ "success": true, "data": json_resp.get("result").cloned().unwrap_or(json!([])) }))
}

/// Get information about a chat.
pub async fn api_get_chat(
    http: &reqwest::Client,
    url: &str,
    params: Value,
) -> Result<Value> {
    let chat_id = params
        .get("chat_id")
        .and_then(|v| v.as_str())
        .ok_or_else(|| AdapterError::InvalidParams {
            tool_name: "telegram_get_chat".into(),
            reason: "missing required string field `chat_id`".into(),
        })?;

    let body = json!({ "chat_id": chat_id });

    debug!(url = %url, chat_id = %chat_id, "getting Telegram chat info");

    let response = http.post(url).json(&body).send().await.map_err(|e| {
        AdapterError::ExecutionFailed {
            tool_name: "telegram_get_chat".into(),
            reason: format!("failed to get chat info: {e}"),
        }
    })?;

    let json_resp: Value = response.json().await.map_err(|e| AdapterError::ExecutionFailed {
        tool_name: "telegram_get_chat".into(),
        reason: format!("failed to parse response: {e}"),
    })?;

    parse_telegram_response(&json_resp, "telegram_get_chat")?;

    Ok(json!({ "success": true, "data": json_resp.get("result").cloned().unwrap_or(json!({})) }))
}

/// Set a webhook URL for receiving updates.
pub async fn api_set_webhook(
    http: &reqwest::Client,
    url: &str,
    params: Value,
) -> Result<Value> {
    let webhook_url = params.get("url").and_then(|v| v.as_str()).ok_or_else(|| {
        AdapterError::InvalidParams {
            tool_name: "telegram_set_webhook".into(),
            reason: "missing required string field `url`".into(),
        }
    })?;

    let body = json!({ "url": webhook_url });

    debug!(url = %url, webhook_url = %webhook_url, "setting Telegram webhook");

    let response = http.post(url).json(&body).send().await.map_err(|e| {
        AdapterError::ExecutionFailed {
            tool_name: "telegram_set_webhook".into(),
            reason: format!("failed to set webhook: {e}"),
        }
    })?;

    let json_resp: Value = response.json().await.map_err(|e| AdapterError::ExecutionFailed {
        tool_name: "telegram_set_webhook".into(),
        reason: format!("failed to parse response: {e}"),
    })?;

    parse_telegram_response(&json_resp, "telegram_set_webhook")?;

    Ok(json!({ "success": true, "data": json_resp.get("result").cloned().unwrap_or(json!(true)) }))
}

/// Get detailed chat member information.
pub async fn api_get_chat_member(
    http: &reqwest::Client,
    url: &str,
    params: Value,
) -> Result<Value> {
    let chat_id = params
        .get("chat_id")
        .and_then(|v| v.as_str())
        .ok_or_else(|| AdapterError::InvalidParams {
            tool_name: "telegram_get_chat_member".into(),
            reason: "missing required string field `chat_id`".into(),
        })?;

    let user_id = params
        .get("user_id")
        .and_then(|v| v.as_i64())
        .ok_or_else(|| AdapterError::InvalidParams {
            tool_name: "telegram_get_chat_member".into(),
            reason: "missing required integer field `user_id`".into(),
        })?;

    let body = json!({ "chat_id": chat_id, "user_id": user_id });

    debug!(url = %url, chat_id = %chat_id, user_id = user_id, "getting Telegram chat member info");

    let response = http.post(url).json(&body).send().await.map_err(|e| {
        AdapterError::ExecutionFailed {
            tool_name: "telegram_get_chat_member".into(),
            reason: format!("failed to get chat member info: {e}"),
        }
    })?;

    let json_resp: Value = response.json().await.map_err(|e| AdapterError::ExecutionFailed {
        tool_name: "telegram_get_chat_member".into(),
        reason: format!("failed to parse response: {e}"),
    })?;

    parse_telegram_response(&json_resp, "telegram_get_chat_member")?;

    Ok(json!({ "success": true, "data": json_resp.get("result").cloned().unwrap_or(json!({})) }))
}
