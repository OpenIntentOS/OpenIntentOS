//! Feishu Open Platform HTTP API helpers.
//!
//! Provides authentication token management, response parsing,
//! and tool-level API call functions for the FeishuAdapter.

use serde_json::{Value, json};
use tracing::debug;

use crate::error::{AdapterError, Result};

// ---------------------------------------------------------------------------
// Response parsing
// ---------------------------------------------------------------------------

/// Parse a Feishu API response, checking the `code` field for errors.
///
/// Feishu responses follow the format:
/// `{ "code": 0, "msg": "success", "data": {...} }`
pub fn parse_feishu_response(response: &Value, tool_name: &str) -> Result<()> {
    let code = response.get("code").and_then(|v| v.as_i64()).unwrap_or(-1);

    if code != 0 {
        let msg = response
            .get("msg")
            .and_then(|v| v.as_str())
            .unwrap_or("unknown error");
        return Err(AdapterError::ExecutionFailed {
            tool_name: tool_name.to_string(),
            reason: format!("Feishu API error (code {code}): {msg}"),
        });
    }

    Ok(())
}

// ---------------------------------------------------------------------------
// Token management
// ---------------------------------------------------------------------------

/// Build the JSON request body for obtaining a tenant access token.
pub fn build_token_request_body(app_id: &str, app_secret: &str) -> Value {
    json!({
        "app_id": app_id,
        "app_secret": app_secret
    })
}

/// Request a new tenant access token from the Feishu API.
pub async fn fetch_tenant_access_token(
    client: &reqwest::Client,
    base_url: &str,
    adapter_id: &str,
    app_id: &str,
    app_secret: &str,
) -> Result<String> {
    let url = format!("{}/auth/v3/tenant_access_token/internal", base_url);
    let body = build_token_request_body(app_id, app_secret);

    debug!(url = %url, "requesting tenant access token");

    let response = client
        .post(&url)
        .header("Content-Type", "application/json; charset=utf-8")
        .json(&body)
        .send()
        .await
        .map_err(|e| AdapterError::ExecutionFailed {
            tool_name: "auth".into(),
            reason: format!("failed to request tenant access token: {e}"),
        })?;

    let json_resp: Value =
        response
            .json()
            .await
            .map_err(|e| AdapterError::ExecutionFailed {
                tool_name: "auth".into(),
                reason: format!("failed to parse token response: {e}"),
            })?;

    parse_feishu_response(&json_resp, "auth")?;

    json_resp
        .get("tenant_access_token")
        .and_then(|v| v.as_str())
        .map(|s| s.to_string())
        .ok_or_else(|| AdapterError::ExecutionFailed {
            tool_name: "auth".into(),
            reason: "tenant_access_token not found in response".into(),
        })
}

// ---------------------------------------------------------------------------
// Message helpers
// ---------------------------------------------------------------------------

/// Build the message body for the send message API.
pub fn build_message_body(receive_id: &str, msg_type: &str, content: &str) -> Value {
    json!({
        "receive_id": receive_id,
        "msg_type": msg_type,
        "content": content
    })
}

/// Build the URL for sending a message with the receive_id_type query param.
pub fn build_send_message_url(base_url: &str, receive_id_type: &str) -> String {
    format!(
        "{}/im/v1/messages?receive_id_type={}",
        base_url, receive_id_type
    )
}

// ---------------------------------------------------------------------------
// HTTP helpers (auth headers)
// ---------------------------------------------------------------------------

fn get_request(client: &reqwest::Client, url: &str, token: &str) -> reqwest::RequestBuilder {
    client
        .get(url)
        .header("Authorization", format!("Bearer {token}"))
        .header("Content-Type", "application/json; charset=utf-8")
}

fn post_request(client: &reqwest::Client, url: &str, token: &str) -> reqwest::RequestBuilder {
    client
        .post(url)
        .header("Authorization", format!("Bearer {token}"))
        .header("Content-Type", "application/json; charset=utf-8")
}

// ---------------------------------------------------------------------------
// Tool implementations
// ---------------------------------------------------------------------------

/// Send a message to a user or group chat.
pub async fn tool_send_message(
    client: &reqwest::Client,
    base_url: &str,
    token: &str,
    params: &Value,
) -> Result<Value> {
    let receive_id = params
        .get("receive_id")
        .and_then(|v| v.as_str())
        .ok_or_else(|| AdapterError::InvalidParams {
            tool_name: "feishu_send_message".into(),
            reason: "missing required string field `receive_id`".into(),
        })?;

    let receive_id_type = params
        .get("receive_id_type")
        .and_then(|v| v.as_str())
        .ok_or_else(|| AdapterError::InvalidParams {
            tool_name: "feishu_send_message".into(),
            reason: "missing required string field `receive_id_type`".into(),
        })?;

    let msg_type = params
        .get("msg_type")
        .and_then(|v| v.as_str())
        .ok_or_else(|| AdapterError::InvalidParams {
            tool_name: "feishu_send_message".into(),
            reason: "missing required string field `msg_type`".into(),
        })?;

    let content = params
        .get("content")
        .and_then(|v| v.as_str())
        .ok_or_else(|| AdapterError::InvalidParams {
            tool_name: "feishu_send_message".into(),
            reason: "missing required string field `content`".into(),
        })?;

    let url = build_send_message_url(base_url, receive_id_type);
    let body = build_message_body(receive_id, msg_type, content);

    debug!(url = %url, receive_id = %receive_id, msg_type = %msg_type, "sending Feishu message");

    let response = post_request(client, &url, token)
        .json(&body)
        .send()
        .await
        .map_err(|e| AdapterError::ExecutionFailed {
            tool_name: "feishu_send_message".into(),
            reason: format!("failed to send message: {e}"),
        })?;

    let json_resp: Value =
        response
            .json()
            .await
            .map_err(|e| AdapterError::ExecutionFailed {
                tool_name: "feishu_send_message".into(),
                reason: format!("failed to parse response: {e}"),
            })?;

    parse_feishu_response(&json_resp, "feishu_send_message")?;

    Ok(json!({
        "success": true,
        "data": json_resp.get("data").cloned().unwrap_or(json!({})),
    }))
}

/// List available group chats.
pub async fn tool_list_chats(
    client: &reqwest::Client,
    base_url: &str,
    token: &str,
    params: &Value,
) -> Result<Value> {
    let page_size = params
        .get("page_size")
        .and_then(|v| v.as_u64())
        .unwrap_or(20);
    let page_token = params
        .get("page_token")
        .and_then(|v| v.as_str())
        .unwrap_or("");

    let mut url = format!("{}/im/v1/chats?page_size={}", base_url, page_size);
    if !page_token.is_empty() {
        url.push_str(&format!("&page_token={}", page_token));
    }

    debug!(url = %url, "listing Feishu chats");

    let response = get_request(client, &url, token)
        .send()
        .await
        .map_err(|e| AdapterError::ExecutionFailed {
            tool_name: "feishu_list_chats".into(),
            reason: format!("failed to list chats: {e}"),
        })?;

    let json_resp: Value =
        response
            .json()
            .await
            .map_err(|e| AdapterError::ExecutionFailed {
                tool_name: "feishu_list_chats".into(),
                reason: format!("failed to parse response: {e}"),
            })?;

    parse_feishu_response(&json_resp, "feishu_list_chats")?;

    Ok(json!({
        "success": true,
        "data": json_resp.get("data").cloned().unwrap_or(json!({})),
    }))
}

/// Get recent messages from a chat.
pub async fn tool_get_chat_messages(
    client: &reqwest::Client,
    base_url: &str,
    token: &str,
    params: &Value,
) -> Result<Value> {
    let container_id = params
        .get("container_id")
        .and_then(|v| v.as_str())
        .ok_or_else(|| AdapterError::InvalidParams {
            tool_name: "feishu_get_chat_messages".into(),
            reason: "missing required string field `container_id`".into(),
        })?;

    let page_size = params
        .get("page_size")
        .and_then(|v| v.as_u64())
        .unwrap_or(20);

    let url = format!(
        "{}/im/v1/messages?container_id_type=chat&container_id={}&page_size={}",
        base_url, container_id, page_size
    );

    debug!(url = %url, container_id = %container_id, "getting Feishu chat messages");

    let response = get_request(client, &url, token)
        .send()
        .await
        .map_err(|e| AdapterError::ExecutionFailed {
            tool_name: "feishu_get_chat_messages".into(),
            reason: format!("failed to get chat messages: {e}"),
        })?;

    let json_resp: Value =
        response
            .json()
            .await
            .map_err(|e| AdapterError::ExecutionFailed {
                tool_name: "feishu_get_chat_messages".into(),
                reason: format!("failed to parse response: {e}"),
            })?;

    parse_feishu_response(&json_resp, "feishu_get_chat_messages")?;

    Ok(json!({
        "success": true,
        "data": json_resp.get("data").cloned().unwrap_or(json!({})),
    }))
}

/// Create a document in Feishu Docs.
pub async fn tool_create_doc(
    client: &reqwest::Client,
    base_url: &str,
    token: &str,
    params: &Value,
) -> Result<Value> {
    let title = params
        .get("title")
        .and_then(|v| v.as_str())
        .ok_or_else(|| AdapterError::InvalidParams {
            tool_name: "feishu_create_doc".into(),
            reason: "missing required string field `title`".into(),
        })?;

    let folder_token = params.get("folder_token").and_then(|v| v.as_str());

    let url = format!("{}/docx/v1/documents", base_url);

    let mut body = json!({ "title": title });
    if let Some(ft) = folder_token {
        body["folder_token"] = json!(ft);
    }

    debug!(url = %url, title = %title, "creating Feishu document");

    let response = post_request(client, &url, token)
        .json(&body)
        .send()
        .await
        .map_err(|e| AdapterError::ExecutionFailed {
            tool_name: "feishu_create_doc".into(),
            reason: format!("failed to create document: {e}"),
        })?;

    let json_resp: Value =
        response
            .json()
            .await
            .map_err(|e| AdapterError::ExecutionFailed {
                tool_name: "feishu_create_doc".into(),
                reason: format!("failed to parse response: {e}"),
            })?;

    parse_feishu_response(&json_resp, "feishu_create_doc")?;

    Ok(json!({
        "success": true,
        "data": json_resp.get("data").cloned().unwrap_or(json!({})),
    }))
}

/// Search for users by name or email.
pub async fn tool_search_users(
    client: &reqwest::Client,
    base_url: &str,
    token: &str,
    params: &Value,
) -> Result<Value> {
    let query = params
        .get("query")
        .and_then(|v| v.as_str())
        .ok_or_else(|| AdapterError::InvalidParams {
            tool_name: "feishu_search_users".into(),
            reason: "missing required string field `query`".into(),
        })?;

    let page_size = params
        .get("page_size")
        .and_then(|v| v.as_u64())
        .unwrap_or(20);

    let url = format!("{}/search/v1/user", base_url);

    let body = json!({
        "query": query,
        "page_size": page_size
    });

    debug!(url = %url, query = %query, "searching Feishu users");

    let response = post_request(client, &url, token)
        .json(&body)
        .send()
        .await
        .map_err(|e| AdapterError::ExecutionFailed {
            tool_name: "feishu_search_users".into(),
            reason: format!("failed to search users: {e}"),
        })?;

    let json_resp: Value =
        response
            .json()
            .await
            .map_err(|e| AdapterError::ExecutionFailed {
                tool_name: "feishu_search_users".into(),
                reason: format!("failed to parse response: {e}"),
            })?;

    parse_feishu_response(&json_resp, "feishu_search_users")?;

    Ok(json!({
        "success": true,
        "data": json_resp.get("data").cloned().unwrap_or(json!({})),
    }))
}

/// Get user details by user ID.
pub async fn tool_get_user_info(
    client: &reqwest::Client,
    base_url: &str,
    token: &str,
    params: &Value,
) -> Result<Value> {
    let user_id = params
        .get("user_id")
        .and_then(|v| v.as_str())
        .ok_or_else(|| AdapterError::InvalidParams {
            tool_name: "feishu_get_user_info".into(),
            reason: "missing required string field `user_id`".into(),
        })?;

    let user_id_type = params
        .get("user_id_type")
        .and_then(|v| v.as_str())
        .unwrap_or("open_id");

    let url = format!(
        "{}/contact/v3/users/{}?user_id_type={}",
        base_url, user_id, user_id_type
    );

    debug!(url = %url, user_id = %user_id, "getting Feishu user info");

    let response = get_request(client, &url, token)
        .send()
        .await
        .map_err(|e| AdapterError::ExecutionFailed {
            tool_name: "feishu_get_user_info".into(),
            reason: format!("failed to get user info: {e}"),
        })?;

    let json_resp: Value =
        response
            .json()
            .await
            .map_err(|e| AdapterError::ExecutionFailed {
                tool_name: "feishu_get_user_info".into(),
                reason: format!("failed to parse response: {e}"),
            })?;

    parse_feishu_response(&json_resp, "feishu_get_user_info")?;

    Ok(json!({
        "success": true,
        "data": json_resp.get("data").cloned().unwrap_or(json!({})),
    }))
}
