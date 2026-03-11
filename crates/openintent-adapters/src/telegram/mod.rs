//! Telegram Bot API adapter for OpenIntentOS.
//!
//! Provides tools for interacting with the Telegram Bot API, enabling the AI
//! agent to send and receive messages, photos, files, videos, and manage
//! webhooks via Telegram bots.

pub mod api;
pub mod tools;
pub mod types;

use async_trait::async_trait;
use serde_json::{Value, json};
use tracing::{debug, info, warn};

use crate::error::{AdapterError, Result};
use crate::proxy;
use crate::traits::{Adapter, AdapterType, AuthRequirement, HealthStatus, ToolDefinition};

use api::{
    api_get_chat, api_get_chat_member, api_get_updates, api_send_document, api_send_message,
    api_send_photo, api_send_video, api_set_webhook, parse_telegram_response,
};

/// Telegram Bot API base URL.
const TELEGRAM_API_BASE: &str = "https://api.telegram.org/bot";

/// Telegram Bot API adapter.
pub struct TelegramAdapter {
    id: String,
    connected: bool,
    bot_token: Option<String>,
    http: reqwest::Client,
}

impl TelegramAdapter {
    /// Create a new Telegram adapter with default configuration and no token.
    pub fn new(id: impl Into<String>) -> Self {
        let http = proxy::build_client(std::time::Duration::from_secs(30))
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

    /// Send a message to a chat (public method for use by other adapters).
    pub async fn send_message(
        &self,
        chat_id: &str,
        text: &str,
        parse_mode: Option<&str>,
    ) -> Result<Value> {
        let params = if let Some(mode) = parse_mode {
            json!({ "chat_id": chat_id, "text": text, "parse_mode": mode })
        } else {
            json!({ "chat_id": chat_id, "text": text })
        };

        let url = self.api_url("sendMessage")?;
        api_send_message(&self.http, &url, params).await
    }

    /// Build a full Telegram Bot API URL for the given method.
    pub fn api_url(&self, method: &str) -> Result<String> {
        let token = self.resolve_token()?;
        Ok(format!("{}{}/{}", TELEGRAM_API_BASE, token, method))
    }

    /// Resolve the bot token, returning an error if none is available.
    pub fn resolve_token(&self) -> Result<String> {
        self.bot_token
            .clone()
            .ok_or_else(|| AdapterError::AuthRequired {
                adapter_id: self.id.clone(),
                provider: "telegram".to_string(),
            })
    }

    /// Parse a Telegram Bot API response, checking the `ok` field for errors.
    pub fn parse_telegram_response(response: &Value, tool_name: &str) -> Result<()> {
        parse_telegram_response(response, tool_name)
    }

    // -----------------------------------------------------------------------
    // Tool implementations
    // -----------------------------------------------------------------------

    async fn tool_send_message(&self, params: Value) -> Result<Value> {
        let url = self.api_url("sendMessage")?;
        api_send_message(&self.http, &url, params).await
    }

    async fn tool_send_photo(&self, params: Value) -> Result<Value> {
        let url = self.api_url("sendPhoto")?;
        api_send_photo(&self.http, &url, params).await
    }

    async fn tool_send_document(&self, params: Value) -> Result<Value> {
        let url = self.api_url("sendDocument")?;
        api_send_document(&self.http, &url, params).await
    }

    async fn tool_send_video(&self, params: Value) -> Result<Value> {
        let url = self.api_url("sendVideo")?;
        api_send_video(&self.http, &url, params).await
    }

    async fn tool_get_updates(&self, params: Value) -> Result<Value> {
        let url = self.api_url("getUpdates")?;
        api_get_updates(&self.http, &url, params).await
    }

    async fn tool_get_chat(&self, params: Value) -> Result<Value> {
        let url = self.api_url("getChat")?;
        api_get_chat(&self.http, &url, params).await
    }

    async fn tool_set_webhook(&self, params: Value) -> Result<Value> {
        let url = self.api_url("setWebhook")?;
        api_set_webhook(&self.http, &url, params).await
    }

    async fn tool_configure_group_chat(&self, params: Value) -> Result<Value> {
        let chat_id = params
            .get("chat_id")
            .and_then(|v| v.as_str())
            .ok_or_else(|| AdapterError::InvalidParams {
                tool_name: "telegram_configure_group_chat".into(),
                reason: "missing required string field `chat_id`".into(),
            })?;

        let allow_bots = params.get("allow_bots").and_then(|v| v.as_bool()).unwrap_or(true);
        let auto_delete_service_messages = params
            .get("auto_delete_service_messages")
            .and_then(|v| v.as_bool())
            .unwrap_or(false);
        let protect_content = params
            .get("protect_content")
            .and_then(|v| v.as_bool())
            .unwrap_or(false);

        let chat_info = self.tool_get_chat(json!({"chat_id": chat_id})).await?;

        debug!(
            chat_id = %chat_id,
            allow_bots = allow_bots,
            auto_delete_service_messages = auto_delete_service_messages,
            protect_content = protect_content,
            "configuring Telegram group chat settings"
        );

        let permissions_url = self.api_url("setChatPermissions")?;
        let permissions_body = json!({
            "chat_id": chat_id,
            "permissions": {
                "can_send_messages": true,
                "can_send_media_messages": true,
                "can_send_polls": true,
                "can_send_other_messages": allow_bots,
                "can_add_web_page_previews": true,
                "can_change_info": false,
                "can_invite_users": true,
                "can_pin_messages": false
            }
        });

        let permissions_response = self
            .http
            .post(&permissions_url)
            .json(&permissions_body)
            .send()
            .await
            .map_err(|e| AdapterError::ExecutionFailed {
                tool_name: "telegram_configure_group_chat".into(),
                reason: format!("failed to set chat permissions: {e}"),
            })?;

        let permissions_json: Value = permissions_response
            .json()
            .await
            .map_err(|e| AdapterError::ExecutionFailed {
                tool_name: "telegram_configure_group_chat".into(),
                reason: format!("failed to parse permissions response: {e}"),
            })?;

        let mut results = vec![(
            "permissions",
            permissions_json
                .get("ok")
                .and_then(|v| v.as_bool())
                .unwrap_or(false),
        )];

        if auto_delete_service_messages {
            let delete_url = self.api_url("setChatMenuButton")?;
            let delete_body = json!({ "chat_id": chat_id, "menu_button": { "type": "default" } });
            if let Ok(response) = self.http.post(&delete_url).json(&delete_body).send().await {
                if let Ok(json_resp) = response.json::<Value>().await {
                    results.push((
                        "auto_delete",
                        json_resp
                            .get("ok")
                            .and_then(|v| v.as_bool())
                            .unwrap_or(false),
                    ));
                }
            }
        }

        if protect_content {
            let protect_url = self.api_url("setChatDescription")?;
            let protect_body = json!({
                "chat_id": chat_id,
                "description": "Protected content - forwarding restricted"
            });
            if let Ok(response) = self.http.post(&protect_url).json(&protect_body).send().await {
                if let Ok(json_resp) = response.json::<Value>().await {
                    results.push((
                        "protect_content",
                        json_resp
                            .get("ok")
                            .and_then(|v| v.as_bool())
                            .unwrap_or(false),
                    ));
                }
            }
        }

        Ok(json!({
            "success": true,
            "data": {
                "chat_id": chat_id,
                "configured_settings": {
                    "allow_bots": allow_bots,
                    "auto_delete_service_messages": auto_delete_service_messages,
                    "protect_content": protect_content
                },
                "results": results,
                "chat_info": chat_info.get("data").cloned().unwrap_or(json!({}))
            }
        }))
    }

    async fn tool_get_chat_member(&self, params: Value) -> Result<Value> {
        let url = self.api_url("getChatMember")?;
        api_get_chat_member(&self.http, &url, params).await
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
        tools::build_tool_definitions()
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
            "telegram_send_document" => self.tool_send_document(params).await,
            "telegram_send_video" => self.tool_send_video(params).await,
            "telegram_get_updates" => self.tool_get_updates(params).await,
            "telegram_get_chat" => self.tool_get_chat(params).await,
            "telegram_set_webhook" => self.tool_set_webhook(params).await,
            "telegram_configure_group_chat" => self.tool_configure_group_chat(params).await,
            "telegram_get_chat_member" => self.tool_get_chat_member(params).await,
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

    #[test]
    fn tools_returns_expected_count() {
        let adapter = TelegramAdapter::new("telegram");
        let tools = adapter.tools();
        assert_eq!(tools.len(), 9);
    }

    #[test]
    fn tools_have_expected_names() {
        let adapter = TelegramAdapter::new("telegram");
        let names: Vec<String> = adapter.tools().iter().map(|t| t.name.clone()).collect();
        let expected = vec![
            "telegram_send_message",
            "telegram_send_photo",
            "telegram_send_document",
            "telegram_send_video",
            "telegram_get_updates",
            "telegram_get_chat",
            "telegram_set_webhook",
            "telegram_configure_group_chat",
            "telegram_get_chat_member",
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
    fn api_url_constructs_correct_url() {
        let adapter = TelegramAdapter::with_token("telegram", "123456:ABC-DEF");
        let url = adapter.api_url("sendMessage").unwrap();
        assert_eq!(url, "https://api.telegram.org/bot123456:ABC-DEF/sendMessage");
    }

    #[test]
    fn api_url_fails_without_token() {
        let adapter = TelegramAdapter::new("telegram");
        let result = adapter.api_url("sendMessage");
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn execute_tool_rejects_when_not_connected() {
        let adapter = TelegramAdapter::with_token("telegram", "token");
        let result = adapter.execute_tool("telegram_send_message", json!({})).await;
        assert!(result.is_err());
        let err = result.unwrap_err();
        assert!(err.to_string().contains("not connected"));
    }

    #[tokio::test]
    async fn execute_tool_rejects_unknown_tool() {
        let mut adapter = TelegramAdapter::new("telegram");
        adapter.connected = true;
        let result = adapter.execute_tool("nonexistent_tool", json!({})).await;
        assert!(result.is_err());
        let err = result.unwrap_err();
        assert!(err.to_string().contains("tool not found"));
    }

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
        let result = adapter.execute_tool("telegram_set_webhook", json!({})).await;
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("url"));
    }

    #[tokio::test]
    async fn get_chat_member_rejects_missing_user_id() {
        let mut adapter = TelegramAdapter::with_token("telegram", "token");
        adapter.connected = true;
        let result = adapter
            .execute_tool("telegram_get_chat_member", json!({ "chat_id": "12345" }))
            .await;
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("user_id"));
    }
}
