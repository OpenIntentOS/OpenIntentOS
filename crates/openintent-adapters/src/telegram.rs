//! Telegram Bot API adapter for OpenIntentOS.
//!
//! Provides tools for interacting with the Telegram Bot API, enabling the AI
//! agent to send and receive messages, photos, and manage webhooks via
//! Telegram bots.  Supports five tools:
//!
//! - `telegram_send_message` — Send a text message to a chat
//! - `telegram_send_photo` — Send a photo to a chat
//! - `telegram_get_updates` — Poll for recent messages/updates
//! - `telegram_get_chat` — Get information about a chat
//! - `telegram_set_webhook` — Set a webhook URL for push-based updates

use async_trait::async_trait;
use serde_json::{Value, json};
use tracing::{debug, info, warn};

use crate::error::{AdapterError, Result};
use crate::traits::{Adapter, AdapterType, AuthRequirement, HealthStatus, ToolDefinition};

/// Telegram Bot API base URL.  All method calls are POSTed to
/// `{BASE_URL}{bot_token}/{method}`.
const TELEGRAM_API_BASE: &str = "https://api.telegram.org/bot";

/// Telegram Bot API adapter.
///
/// Provides tools for sending messages, photos, polling updates, querying
/// chat metadata, and configuring webhooks.  Authentication is performed via
/// a bot token obtained from [@BotFather](https://t.me/BotFather).
pub struct TelegramAdapter {
    /// Unique identifier for this adapter instance.
    id: String,
    /// Whether the adapter has been connected.
    connected: bool,
    /// Telegram bot token used to authenticate API requests.
    bot_token: Option<String>,
    /// HTTP client for making requests.
    http: reqwest::Client,
}

impl TelegramAdapter {
    /// Create a new Telegram adapter with default configuration and no token.
    pub fn new(id: impl Into<String>) -> Self {
        let http = reqwest::Client::builder()
            .user_agent("OpenIntentOS/0.1")
            .build()
            .unwrap_or_default();

        Self {
            id: id.into(),
            connected: false,
            bot_token: None,
            http,
        }
    }

    /// Create a new Telegram adapter with a pre-configured bot token.
    pub fn with_token(id: impl Into<String>, token: impl Into<String>) -> Self {
        let mut adapter = Self::new(id);
        adapter.bot_token = Some(token.into());
        adapter
    }

    // -----------------------------------------------------------------------
    // URL construction
    // -----------------------------------------------------------------------

    /// Build a full Telegram Bot API URL for the given method.
    fn api_url(&self, method: &str) -> Result<String> {
        let token = self.resolve_token()?;
        Ok(format!("{}{}/{}", TELEGRAM_API_BASE, token, method))
    }

    // -----------------------------------------------------------------------
    // Token management
    // -----------------------------------------------------------------------

    /// Resolve the bot token, returning an error if none is available.
    fn resolve_token(&self) -> Result<String> {
        self.bot_token
            .clone()
            .ok_or_else(|| AdapterError::AuthRequired {
                adapter_id: self.id.clone(),
                provider: "telegram".to_string(),
            })
    }

    // -----------------------------------------------------------------------
    // Response parsing
    // -----------------------------------------------------------------------

    /// Parse a Telegram Bot API response, checking the `ok` field for errors.
    ///
    /// Telegram responses follow the format:
    /// `{ "ok": true, "result": {...} }` on success, or
    /// `{ "ok": false, "error_code": 400, "description": "..." }` on failure.
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

    // -----------------------------------------------------------------------
    // Tool implementations
    // -----------------------------------------------------------------------

    /// Send a text message to a chat.
    async fn tool_send_message(&self, params: Value) -> Result<Value> {
        let url = self.api_url("sendMessage")?;

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

        let mut body = json!({
            "chat_id": chat_id,
            "text": text,
        });
        if let Some(mode) = parse_mode {
            body["parse_mode"] = json!(mode);
        }

        debug!(url = %url, chat_id = %chat_id, "sending Telegram message");

        let response = self.http.post(&url).json(&body).send().await.map_err(|e| {
            AdapterError::ExecutionFailed {
                tool_name: "telegram_send_message".into(),
                reason: format!("failed to send message: {e}"),
            }
        })?;

        let json_resp: Value =
            response
                .json()
                .await
                .map_err(|e| AdapterError::ExecutionFailed {
                    tool_name: "telegram_send_message".into(),
                    reason: format!("failed to parse response: {e}"),
                })?;

        Self::parse_telegram_response(&json_resp, "telegram_send_message")?;

        Ok(json!({
            "success": true,
            "data": json_resp.get("result").cloned().unwrap_or(json!({})),
        }))
    }

    /// Send a photo to a chat.
    async fn tool_send_photo(&self, params: Value) -> Result<Value> {
        let url = self.api_url("sendPhoto")?;

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

        let mut body = json!({
            "chat_id": chat_id,
            "photo": photo_url,
        });
        if let Some(cap) = caption {
            body["caption"] = json!(cap);
        }

        debug!(url = %url, chat_id = %chat_id, "sending Telegram photo");

        let response = self.http.post(&url).json(&body).send().await.map_err(|e| {
            AdapterError::ExecutionFailed {
                tool_name: "telegram_send_photo".into(),
                reason: format!("failed to send photo: {e}"),
            }
        })?;

        let json_resp: Value =
            response
                .json()
                .await
                .map_err(|e| AdapterError::ExecutionFailed {
                    tool_name: "telegram_send_photo".into(),
                    reason: format!("failed to parse response: {e}"),
                })?;

        Self::parse_telegram_response(&json_resp, "telegram_send_photo")?;

        Ok(json!({
            "success": true,
            "data": json_resp.get("result").cloned().unwrap_or(json!({})),
        }))
    }

    /// Get recent messages/updates from the bot.
    async fn tool_get_updates(&self, params: Value) -> Result<Value> {
        let url = self.api_url("getUpdates")?;

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

        let response = self.http.post(&url).json(&body).send().await.map_err(|e| {
            AdapterError::ExecutionFailed {
                tool_name: "telegram_get_updates".into(),
                reason: format!("failed to get updates: {e}"),
            }
        })?;

        let json_resp: Value =
            response
                .json()
                .await
                .map_err(|e| AdapterError::ExecutionFailed {
                    tool_name: "telegram_get_updates".into(),
                    reason: format!("failed to parse response: {e}"),
                })?;

        Self::parse_telegram_response(&json_resp, "telegram_get_updates")?;

        Ok(json!({
            "success": true,
            "data": json_resp.get("result").cloned().unwrap_or(json!([])),
        }))
    }

    /// Get information about a chat.
    async fn tool_get_chat(&self, params: Value) -> Result<Value> {
        let url = self.api_url("getChat")?;

        let chat_id = params
            .get("chat_id")
            .and_then(|v| v.as_str())
            .ok_or_else(|| AdapterError::InvalidParams {
                tool_name: "telegram_get_chat".into(),
                reason: "missing required string field `chat_id`".into(),
            })?;

        let body = json!({ "chat_id": chat_id });

        debug!(url = %url, chat_id = %chat_id, "getting Telegram chat info");

        let response = self.http.post(&url).json(&body).send().await.map_err(|e| {
            AdapterError::ExecutionFailed {
                tool_name: "telegram_get_chat".into(),
                reason: format!("failed to get chat info: {e}"),
            }
        })?;

        let json_resp: Value =
            response
                .json()
                .await
                .map_err(|e| AdapterError::ExecutionFailed {
                    tool_name: "telegram_get_chat".into(),
                    reason: format!("failed to parse response: {e}"),
                })?;

        Self::parse_telegram_response(&json_resp, "telegram_get_chat")?;

        Ok(json!({
            "success": true,
            "data": json_resp.get("result").cloned().unwrap_or(json!({})),
        }))
    }

    /// Set a webhook URL for receiving updates.
    async fn tool_set_webhook(&self, params: Value) -> Result<Value> {
        let url = self.api_url("setWebhook")?;

        let webhook_url = params.get("url").and_then(|v| v.as_str()).ok_or_else(|| {
            AdapterError::InvalidParams {
                tool_name: "telegram_set_webhook".into(),
                reason: "missing required string field `url`".into(),
            }
        })?;

        let body = json!({ "url": webhook_url });

        debug!(url = %url, webhook_url = %webhook_url, "setting Telegram webhook");

        let response = self.http.post(&url).json(&body).send().await.map_err(|e| {
            AdapterError::ExecutionFailed {
                tool_name: "telegram_set_webhook".into(),
                reason: format!("failed to set webhook: {e}"),
            }
        })?;

        let json_resp: Value =
            response
                .json()
                .await
                .map_err(|e| AdapterError::ExecutionFailed {
                    tool_name: "telegram_set_webhook".into(),
                    reason: format!("failed to parse response: {e}"),
                })?;

        Self::parse_telegram_response(&json_resp, "telegram_set_webhook")?;

        Ok(json!({
            "success": true,
            "data": json_resp.get("result").cloned().unwrap_or(json!(true)),
        }))
    }
}

// ---------------------------------------------------------------------------
// Adapter trait implementation
// ---------------------------------------------------------------------------

#[async_trait]
impl Adapter for TelegramAdapter {
    fn id(&self) -> &str {
        &self.id
    }

    fn adapter_type(&self) -> AdapterType {
        AdapterType::Messaging
    }

    async fn connect(&mut self) -> Result<()> {
        // Attempt to read the bot token from environment if not already set.
        if self.bot_token.is_none() {
            match std::env::var("TELEGRAM_BOT_TOKEN") {
                Ok(token) if !token.is_empty() => {
                    info!(id = %self.id, "Telegram adapter loaded bot token from environment");
                    self.bot_token = Some(token);
                }
                _ => {
                    warn!(
                        id = %self.id,
                        "TELEGRAM_BOT_TOKEN not set; Telegram adapter connecting without auth"
                    );
                }
            }
        } else {
            info!(id = %self.id, "Telegram adapter connecting with pre-configured token");
        }

        self.connected = true;
        Ok(())
    }

    async fn disconnect(&mut self) -> Result<()> {
        info!(id = %self.id, "Telegram adapter disconnected");
        self.bot_token = None;
        self.connected = false;
        Ok(())
    }

    async fn health_check(&self) -> Result<HealthStatus> {
        if !self.connected {
            return Ok(HealthStatus::Unhealthy);
        }
        if self.bot_token.is_some() {
            Ok(HealthStatus::Healthy)
        } else {
            Ok(HealthStatus::Degraded)
        }
    }

    fn tools(&self) -> Vec<ToolDefinition> {
        vec![
            ToolDefinition {
                name: "telegram_send_message".into(),
                description: "Send a text message to a Telegram chat".into(),
                parameters: json!({
                    "type": "object",
                    "properties": {
                        "chat_id": {
                            "type": "string",
                            "description": "Unique identifier for the target chat or username of the target channel (e.g. @channelusername)"
                        },
                        "text": {
                            "type": "string",
                            "description": "Text of the message to send"
                        },
                        "parse_mode": {
                            "type": "string",
                            "description": "Mode for parsing entities in the message text: HTML or Markdown",
                            "enum": ["HTML", "Markdown"]
                        }
                    },
                    "required": ["chat_id", "text"]
                }),
            },
            ToolDefinition {
                name: "telegram_send_photo".into(),
                description: "Send a photo to a Telegram chat".into(),
                parameters: json!({
                    "type": "object",
                    "properties": {
                        "chat_id": {
                            "type": "string",
                            "description": "Unique identifier for the target chat or username of the target channel"
                        },
                        "photo_url": {
                            "type": "string",
                            "description": "URL of the photo to send"
                        },
                        "caption": {
                            "type": "string",
                            "description": "Photo caption, 0-1024 characters"
                        }
                    },
                    "required": ["chat_id", "photo_url"]
                }),
            },
            ToolDefinition {
                name: "telegram_get_updates".into(),
                description:
                    "Get recent incoming updates (messages, callback queries, etc.) for the bot"
                        .into(),
                parameters: json!({
                    "type": "object",
                    "properties": {
                        "limit": {
                            "type": "integer",
                            "description": "Maximum number of updates to retrieve (1-100, default: 100)"
                        },
                        "offset": {
                            "type": "integer",
                            "description": "Identifier of the first update to be returned; use to acknowledge previous updates"
                        }
                    },
                    "required": []
                }),
            },
            ToolDefinition {
                name: "telegram_get_chat".into(),
                description: "Get up-to-date information about a Telegram chat".into(),
                parameters: json!({
                    "type": "object",
                    "properties": {
                        "chat_id": {
                            "type": "string",
                            "description": "Unique identifier for the target chat or username of the target supergroup/channel"
                        }
                    },
                    "required": ["chat_id"]
                }),
            },
            ToolDefinition {
                name: "telegram_set_webhook".into(),
                description: "Set a webhook URL for the bot to receive updates via HTTPS POST"
                    .into(),
                parameters: json!({
                    "type": "object",
                    "properties": {
                        "url": {
                            "type": "string",
                            "description": "HTTPS URL to send updates to; use an empty string to remove the webhook"
                        }
                    },
                    "required": ["url"]
                }),
            },
        ]
    }

    async fn execute_tool(&self, name: &str, params: Value) -> Result<Value> {
        if !self.connected {
            return Err(AdapterError::ExecutionFailed {
                tool_name: name.to_string(),
                reason: format!("adapter `{}` is not connected", self.id),
            });
        }

        match name {
            "telegram_send_message" => self.tool_send_message(params).await,
            "telegram_send_photo" => self.tool_send_photo(params).await,
            "telegram_get_updates" => self.tool_get_updates(params).await,
            "telegram_get_chat" => self.tool_get_chat(params).await,
            "telegram_set_webhook" => self.tool_set_webhook(params).await,
            _ => Err(AdapterError::ToolNotFound {
                adapter_id: self.id.clone(),
                tool_name: name.to_string(),
            }),
        }
    }

    fn required_auth(&self) -> Option<AuthRequirement> {
        Some(AuthRequirement {
            provider: "telegram".into(),
            scopes: vec!["TELEGRAM_BOT_TOKEN".into()],
        })
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    // -- Construction tests --

    #[test]
    fn new_creates_adapter_with_defaults() {
        let adapter = TelegramAdapter::new("tg-test");
        assert_eq!(adapter.id, "tg-test");
        assert!(!adapter.connected);
        assert!(adapter.bot_token.is_none());
    }

    #[test]
    fn with_token_sets_bot_token() {
        let adapter = TelegramAdapter::with_token("tg-test", "123456:ABC-DEF");
        assert_eq!(adapter.id, "tg-test");
        assert_eq!(adapter.bot_token.as_deref(), Some("123456:ABC-DEF"));
        assert!(!adapter.connected);
    }

    // -- Adapter trait basics --

    #[test]
    fn adapter_id_returns_id() {
        let adapter = TelegramAdapter::new("my-telegram");
        assert_eq!(adapter.id(), "my-telegram");
    }

    #[test]
    fn adapter_type_is_messaging() {
        let adapter = TelegramAdapter::new("telegram");
        assert_eq!(adapter.adapter_type(), AdapterType::Messaging);
    }

    #[test]
    fn required_auth_returns_telegram_provider() {
        let adapter = TelegramAdapter::new("telegram");
        let auth = adapter.required_auth().expect("should require auth");
        assert_eq!(auth.provider, "telegram");
        assert!(auth.scopes.contains(&"TELEGRAM_BOT_TOKEN".to_string()));
    }

    // -- Tool definitions --

    #[test]
    fn tools_returns_exactly_five() {
        let adapter = TelegramAdapter::new("telegram");
        let tools = adapter.tools();
        assert_eq!(tools.len(), 5);
    }

    #[test]
    fn tools_have_expected_names() {
        let adapter = TelegramAdapter::new("telegram");
        let names: Vec<String> = adapter.tools().iter().map(|t| t.name.clone()).collect();
        let expected = vec![
            "telegram_send_message",
            "telegram_send_photo",
            "telegram_get_updates",
            "telegram_get_chat",
            "telegram_set_webhook",
        ];
        assert_eq!(names, expected);
    }

    #[test]
    fn tool_send_message_has_required_fields() {
        let adapter = TelegramAdapter::new("telegram");
        let tools = adapter.tools();
        let send_msg = tools
            .iter()
            .find(|t| t.name == "telegram_send_message")
            .expect("should have telegram_send_message");
        let required = send_msg.parameters["required"]
            .as_array()
            .expect("required should be an array");
        assert!(required.contains(&json!("chat_id")));
        assert!(required.contains(&json!("text")));
    }

    #[test]
    fn tool_send_photo_has_required_fields() {
        let adapter = TelegramAdapter::new("telegram");
        let tools = adapter.tools();
        let send_photo = tools
            .iter()
            .find(|t| t.name == "telegram_send_photo")
            .expect("should have telegram_send_photo");
        let required = send_photo.parameters["required"]
            .as_array()
            .expect("required should be an array");
        assert!(required.contains(&json!("chat_id")));
        assert!(required.contains(&json!("photo_url")));
    }

    #[test]
    fn tool_get_updates_has_no_required_fields() {
        let adapter = TelegramAdapter::new("telegram");
        let tools = adapter.tools();
        let get_updates = tools
            .iter()
            .find(|t| t.name == "telegram_get_updates")
            .expect("should have telegram_get_updates");
        let required = get_updates.parameters["required"]
            .as_array()
            .expect("required should be an array");
        assert!(required.is_empty());
    }

    #[test]
    fn tool_get_chat_has_required_fields() {
        let adapter = TelegramAdapter::new("telegram");
        let tools = adapter.tools();
        let get_chat = tools
            .iter()
            .find(|t| t.name == "telegram_get_chat")
            .expect("should have telegram_get_chat");
        let required = get_chat.parameters["required"]
            .as_array()
            .expect("required should be an array");
        assert!(required.contains(&json!("chat_id")));
    }

    #[test]
    fn tool_set_webhook_has_required_fields() {
        let adapter = TelegramAdapter::new("telegram");
        let tools = adapter.tools();
        let set_webhook = tools
            .iter()
            .find(|t| t.name == "telegram_set_webhook")
            .expect("should have telegram_set_webhook");
        let required = set_webhook.parameters["required"]
            .as_array()
            .expect("required should be an array");
        assert!(required.contains(&json!("url")));
    }

    // -- Connect / disconnect --

    #[tokio::test]
    async fn connect_succeeds_without_env_token() {
        let mut adapter = TelegramAdapter::new("telegram");
        let result = adapter.connect().await;
        assert!(result.is_ok());
        assert!(adapter.connected);
    }

    #[tokio::test]
    async fn connect_with_preloaded_token_keeps_token() {
        let mut adapter = TelegramAdapter::with_token("telegram", "my-token");
        adapter.connect().await.unwrap();
        assert!(adapter.connected);
        assert_eq!(adapter.bot_token.as_deref(), Some("my-token"));
    }

    #[tokio::test]
    async fn disconnect_clears_token_and_sets_disconnected() {
        let mut adapter = TelegramAdapter::with_token("telegram", "test-token");
        adapter.connected = true;
        adapter.disconnect().await.unwrap();
        assert!(!adapter.connected);
        assert!(adapter.bot_token.is_none());
    }

    // -- Health check --

    #[tokio::test]
    async fn health_check_returns_unhealthy_when_disconnected() {
        let adapter = TelegramAdapter::new("telegram");
        let status = adapter.health_check().await.unwrap();
        assert_eq!(status, HealthStatus::Unhealthy);
    }

    #[tokio::test]
    async fn health_check_returns_degraded_when_connected_without_token() {
        let mut adapter = TelegramAdapter::new("telegram");
        adapter.connected = true;
        let status = adapter.health_check().await.unwrap();
        assert_eq!(status, HealthStatus::Degraded);
    }

    #[tokio::test]
    async fn health_check_returns_healthy_when_connected_with_token() {
        let mut adapter = TelegramAdapter::with_token("telegram", "valid-token");
        adapter.connected = true;
        let status = adapter.health_check().await.unwrap();
        assert_eq!(status, HealthStatus::Healthy);
    }

    // -- Token resolution --

    #[test]
    fn resolve_token_succeeds_with_token() {
        let adapter = TelegramAdapter::with_token("telegram", "my-token");
        let token = adapter.resolve_token().unwrap();
        assert_eq!(token, "my-token");
    }

    #[test]
    fn resolve_token_fails_without_token() {
        let adapter = TelegramAdapter::new("telegram");
        let result = adapter.resolve_token();
        assert!(result.is_err());
    }

    // -- Response parsing --

    #[test]
    fn parse_telegram_response_succeeds_on_ok_true() {
        let resp = json!({ "ok": true, "result": { "message_id": 42 } });
        let result = TelegramAdapter::parse_telegram_response(&resp, "test_tool");
        assert!(result.is_ok());
    }

    #[test]
    fn parse_telegram_response_fails_on_ok_false() {
        let resp = json!({ "ok": false, "error_code": 401, "description": "Unauthorized" });
        let result = TelegramAdapter::parse_telegram_response(&resp, "test_tool");
        assert!(result.is_err());
        let err_msg = result.unwrap_err().to_string();
        assert!(err_msg.contains("401"));
        assert!(err_msg.contains("Unauthorized"));
    }

    #[test]
    fn parse_telegram_response_fails_on_missing_ok() {
        let resp = json!({});
        let result = TelegramAdapter::parse_telegram_response(&resp, "test_tool");
        assert!(result.is_err());
    }

    // -- URL construction --

    #[test]
    fn api_url_constructs_correct_url() {
        let adapter = TelegramAdapter::with_token("telegram", "123456:ABC-DEF");
        let url = adapter.api_url("sendMessage").unwrap();
        assert_eq!(
            url,
            "https://api.telegram.org/bot123456:ABC-DEF/sendMessage"
        );
    }

    #[test]
    fn api_url_fails_without_token() {
        let adapter = TelegramAdapter::new("telegram");
        let result = adapter.api_url("sendMessage");
        assert!(result.is_err());
    }

    // -- Execute tool when not connected --

    #[tokio::test]
    async fn execute_tool_rejects_when_not_connected() {
        let adapter = TelegramAdapter::with_token("telegram", "token");
        let result = adapter
            .execute_tool("telegram_send_message", json!({}))
            .await;
        assert!(result.is_err());
        let err = result.unwrap_err();
        assert!(err.to_string().contains("not connected"));
    }

    // -- Execute tool rejects unknown tool --

    #[tokio::test]
    async fn execute_tool_rejects_unknown_tool() {
        let mut adapter = TelegramAdapter::new("telegram");
        adapter.connected = true;
        let result = adapter.execute_tool("nonexistent_tool", json!({})).await;
        assert!(result.is_err());
        let err = result.unwrap_err();
        assert!(err.to_string().contains("tool not found"));
    }

    // -- Missing required parameters --

    #[tokio::test]
    async fn send_message_rejects_missing_chat_id() {
        let mut adapter = TelegramAdapter::with_token("telegram", "token");
        adapter.connected = true;
        let result = adapter
            .execute_tool("telegram_send_message", json!({ "text": "hello" }))
            .await;
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("chat_id"));
    }

    #[tokio::test]
    async fn send_message_rejects_missing_text() {
        let mut adapter = TelegramAdapter::with_token("telegram", "token");
        adapter.connected = true;
        let result = adapter
            .execute_tool("telegram_send_message", json!({ "chat_id": "12345" }))
            .await;
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("text"));
    }

    #[tokio::test]
    async fn send_photo_rejects_missing_chat_id() {
        let mut adapter = TelegramAdapter::with_token("telegram", "token");
        adapter.connected = true;
        let result = adapter
            .execute_tool(
                "telegram_send_photo",
                json!({ "photo_url": "https://example.com/photo.jpg" }),
            )
            .await;
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("chat_id"));
    }

    #[tokio::test]
    async fn send_photo_rejects_missing_photo_url() {
        let mut adapter = TelegramAdapter::with_token("telegram", "token");
        adapter.connected = true;
        let result = adapter
            .execute_tool("telegram_send_photo", json!({ "chat_id": "12345" }))
            .await;
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("photo_url"));
    }

    #[tokio::test]
    async fn get_chat_rejects_missing_chat_id() {
        let mut adapter = TelegramAdapter::with_token("telegram", "token");
        adapter.connected = true;
        let result = adapter.execute_tool("telegram_get_chat", json!({})).await;
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("chat_id"));
    }

    #[tokio::test]
    async fn set_webhook_rejects_missing_url() {
        let mut adapter = TelegramAdapter::with_token("telegram", "token");
        adapter.connected = true;
        let result = adapter
            .execute_tool("telegram_set_webhook", json!({}))
            .await;
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("url"));
    }
}
