//! Slack Web API adapter for OpenIntentOS.
//!
//! Provides tools for interacting with Slack workspaces: sending messages,
//! listing channels, retrieving message history, uploading files, and adding
//! reactions.
//!
//! Authentication uses a bot token read from the `SLACK_BOT_TOKEN` environment
//! variable, or supplied per-call via the `token` parameter.

use async_trait::async_trait;
use serde_json::{Value, json};
use tracing::{debug, info, warn};

use crate::error::{AdapterError, Result};
use crate::proxy;
use crate::traits::{Adapter, AdapterType, AuthRequirement, HealthStatus, ToolDefinition};

/// Default Slack Web API base URL.
const DEFAULT_BASE_URL: &str = "https://slack.com/api";

/// Slack Web API adapter.
///
/// Provides tools for messaging, channel management, and file operations.
/// Authentication uses a bot token from the `SLACK_BOT_TOKEN` environment
/// variable, or supplied per-call via the `token` parameter.
pub struct SlackAdapter {
    /// Unique identifier for this adapter instance.
    id: String,
    /// Whether the adapter has been connected.
    connected: bool,
    /// Bot token (from env or pre-configured).
    bot_token: Option<String>,
    /// Base URL for the Slack API.
    base_url: String,
    /// HTTP client for making requests.
    client: reqwest::Client,
}

impl SlackAdapter {
    /// Create a new Slack adapter.
    ///
    /// Reads `SLACK_BOT_TOKEN` from the environment at construction time.
    pub fn new(id: impl Into<String>) -> Self {
        let client = proxy::build_client(std::time::Duration::from_secs(30))
            .user_agent("OpenIntentOS/0.1")
            .build()
            .unwrap_or_default();

        let bot_token = std::env::var("SLACK_BOT_TOKEN").ok();

        Self {
            id: id.into(),
            connected: false,
            bot_token,
            base_url: DEFAULT_BASE_URL.to_string(),
            client,
        }
    }

    /// Resolve the bot token from the per-call params or pre-configured value.
    fn resolve_token(&self, params: &Value) -> Result<String> {
        if let Some(t) = params.get("token").and_then(|v| v.as_str())
            && !t.is_empty()
        {
            return Ok(t.to_string());
        }
        self.bot_token
            .clone()
            .ok_or_else(|| AdapterError::AuthRequired {
                adapter_id: self.id.clone(),
                provider: "slack".to_string(),
            })
    }

    /// Parse a Slack API response and check the `ok` field.
    fn parse_slack_response(response: &Value, tool_name: &str) -> Result<()> {
        let ok = response.get("ok").and_then(|v| v.as_bool()).unwrap_or(false);
        if !ok {
            let error = response
                .get("error")
                .and_then(|v| v.as_str())
                .unwrap_or("unknown_error");
            return Err(AdapterError::ExecutionFailed {
                tool_name: tool_name.to_string(),
                reason: format!("Slack API error: {error}"),
            });
        }
        Ok(())
    }

    // -----------------------------------------------------------------------
    // Tool implementations
    // -----------------------------------------------------------------------

    async fn tool_send_message(&self, params: Value) -> Result<Value> {
        let token = self.resolve_token(&params)?;

        let channel = params
            .get("channel")
            .and_then(|v| v.as_str())
            .ok_or_else(|| AdapterError::InvalidParams {
                tool_name: "slack_send_message".into(),
                reason: "missing required string field `channel`".into(),
            })?;

        let text = params
            .get("text")
            .and_then(|v| v.as_str())
            .ok_or_else(|| AdapterError::InvalidParams {
                tool_name: "slack_send_message".into(),
                reason: "missing required string field `text`".into(),
            })?;

        let mut body = json!({
            "channel": channel,
            "text": text,
        });

        if let Some(thread_ts) = params.get("thread_ts").and_then(|v| v.as_str()) {
            body["thread_ts"] = json!(thread_ts);
        }

        debug!(channel = channel, "sending Slack message");

        let url = format!("{}/chat.postMessage", self.base_url);
        let response = self
            .client
            .post(&url)
            .bearer_auth(&token)
            .json(&body)
            .send()
            .await
            .map_err(|e| AdapterError::ExecutionFailed {
                tool_name: "slack_send_message".into(),
                reason: format!("request failed: {e}"),
            })?;

        let json_resp: Value =
            response
                .json()
                .await
                .map_err(|e| AdapterError::ExecutionFailed {
                    tool_name: "slack_send_message".into(),
                    reason: format!("failed to parse response: {e}"),
                })?;

        Self::parse_slack_response(&json_resp, "slack_send_message")?;

        Ok(json!({
            "success": true,
            "ts": json_resp.get("ts").cloned().unwrap_or(json!(null)),
            "channel": json_resp.get("channel").cloned().unwrap_or(json!(channel)),
        }))
    }

    async fn tool_list_channels(&self, params: Value) -> Result<Value> {
        let token = self.resolve_token(&params)?;

        let limit = params
            .get("limit")
            .and_then(|v| v.as_u64())
            .unwrap_or(100);

        let url = format!(
            "{}/conversations.list?limit={}&exclude_archived=true",
            self.base_url, limit
        );

        debug!("listing Slack channels");

        let response = self
            .client
            .get(&url)
            .bearer_auth(&token)
            .send()
            .await
            .map_err(|e| AdapterError::ExecutionFailed {
                tool_name: "slack_list_channels".into(),
                reason: format!("request failed: {e}"),
            })?;

        let json_resp: Value =
            response
                .json()
                .await
                .map_err(|e| AdapterError::ExecutionFailed {
                    tool_name: "slack_list_channels".into(),
                    reason: format!("failed to parse response: {e}"),
                })?;

        Self::parse_slack_response(&json_resp, "slack_list_channels")?;

        let channels = json_resp
            .get("channels")
            .cloned()
            .unwrap_or(json!([]));

        Ok(json!({
            "success": true,
            "channels": channels,
        }))
    }

    async fn tool_get_messages(&self, params: Value) -> Result<Value> {
        let token = self.resolve_token(&params)?;

        let channel = params
            .get("channel")
            .and_then(|v| v.as_str())
            .ok_or_else(|| AdapterError::InvalidParams {
                tool_name: "slack_get_messages".into(),
                reason: "missing required string field `channel`".into(),
            })?;

        let limit = params
            .get("limit")
            .and_then(|v| v.as_u64())
            .unwrap_or(20);

        let url = format!(
            "{}/conversations.history?channel={}&limit={}",
            self.base_url, channel, limit
        );

        debug!(channel = channel, "getting Slack messages");

        let response = self
            .client
            .get(&url)
            .bearer_auth(&token)
            .send()
            .await
            .map_err(|e| AdapterError::ExecutionFailed {
                tool_name: "slack_get_messages".into(),
                reason: format!("request failed: {e}"),
            })?;

        let json_resp: Value =
            response
                .json()
                .await
                .map_err(|e| AdapterError::ExecutionFailed {
                    tool_name: "slack_get_messages".into(),
                    reason: format!("failed to parse response: {e}"),
                })?;

        Self::parse_slack_response(&json_resp, "slack_get_messages")?;

        let messages = json_resp
            .get("messages")
            .cloned()
            .unwrap_or(json!([]));

        Ok(json!({
            "success": true,
            "messages": messages,
            "channel": channel,
        }))
    }

    async fn tool_upload_file(&self, params: Value) -> Result<Value> {
        let token = self.resolve_token(&params)?;

        let channel = params
            .get("channel")
            .and_then(|v| v.as_str())
            .ok_or_else(|| AdapterError::InvalidParams {
                tool_name: "slack_upload_file".into(),
                reason: "missing required string field `channel`".into(),
            })?;

        let filename = params
            .get("filename")
            .and_then(|v| v.as_str())
            .ok_or_else(|| AdapterError::InvalidParams {
                tool_name: "slack_upload_file".into(),
                reason: "missing required string field `filename`".into(),
            })?;

        let content = params
            .get("content")
            .and_then(|v| v.as_str())
            .ok_or_else(|| AdapterError::InvalidParams {
                tool_name: "slack_upload_file".into(),
                reason: "missing required string field `content`".into(),
            })?;

        let title = params
            .get("title")
            .and_then(|v| v.as_str())
            .unwrap_or(filename);

        debug!(channel = channel, filename = filename, "uploading file to Slack");

        let url = format!("{}/files.upload", self.base_url);
        let form = reqwest::multipart::Form::new()
            .text("channels", channel.to_string())
            .text("filename", filename.to_string())
            .text("title", title.to_string())
            .text("content", content.to_string());

        let response = self
            .client
            .post(&url)
            .bearer_auth(&token)
            .multipart(form)
            .send()
            .await
            .map_err(|e| AdapterError::ExecutionFailed {
                tool_name: "slack_upload_file".into(),
                reason: format!("request failed: {e}"),
            })?;

        let json_resp: Value =
            response
                .json()
                .await
                .map_err(|e| AdapterError::ExecutionFailed {
                    tool_name: "slack_upload_file".into(),
                    reason: format!("failed to parse response: {e}"),
                })?;

        Self::parse_slack_response(&json_resp, "slack_upload_file")?;

        Ok(json!({
            "success": true,
            "file": json_resp.get("file").cloned().unwrap_or(json!({})),
        }))
    }

    async fn tool_add_reaction(&self, params: Value) -> Result<Value> {
        let token = self.resolve_token(&params)?;

        let channel = params
            .get("channel")
            .and_then(|v| v.as_str())
            .ok_or_else(|| AdapterError::InvalidParams {
                tool_name: "slack_add_reaction".into(),
                reason: "missing required string field `channel`".into(),
            })?;

        let timestamp = params
            .get("timestamp")
            .and_then(|v| v.as_str())
            .ok_or_else(|| AdapterError::InvalidParams {
                tool_name: "slack_add_reaction".into(),
                reason: "missing required string field `timestamp`".into(),
            })?;

        let name = params
            .get("name")
            .and_then(|v| v.as_str())
            .ok_or_else(|| AdapterError::InvalidParams {
                tool_name: "slack_add_reaction".into(),
                reason: "missing required string field `name`".into(),
            })?;

        debug!(channel = channel, reaction = name, "adding Slack reaction");

        let body = json!({
            "channel": channel,
            "timestamp": timestamp,
            "name": name,
        });

        let url = format!("{}/reactions.add", self.base_url);
        let response = self
            .client
            .post(&url)
            .bearer_auth(&token)
            .json(&body)
            .send()
            .await
            .map_err(|e| AdapterError::ExecutionFailed {
                tool_name: "slack_add_reaction".into(),
                reason: format!("request failed: {e}"),
            })?;

        let json_resp: Value =
            response
                .json()
                .await
                .map_err(|e| AdapterError::ExecutionFailed {
                    tool_name: "slack_add_reaction".into(),
                    reason: format!("failed to parse response: {e}"),
                })?;

        Self::parse_slack_response(&json_resp, "slack_add_reaction")?;

        Ok(json!({
            "success": true,
        }))
    }
}

#[async_trait]
impl Adapter for SlackAdapter {
    fn id(&self) -> &str {
        &self.id
    }

    fn adapter_type(&self) -> AdapterType {
        AdapterType::Messaging
    }

    async fn connect(&mut self) -> Result<()> {
        if self.bot_token.is_some() {
            // Verify the token works by calling auth.test.
            let url = format!("{}/auth.test", self.base_url);
            let token = self.bot_token.as_ref().expect("checked above");
            match self
                .client
                .post(&url)
                .bearer_auth(token)
                .json(&json!({}))
                .send()
                .await
            {
                Ok(resp) => {
                    if let Ok(body) = resp.json::<Value>().await {
                        if body.get("ok").and_then(|v| v.as_bool()).unwrap_or(false) {
                            let team = body
                                .get("team")
                                .and_then(|v| v.as_str())
                                .unwrap_or("unknown");
                            info!(id = %self.id, team = team, "Slack adapter connected");
                        } else {
                            warn!(id = %self.id, "Slack auth.test returned ok=false");
                        }
                    }
                }
                Err(e) => {
                    warn!(id = %self.id, error = %e, "Slack auth.test failed, connecting without verification");
                }
            }
        } else {
            info!(id = %self.id, "Slack adapter connected without token (set SLACK_BOT_TOKEN)");
        }
        self.connected = true;
        Ok(())
    }

    async fn disconnect(&mut self) -> Result<()> {
        info!(id = %self.id, "Slack adapter disconnected");
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
                name: "slack_send_message".into(),
                description: "Send a message to a Slack channel or thread".into(),
                parameters: json!({
                    "type": "object",
                    "properties": {
                        "channel": {
                            "type": "string",
                            "description": "Channel ID or name (e.g. C01234ABCDE or #general)"
                        },
                        "text": {
                            "type": "string",
                            "description": "Message text (supports Slack mrkdwn formatting)"
                        },
                        "thread_ts": {
                            "type": "string",
                            "description": "Optional thread timestamp to reply in a thread"
                        },
                        "token": {
                            "type": "string",
                            "description": "Optional per-call Slack bot token (overrides SLACK_BOT_TOKEN)"
                        }
                    },
                    "required": ["channel", "text"]
                }),
            },
            ToolDefinition {
                name: "slack_list_channels".into(),
                description: "List public channels in the Slack workspace".into(),
                parameters: json!({
                    "type": "object",
                    "properties": {
                        "limit": {
                            "type": "integer",
                            "description": "Maximum number of channels to return (default: 100)"
                        },
                        "token": {
                            "type": "string",
                            "description": "Optional per-call Slack bot token"
                        }
                    },
                    "required": []
                }),
            },
            ToolDefinition {
                name: "slack_get_messages".into(),
                description: "Get recent messages from a Slack channel".into(),
                parameters: json!({
                    "type": "object",
                    "properties": {
                        "channel": {
                            "type": "string",
                            "description": "Channel ID to retrieve messages from"
                        },
                        "limit": {
                            "type": "integer",
                            "description": "Number of messages to retrieve (default: 20)"
                        },
                        "token": {
                            "type": "string",
                            "description": "Optional per-call Slack bot token"
                        }
                    },
                    "required": ["channel"]
                }),
            },
            ToolDefinition {
                name: "slack_upload_file".into(),
                description: "Upload a text file or content snippet to a Slack channel".into(),
                parameters: json!({
                    "type": "object",
                    "properties": {
                        "channel": {
                            "type": "string",
                            "description": "Channel ID to upload the file to"
                        },
                        "filename": {
                            "type": "string",
                            "description": "Name of the file"
                        },
                        "content": {
                            "type": "string",
                            "description": "Text content of the file"
                        },
                        "title": {
                            "type": "string",
                            "description": "Optional display title for the file"
                        },
                        "token": {
                            "type": "string",
                            "description": "Optional per-call Slack bot token"
                        }
                    },
                    "required": ["channel", "filename", "content"]
                }),
            },
            ToolDefinition {
                name: "slack_add_reaction".into(),
                description: "Add an emoji reaction to a Slack message".into(),
                parameters: json!({
                    "type": "object",
                    "properties": {
                        "channel": {
                            "type": "string",
                            "description": "Channel ID containing the message"
                        },
                        "timestamp": {
                            "type": "string",
                            "description": "Timestamp of the message to react to (from message `ts` field)"
                        },
                        "name": {
                            "type": "string",
                            "description": "Emoji name without colons (e.g. thumbsup, rocket)"
                        },
                        "token": {
                            "type": "string",
                            "description": "Optional per-call Slack bot token"
                        }
                    },
                    "required": ["channel", "timestamp", "name"]
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
            "slack_send_message" => self.tool_send_message(params).await,
            "slack_list_channels" => self.tool_list_channels(params).await,
            "slack_get_messages" => self.tool_get_messages(params).await,
            "slack_upload_file" => self.tool_upload_file(params).await,
            "slack_add_reaction" => self.tool_add_reaction(params).await,
            _ => Err(AdapterError::ToolNotFound {
                adapter_id: self.id.clone(),
                tool_name: name.to_string(),
            }),
        }
    }

    fn required_auth(&self) -> Option<AuthRequirement> {
        Some(AuthRequirement {
            provider: "slack".into(),
            scopes: vec![
                "channels:read".into(),
                "chat:write".into(),
                "files:write".into(),
                "reactions:write".into(),
                "channels:history".into(),
            ],
        })
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn new_creates_adapter() {
        // SLACK_BOT_TOKEN may or may not be set in test env; just check defaults.
        let adapter = SlackAdapter::new("slack-test");
        assert_eq!(adapter.id, "slack-test");
        assert!(!adapter.connected);
        assert_eq!(adapter.base_url, DEFAULT_BASE_URL);
    }

    #[test]
    fn adapter_id_returns_id() {
        let adapter = SlackAdapter::new("my-slack");
        assert_eq!(adapter.id(), "my-slack");
    }

    #[test]
    fn adapter_type_is_messaging() {
        let adapter = SlackAdapter::new("slack");
        assert_eq!(adapter.adapter_type(), AdapterType::Messaging);
    }

    #[test]
    fn required_auth_returns_slack_scopes() {
        let adapter = SlackAdapter::new("slack");
        let auth = adapter.required_auth().expect("should require auth");
        assert_eq!(auth.provider, "slack");
        assert!(auth.scopes.contains(&"chat:write".to_string()));
        assert!(auth.scopes.contains(&"channels:read".to_string()));
    }

    #[test]
    fn tools_returns_exactly_five() {
        let adapter = SlackAdapter::new("slack");
        assert_eq!(adapter.tools().len(), 5);
    }

    #[test]
    fn tools_have_expected_names() {
        let adapter = SlackAdapter::new("slack");
        let names: Vec<String> = adapter.tools().iter().map(|t| t.name.clone()).collect();
        let expected = vec![
            "slack_send_message",
            "slack_list_channels",
            "slack_get_messages",
            "slack_upload_file",
            "slack_add_reaction",
        ];
        assert_eq!(names, expected);
    }

    #[test]
    fn tool_send_message_has_required_fields() {
        let adapter = SlackAdapter::new("slack");
        let tools = adapter.tools();
        let t = tools.iter().find(|t| t.name == "slack_send_message").unwrap();
        let required = t.parameters["required"].as_array().unwrap();
        assert!(required.contains(&json!("channel")));
        assert!(required.contains(&json!("text")));
    }

    #[test]
    fn tool_list_channels_has_no_required_fields() {
        let adapter = SlackAdapter::new("slack");
        let tools = adapter.tools();
        let t = tools.iter().find(|t| t.name == "slack_list_channels").unwrap();
        let required = t.parameters["required"].as_array().unwrap();
        assert!(required.is_empty());
    }

    #[test]
    fn parse_slack_response_ok_true() {
        let resp = json!({ "ok": true });
        assert!(SlackAdapter::parse_slack_response(&resp, "test").is_ok());
    }

    #[test]
    fn parse_slack_response_ok_false() {
        let resp = json!({ "ok": false, "error": "invalid_auth" });
        let result = SlackAdapter::parse_slack_response(&resp, "test");
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("invalid_auth"));
    }

    #[tokio::test]
    async fn connect_succeeds_without_token() {
        let mut adapter = SlackAdapter::new("slack");
        // Clear any token that might be in env.
        adapter.bot_token = None;
        let result = adapter.connect().await;
        assert!(result.is_ok());
        assert!(adapter.connected);
    }

    #[tokio::test]
    async fn disconnect_sets_disconnected() {
        let mut adapter = SlackAdapter::new("slack");
        adapter.connected = true;
        adapter.disconnect().await.unwrap();
        assert!(!adapter.connected);
    }

    #[tokio::test]
    async fn health_check_unhealthy_when_disconnected() {
        let adapter = SlackAdapter::new("slack");
        assert_eq!(
            adapter.health_check().await.unwrap(),
            HealthStatus::Unhealthy
        );
    }

    #[tokio::test]
    async fn health_check_degraded_when_no_token() {
        let mut adapter = SlackAdapter::new("slack");
        adapter.connected = true;
        adapter.bot_token = None;
        assert_eq!(
            adapter.health_check().await.unwrap(),
            HealthStatus::Degraded
        );
    }

    #[tokio::test]
    async fn execute_tool_rejects_when_not_connected() {
        let adapter = SlackAdapter::new("slack");
        let result = adapter
            .execute_tool("slack_send_message", json!({"channel": "C123", "text": "hi"}))
            .await;
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("not connected"));
    }

    #[tokio::test]
    async fn execute_tool_rejects_unknown_tool() {
        let mut adapter = SlackAdapter::new("slack");
        adapter.connected = true;
        adapter.bot_token = Some("xoxb-fake".into());
        let result = adapter.execute_tool("nonexistent_tool", json!({})).await;
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("tool not found"));
    }

    #[tokio::test]
    async fn send_message_rejects_missing_channel() {
        let mut adapter = SlackAdapter::new("slack");
        adapter.connected = true;
        adapter.bot_token = Some("xoxb-fake".into());
        let result = adapter
            .execute_tool("slack_send_message", json!({"text": "hello"}))
            .await;
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("channel"));
    }

    #[tokio::test]
    async fn send_message_rejects_missing_text() {
        let mut adapter = SlackAdapter::new("slack");
        adapter.connected = true;
        adapter.bot_token = Some("xoxb-fake".into());
        let result = adapter
            .execute_tool("slack_send_message", json!({"channel": "C123"}))
            .await;
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("text"));
    }

    #[tokio::test]
    async fn get_messages_rejects_missing_channel() {
        let mut adapter = SlackAdapter::new("slack");
        adapter.connected = true;
        adapter.bot_token = Some("xoxb-fake".into());
        let result = adapter
            .execute_tool("slack_get_messages", json!({}))
            .await;
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("channel"));
    }

    #[tokio::test]
    async fn upload_file_rejects_missing_filename() {
        let mut adapter = SlackAdapter::new("slack");
        adapter.connected = true;
        adapter.bot_token = Some("xoxb-fake".into());
        let result = adapter
            .execute_tool(
                "slack_upload_file",
                json!({"channel": "C123", "content": "data"}),
            )
            .await;
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("filename"));
    }

    #[tokio::test]
    async fn add_reaction_rejects_missing_name() {
        let mut adapter = SlackAdapter::new("slack");
        adapter.connected = true;
        adapter.bot_token = Some("xoxb-fake".into());
        let result = adapter
            .execute_tool(
                "slack_add_reaction",
                json!({"channel": "C123", "timestamp": "1234567890.000001"}),
            )
            .await;
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("name"));
    }

    #[test]
    fn resolve_token_uses_per_call_token() {
        let adapter = SlackAdapter::new("slack");
        let token = adapter
            .resolve_token(&json!({"token": "per-call-token"}))
            .unwrap();
        assert_eq!(token, "per-call-token");
    }

    #[test]
    fn resolve_token_fails_when_none() {
        let mut adapter = SlackAdapter::new("slack");
        adapter.bot_token = None;
        let result = adapter.resolve_token(&json!({}));
        assert!(result.is_err());
    }
}
