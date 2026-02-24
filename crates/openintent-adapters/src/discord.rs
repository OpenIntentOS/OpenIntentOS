//! Discord Bot API adapter for OpenIntentOS.
//!
//! Provides tools for interacting with Discord servers via the Discord Bot API
//! (v10).  Supports sending messages, retrieving messages, fetching channel and
//! guild info, and adding reactions to messages.

use async_trait::async_trait;
use serde_json::{Value, json};
use tracing::{debug, info, warn};

use crate::error::{AdapterError, Result};
use crate::traits::{Adapter, AdapterType, AuthRequirement, HealthStatus, ToolDefinition};

/// Discord API v10 base URL.
const API_BASE_URL: &str = "https://discord.com/api/v10";

/// Discord Bot API adapter.
///
/// Provides tools for messaging, channel management, guild info, and reactions.
/// Authentication uses a bot token supplied via the `DISCORD_BOT_TOKEN`
/// environment variable.
pub struct DiscordAdapter {
    /// Unique identifier for this adapter instance.
    id: String,
    /// Whether the adapter has been connected.
    connected: bool,
    /// Discord bot token for authentication.
    bot_token: Option<String>,
    /// HTTP client for making requests.
    http: reqwest::Client,
}

impl DiscordAdapter {
    /// Create a new Discord adapter with default configuration and no token.
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

    /// Create a new Discord adapter with a pre-configured bot token.
    pub fn with_token(id: impl Into<String>, bot_token: impl Into<String>) -> Self {
        let mut adapter = Self::new(id);
        adapter.bot_token = Some(bot_token.into());
        adapter
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
                provider: "discord".to_string(),
            })
    }

    // -----------------------------------------------------------------------
    // URL construction
    // -----------------------------------------------------------------------

    /// Build a full API URL from a path segment.
    fn api_url(path: &str) -> String {
        format!("{}{}", API_BASE_URL, path)
    }

    // -----------------------------------------------------------------------
    // HTTP helpers
    // -----------------------------------------------------------------------

    /// Build a GET request with Discord Bot authorization header.
    fn get_request(&self, url: &str, token: &str) -> reqwest::RequestBuilder {
        self.http
            .get(url)
            .header("Authorization", format!("Bot {token}"))
            .header("Content-Type", "application/json")
    }

    /// Build a POST request with Discord Bot authorization header.
    fn post_request(&self, url: &str, token: &str) -> reqwest::RequestBuilder {
        self.http
            .post(url)
            .header("Authorization", format!("Bot {token}"))
            .header("Content-Type", "application/json")
    }

    /// Build a PUT request with Discord Bot authorization header.
    fn put_request(&self, url: &str, token: &str) -> reqwest::RequestBuilder {
        self.http
            .put(url)
            .header("Authorization", format!("Bot {token}"))
            .header("Content-Length", "0")
    }

    // -----------------------------------------------------------------------
    // Response helpers
    // -----------------------------------------------------------------------

    /// Parse an HTTP response, returning an error on non-success status codes.
    async fn parse_response(response: reqwest::Response, tool_name: &str) -> Result<Value> {
        let status = response.status();

        // Discord returns 204 No Content for some successful mutations (e.g. reactions).
        if status == reqwest::StatusCode::NO_CONTENT {
            return Ok(json!({ "success": true }));
        }

        let json_resp: Value =
            response
                .json()
                .await
                .map_err(|e| AdapterError::ExecutionFailed {
                    tool_name: tool_name.to_string(),
                    reason: format!("failed to parse response: {e}"),
                })?;

        if !status.is_success() {
            let message = json_resp
                .get("message")
                .and_then(|v| v.as_str())
                .unwrap_or("unknown error");
            let code = json_resp.get("code").and_then(|v| v.as_i64()).unwrap_or(-1);
            return Err(AdapterError::ExecutionFailed {
                tool_name: tool_name.to_string(),
                reason: format!("Discord API error (code {code}, status {status}): {message}"),
            });
        }

        Ok(json_resp)
    }

    // -----------------------------------------------------------------------
    // Tool implementations
    // -----------------------------------------------------------------------

    /// Send a message to a Discord channel.
    async fn tool_send_message(&self, params: Value) -> Result<Value> {
        let token = self.resolve_token()?;

        let channel_id = params
            .get("channel_id")
            .and_then(|v| v.as_str())
            .ok_or_else(|| AdapterError::InvalidParams {
                tool_name: "discord_send_message".into(),
                reason: "missing required string field `channel_id`".into(),
            })?;

        let content = params
            .get("content")
            .and_then(|v| v.as_str())
            .ok_or_else(|| AdapterError::InvalidParams {
                tool_name: "discord_send_message".into(),
                reason: "missing required string field `content`".into(),
            })?;

        let url = Self::api_url(&format!("/channels/{channel_id}/messages"));
        let body = json!({ "content": content });

        debug!(url = %url, channel_id = %channel_id, "sending Discord message");

        let response = self
            .post_request(&url, &token)
            .json(&body)
            .send()
            .await
            .map_err(|e| AdapterError::ExecutionFailed {
                tool_name: "discord_send_message".into(),
                reason: format!("failed to send message: {e}"),
            })?;

        let data = Self::parse_response(response, "discord_send_message").await?;

        Ok(json!({
            "success": true,
            "data": data,
        }))
    }

    /// Get recent messages from a Discord channel.
    async fn tool_get_messages(&self, params: Value) -> Result<Value> {
        let token = self.resolve_token()?;

        let channel_id = params
            .get("channel_id")
            .and_then(|v| v.as_str())
            .ok_or_else(|| AdapterError::InvalidParams {
                tool_name: "discord_get_messages".into(),
                reason: "missing required string field `channel_id`".into(),
            })?;

        let limit = params.get("limit").and_then(|v| v.as_u64()).unwrap_or(50);

        let url = Self::api_url(&format!("/channels/{channel_id}/messages?limit={limit}"));

        debug!(url = %url, channel_id = %channel_id, limit = %limit, "getting Discord messages");

        let response = self.get_request(&url, &token).send().await.map_err(|e| {
            AdapterError::ExecutionFailed {
                tool_name: "discord_get_messages".into(),
                reason: format!("failed to get messages: {e}"),
            }
        })?;

        let data = Self::parse_response(response, "discord_get_messages").await?;

        Ok(json!({
            "success": true,
            "data": data,
        }))
    }

    /// Get information about a Discord channel.
    async fn tool_get_channel(&self, params: Value) -> Result<Value> {
        let token = self.resolve_token()?;

        let channel_id = params
            .get("channel_id")
            .and_then(|v| v.as_str())
            .ok_or_else(|| AdapterError::InvalidParams {
                tool_name: "discord_get_channel".into(),
                reason: "missing required string field `channel_id`".into(),
            })?;

        let url = Self::api_url(&format!("/channels/{channel_id}"));

        debug!(url = %url, channel_id = %channel_id, "getting Discord channel info");

        let response = self.get_request(&url, &token).send().await.map_err(|e| {
            AdapterError::ExecutionFailed {
                tool_name: "discord_get_channel".into(),
                reason: format!("failed to get channel: {e}"),
            }
        })?;

        let data = Self::parse_response(response, "discord_get_channel").await?;

        Ok(json!({
            "success": true,
            "data": data,
        }))
    }

    /// Get information about a Discord guild (server).
    async fn tool_get_guild(&self, params: Value) -> Result<Value> {
        let token = self.resolve_token()?;

        let guild_id = params
            .get("guild_id")
            .and_then(|v| v.as_str())
            .ok_or_else(|| AdapterError::InvalidParams {
                tool_name: "discord_get_guild".into(),
                reason: "missing required string field `guild_id`".into(),
            })?;

        let url = Self::api_url(&format!("/guilds/{guild_id}"));

        debug!(url = %url, guild_id = %guild_id, "getting Discord guild info");

        let response = self.get_request(&url, &token).send().await.map_err(|e| {
            AdapterError::ExecutionFailed {
                tool_name: "discord_get_guild".into(),
                reason: format!("failed to get guild: {e}"),
            }
        })?;

        let data = Self::parse_response(response, "discord_get_guild").await?;

        Ok(json!({
            "success": true,
            "data": data,
        }))
    }

    /// Add a reaction to a Discord message.
    async fn tool_create_reaction(&self, params: Value) -> Result<Value> {
        let token = self.resolve_token()?;

        let channel_id = params
            .get("channel_id")
            .and_then(|v| v.as_str())
            .ok_or_else(|| AdapterError::InvalidParams {
                tool_name: "discord_create_reaction".into(),
                reason: "missing required string field `channel_id`".into(),
            })?;

        let message_id = params
            .get("message_id")
            .and_then(|v| v.as_str())
            .ok_or_else(|| AdapterError::InvalidParams {
                tool_name: "discord_create_reaction".into(),
                reason: "missing required string field `message_id`".into(),
            })?;

        let emoji = params
            .get("emoji")
            .and_then(|v| v.as_str())
            .ok_or_else(|| AdapterError::InvalidParams {
                tool_name: "discord_create_reaction".into(),
                reason: "missing required string field `emoji`".into(),
            })?;

        // URL-encode the emoji for the path segment.
        let encoded_emoji = urlencoding::encode(emoji);
        let url = Self::api_url(&format!(
            "/channels/{channel_id}/messages/{message_id}/reactions/{encoded_emoji}/@me"
        ));

        debug!(
            url = %url,
            channel_id = %channel_id,
            message_id = %message_id,
            emoji = %emoji,
            "creating Discord reaction"
        );

        let response = self.put_request(&url, &token).send().await.map_err(|e| {
            AdapterError::ExecutionFailed {
                tool_name: "discord_create_reaction".into(),
                reason: format!("failed to create reaction: {e}"),
            }
        })?;

        let data = Self::parse_response(response, "discord_create_reaction").await?;

        Ok(json!({
            "success": true,
            "data": data,
        }))
    }
}

// ---------------------------------------------------------------------------
// Adapter trait implementation
// ---------------------------------------------------------------------------

#[async_trait]
impl Adapter for DiscordAdapter {
    fn id(&self) -> &str {
        &self.id
    }

    fn adapter_type(&self) -> AdapterType {
        AdapterType::Messaging
    }

    async fn connect(&mut self) -> Result<()> {
        // Attempt to read the bot token from the environment if not already set.
        if self.bot_token.is_none() {
            match std::env::var("DISCORD_BOT_TOKEN") {
                Ok(token) if !token.is_empty() => {
                    info!(id = %self.id, "Discord adapter loaded bot token from environment");
                    self.bot_token = Some(token);
                }
                _ => {
                    warn!(
                        id = %self.id,
                        "DISCORD_BOT_TOKEN not set; Discord adapter connecting without auth"
                    );
                }
            }
        } else {
            info!(id = %self.id, "Discord adapter connected with pre-configured bot token");
        }

        self.connected = true;
        Ok(())
    }

    async fn disconnect(&mut self) -> Result<()> {
        info!(id = %self.id, "Discord adapter disconnected");
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
                name: "discord_send_message".into(),
                description: "Send a message to a Discord channel".into(),
                parameters: json!({
                    "type": "object",
                    "properties": {
                        "channel_id": {
                            "type": "string",
                            "description": "The ID of the Discord channel to send the message to"
                        },
                        "content": {
                            "type": "string",
                            "description": "The message content to send"
                        }
                    },
                    "required": ["channel_id", "content"]
                }),
            },
            ToolDefinition {
                name: "discord_get_messages".into(),
                description: "Get recent messages from a Discord channel".into(),
                parameters: json!({
                    "type": "object",
                    "properties": {
                        "channel_id": {
                            "type": "string",
                            "description": "The ID of the Discord channel to retrieve messages from"
                        },
                        "limit": {
                            "type": "integer",
                            "description": "Number of messages to retrieve (default: 50, max: 100)"
                        }
                    },
                    "required": ["channel_id"]
                }),
            },
            ToolDefinition {
                name: "discord_get_channel".into(),
                description: "Get information about a Discord channel".into(),
                parameters: json!({
                    "type": "object",
                    "properties": {
                        "channel_id": {
                            "type": "string",
                            "description": "The ID of the Discord channel"
                        }
                    },
                    "required": ["channel_id"]
                }),
            },
            ToolDefinition {
                name: "discord_get_guild".into(),
                description: "Get information about a Discord guild (server)".into(),
                parameters: json!({
                    "type": "object",
                    "properties": {
                        "guild_id": {
                            "type": "string",
                            "description": "The ID of the Discord guild"
                        }
                    },
                    "required": ["guild_id"]
                }),
            },
            ToolDefinition {
                name: "discord_create_reaction".into(),
                description: "Add a reaction emoji to a Discord message".into(),
                parameters: json!({
                    "type": "object",
                    "properties": {
                        "channel_id": {
                            "type": "string",
                            "description": "The ID of the channel containing the message"
                        },
                        "message_id": {
                            "type": "string",
                            "description": "The ID of the message to react to"
                        },
                        "emoji": {
                            "type": "string",
                            "description": "The emoji to react with (Unicode emoji or custom format name:id)"
                        }
                    },
                    "required": ["channel_id", "message_id", "emoji"]
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
            "discord_send_message" => self.tool_send_message(params).await,
            "discord_get_messages" => self.tool_get_messages(params).await,
            "discord_get_channel" => self.tool_get_channel(params).await,
            "discord_get_guild" => self.tool_get_guild(params).await,
            "discord_create_reaction" => self.tool_create_reaction(params).await,
            _ => Err(AdapterError::ToolNotFound {
                adapter_id: self.id.clone(),
                tool_name: name.to_string(),
            }),
        }
    }

    fn required_auth(&self) -> Option<AuthRequirement> {
        Some(AuthRequirement {
            provider: "discord".into(),
            scopes: vec![
                "bot".into(),
                "messages.read".into(),
                "messages.write".into(),
                "guilds.read".into(),
                "reactions.write".into(),
            ],
        })
    }
}

// ---------------------------------------------------------------------------
// URL encoding
// ---------------------------------------------------------------------------

mod urlencoding {
    /// Percent-encode a string for use in a URL path or query parameter.
    pub fn encode(input: &str) -> String {
        let mut encoded = String::with_capacity(input.len() * 2);
        for byte in input.bytes() {
            match byte {
                b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                    encoded.push(byte as char);
                }
                _ => {
                    encoded.push('%');
                    encoded.push_str(&format!("{byte:02X}"));
                }
            }
        }
        encoded
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
        let adapter = DiscordAdapter::new("discord-test");
        assert_eq!(adapter.id, "discord-test");
        assert!(!adapter.connected);
        assert!(adapter.bot_token.is_none());
    }

    #[test]
    fn with_token_sets_bot_token() {
        let adapter = DiscordAdapter::with_token("discord-test", "my-bot-token");
        assert_eq!(adapter.id, "discord-test");
        assert_eq!(adapter.bot_token.as_deref(), Some("my-bot-token"));
    }

    // -- Adapter trait basics --

    #[test]
    fn adapter_id_returns_id() {
        let adapter = DiscordAdapter::new("my-discord");
        assert_eq!(adapter.id(), "my-discord");
    }

    #[test]
    fn adapter_type_is_messaging() {
        let adapter = DiscordAdapter::new("discord");
        assert_eq!(adapter.adapter_type(), AdapterType::Messaging);
    }

    #[test]
    fn required_auth_returns_discord_scopes() {
        let adapter = DiscordAdapter::new("discord");
        let auth = adapter.required_auth().expect("should require auth");
        assert_eq!(auth.provider, "discord");
        assert!(auth.scopes.contains(&"bot".to_string()));
        assert!(auth.scopes.contains(&"messages.read".to_string()));
        assert!(auth.scopes.contains(&"messages.write".to_string()));
        assert!(auth.scopes.contains(&"guilds.read".to_string()));
        assert!(auth.scopes.contains(&"reactions.write".to_string()));
    }

    // -- Tool definitions --

    #[test]
    fn tools_returns_exactly_five() {
        let adapter = DiscordAdapter::new("discord");
        let tools = adapter.tools();
        assert_eq!(tools.len(), 5);
    }

    #[test]
    fn tools_have_expected_names() {
        let adapter = DiscordAdapter::new("discord");
        let names: Vec<String> = adapter.tools().iter().map(|t| t.name.clone()).collect();
        let expected = vec![
            "discord_send_message",
            "discord_get_messages",
            "discord_get_channel",
            "discord_get_guild",
            "discord_create_reaction",
        ];
        assert_eq!(names, expected);
    }

    #[test]
    fn tool_send_message_has_required_fields() {
        let adapter = DiscordAdapter::new("discord");
        let tools = adapter.tools();
        let send_msg = tools
            .iter()
            .find(|t| t.name == "discord_send_message")
            .expect("should have discord_send_message");
        let required = send_msg.parameters["required"]
            .as_array()
            .expect("required should be an array");
        assert!(required.contains(&json!("channel_id")));
        assert!(required.contains(&json!("content")));
    }

    #[test]
    fn tool_get_messages_requires_channel_id_only() {
        let adapter = DiscordAdapter::new("discord");
        let tools = adapter.tools();
        let get_msgs = tools
            .iter()
            .find(|t| t.name == "discord_get_messages")
            .expect("should have discord_get_messages");
        let required = get_msgs.parameters["required"]
            .as_array()
            .expect("required should be an array");
        assert_eq!(required.len(), 1);
        assert!(required.contains(&json!("channel_id")));
    }

    #[test]
    fn tool_create_reaction_has_required_fields() {
        let adapter = DiscordAdapter::new("discord");
        let tools = adapter.tools();
        let react = tools
            .iter()
            .find(|t| t.name == "discord_create_reaction")
            .expect("should have discord_create_reaction");
        let required = react.parameters["required"]
            .as_array()
            .expect("required should be an array");
        assert!(required.contains(&json!("channel_id")));
        assert!(required.contains(&json!("message_id")));
        assert!(required.contains(&json!("emoji")));
    }

    // -- Connect / disconnect --

    #[tokio::test]
    async fn connect_succeeds_without_env_token() {
        let mut adapter = DiscordAdapter::new("discord");
        let result = adapter.connect().await;
        assert!(result.is_ok());
        assert!(adapter.connected);
    }

    #[tokio::test]
    async fn connect_uses_preexisting_token() {
        let mut adapter = DiscordAdapter::with_token("discord", "preexisting-token");
        adapter.connect().await.unwrap();
        assert!(adapter.connected);
        assert_eq!(adapter.bot_token.as_deref(), Some("preexisting-token"));
    }

    #[tokio::test]
    async fn disconnect_clears_token_and_sets_disconnected() {
        let mut adapter = DiscordAdapter::new("discord");
        adapter.connected = true;
        adapter.bot_token = Some("test-token".into());
        adapter.disconnect().await.unwrap();
        assert!(!adapter.connected);
        assert!(adapter.bot_token.is_none());
    }

    // -- Health check --

    #[tokio::test]
    async fn health_check_returns_unhealthy_when_disconnected() {
        let adapter = DiscordAdapter::new("discord");
        let status = adapter.health_check().await.unwrap();
        assert_eq!(status, HealthStatus::Unhealthy);
    }

    #[tokio::test]
    async fn health_check_returns_degraded_when_connected_without_token() {
        let mut adapter = DiscordAdapter::new("discord");
        adapter.connected = true;
        let status = adapter.health_check().await.unwrap();
        assert_eq!(status, HealthStatus::Degraded);
    }

    #[tokio::test]
    async fn health_check_returns_healthy_when_connected_with_token() {
        let mut adapter = DiscordAdapter::new("discord");
        adapter.connected = true;
        adapter.bot_token = Some("valid-token".into());
        let status = adapter.health_check().await.unwrap();
        assert_eq!(status, HealthStatus::Healthy);
    }

    // -- Token resolution --

    #[test]
    fn resolve_token_succeeds_with_token() {
        let mut adapter = DiscordAdapter::new("discord");
        adapter.bot_token = Some("my-token".into());
        let token = adapter.resolve_token().unwrap();
        assert_eq!(token, "my-token");
    }

    #[test]
    fn resolve_token_fails_without_token() {
        let adapter = DiscordAdapter::new("discord");
        let result = adapter.resolve_token();
        assert!(result.is_err());
    }

    // -- URL construction --

    #[test]
    fn api_url_constructs_correct_urls() {
        assert_eq!(
            DiscordAdapter::api_url("/channels/123/messages"),
            "https://discord.com/api/v10/channels/123/messages"
        );
        assert_eq!(
            DiscordAdapter::api_url("/guilds/456"),
            "https://discord.com/api/v10/guilds/456"
        );
    }

    // -- Execute tool when not connected --

    #[tokio::test]
    async fn execute_tool_rejects_when_not_connected() {
        let adapter = DiscordAdapter::with_token("discord", "token");
        let result = adapter
            .execute_tool("discord_send_message", json!({}))
            .await;
        assert!(result.is_err());
        let err = result.unwrap_err();
        assert!(err.to_string().contains("not connected"));
    }

    // -- Execute tool rejects unknown tool --

    #[tokio::test]
    async fn execute_tool_rejects_unknown_tool() {
        let mut adapter = DiscordAdapter::new("discord");
        adapter.connected = true;
        let result = adapter.execute_tool("nonexistent_tool", json!({})).await;
        assert!(result.is_err());
        let err = result.unwrap_err();
        assert!(err.to_string().contains("tool not found"));
    }

    // -- Missing required parameters --

    #[tokio::test]
    async fn send_message_rejects_missing_channel_id() {
        let mut adapter = DiscordAdapter::new("discord");
        adapter.connected = true;
        adapter.bot_token = Some("token".into());
        let result = adapter
            .execute_tool("discord_send_message", json!({ "content": "hello" }))
            .await;
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("channel_id"));
    }

    #[tokio::test]
    async fn send_message_rejects_missing_content() {
        let mut adapter = DiscordAdapter::new("discord");
        adapter.connected = true;
        adapter.bot_token = Some("token".into());
        let result = adapter
            .execute_tool("discord_send_message", json!({ "channel_id": "123" }))
            .await;
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("content"));
    }

    #[tokio::test]
    async fn get_messages_rejects_missing_channel_id() {
        let mut adapter = DiscordAdapter::new("discord");
        adapter.connected = true;
        adapter.bot_token = Some("token".into());
        let result = adapter
            .execute_tool("discord_get_messages", json!({}))
            .await;
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("channel_id"));
    }

    #[tokio::test]
    async fn get_channel_rejects_missing_channel_id() {
        let mut adapter = DiscordAdapter::new("discord");
        adapter.connected = true;
        adapter.bot_token = Some("token".into());
        let result = adapter.execute_tool("discord_get_channel", json!({})).await;
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("channel_id"));
    }

    #[tokio::test]
    async fn get_guild_rejects_missing_guild_id() {
        let mut adapter = DiscordAdapter::new("discord");
        adapter.connected = true;
        adapter.bot_token = Some("token".into());
        let result = adapter.execute_tool("discord_get_guild", json!({})).await;
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("guild_id"));
    }

    #[tokio::test]
    async fn create_reaction_rejects_missing_channel_id() {
        let mut adapter = DiscordAdapter::new("discord");
        adapter.connected = true;
        adapter.bot_token = Some("token".into());
        let result = adapter
            .execute_tool(
                "discord_create_reaction",
                json!({ "message_id": "123", "emoji": "thumbsup" }),
            )
            .await;
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("channel_id"));
    }

    #[tokio::test]
    async fn create_reaction_rejects_missing_message_id() {
        let mut adapter = DiscordAdapter::new("discord");
        adapter.connected = true;
        adapter.bot_token = Some("token".into());
        let result = adapter
            .execute_tool(
                "discord_create_reaction",
                json!({ "channel_id": "123", "emoji": "thumbsup" }),
            )
            .await;
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("message_id"));
    }

    #[tokio::test]
    async fn create_reaction_rejects_missing_emoji() {
        let mut adapter = DiscordAdapter::new("discord");
        adapter.connected = true;
        adapter.bot_token = Some("token".into());
        let result = adapter
            .execute_tool(
                "discord_create_reaction",
                json!({ "channel_id": "123", "message_id": "456" }),
            )
            .await;
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("emoji"));
    }
}
