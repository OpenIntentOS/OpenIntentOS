//! Feishu (Lark) API adapter for OpenIntentOS.
//!
//! Provides tools for interacting with the Feishu enterprise messenger by
//! ByteDance.  Supports sending messages, listing chats, retrieving messages,
//! creating documents, and searching users via the Feishu Open Platform REST
//! API.

use async_trait::async_trait;
use serde_json::{Value, json};
use tracing::{debug, info, warn};

use crate::error::{AdapterError, Result};
use crate::traits::{Adapter, AdapterType, AuthRequirement, HealthStatus, ToolDefinition};

/// Default Feishu Open Platform API base URL.
const DEFAULT_BASE_URL: &str = "https://open.feishu.cn/open-apis";

/// Feishu Open Platform REST API adapter.
///
/// Provides tools for messaging, chat management, document creation, and
/// user lookup.  Authentication uses tenant access tokens obtained via
/// app credentials (app_id + app_secret).
pub struct FeishuAdapter {
    /// Unique identifier for this adapter instance.
    id: String,
    /// Whether the adapter has been connected.
    connected: bool,
    /// Feishu app ID for authentication.
    app_id: Option<String>,
    /// Feishu app secret for authentication.
    app_secret: Option<String>,
    /// Cached tenant access token (expires after ~2 hours).
    tenant_access_token: Option<String>,
    /// Base URL for the Feishu API.
    base_url: String,
    /// HTTP client for making requests.
    client: reqwest::Client,
}

impl FeishuAdapter {
    /// Create a new Feishu adapter with default configuration and no credentials.
    pub fn new(id: impl Into<String>) -> Self {
        let client = reqwest::Client::builder()
            .user_agent("OpenIntentOS/0.1")
            .build()
            .unwrap_or_default();

        Self {
            id: id.into(),
            connected: false,
            app_id: None,
            app_secret: None,
            tenant_access_token: None,
            base_url: DEFAULT_BASE_URL.to_string(),
            client,
        }
    }

    /// Create a new Feishu adapter with pre-configured app credentials.
    pub fn with_credentials(
        id: impl Into<String>,
        app_id: impl Into<String>,
        app_secret: impl Into<String>,
    ) -> Self {
        let mut adapter = Self::new(id);
        adapter.app_id = Some(app_id.into());
        adapter.app_secret = Some(app_secret.into());
        adapter
    }

    // -----------------------------------------------------------------------
    // URL construction
    // -----------------------------------------------------------------------

    /// Build a full API URL from a path segment.
    fn api_url(&self, path: &str) -> String {
        format!("{}{}", self.base_url, path)
    }

    // -----------------------------------------------------------------------
    // Token management
    // -----------------------------------------------------------------------

    /// Build the JSON request body for obtaining a tenant access token.
    pub fn build_token_request_body(app_id: &str, app_secret: &str) -> Value {
        json!({
            "app_id": app_id,
            "app_secret": app_secret
        })
    }

    /// Resolve the tenant access token, returning an error if none is available.
    fn resolve_token(&self) -> Result<String> {
        self.tenant_access_token
            .clone()
            .ok_or_else(|| AdapterError::AuthRequired {
                adapter_id: self.id.clone(),
                provider: "feishu".to_string(),
            })
    }

    /// Request a new tenant access token from the Feishu API.
    async fn fetch_tenant_access_token(&self) -> Result<String> {
        let app_id = self
            .app_id
            .as_deref()
            .ok_or_else(|| AdapterError::AuthRequired {
                adapter_id: self.id.clone(),
                provider: "feishu".to_string(),
            })?;
        let app_secret = self
            .app_secret
            .as_deref()
            .ok_or_else(|| AdapterError::AuthRequired {
                adapter_id: self.id.clone(),
                provider: "feishu".to_string(),
            })?;

        let url = self.api_url("/auth/v3/tenant_access_token/internal");
        let body = Self::build_token_request_body(app_id, app_secret);

        debug!(url = %url, "requesting tenant access token");

        let response = self
            .client
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

        Self::parse_feishu_response(&json_resp, "auth")?;

        json_resp
            .get("tenant_access_token")
            .and_then(|v| v.as_str())
            .map(|s| s.to_string())
            .ok_or_else(|| AdapterError::ExecutionFailed {
                tool_name: "auth".into(),
                reason: "tenant_access_token not found in response".into(),
            })
    }

    // -----------------------------------------------------------------------
    // Response parsing
    // -----------------------------------------------------------------------

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

    // -----------------------------------------------------------------------
    // HTTP helpers
    // -----------------------------------------------------------------------

    /// Build a GET request with Feishu authorization headers.
    fn get_request(&self, url: &str, token: &str) -> reqwest::RequestBuilder {
        self.client
            .get(url)
            .header("Authorization", format!("Bearer {token}"))
            .header("Content-Type", "application/json; charset=utf-8")
    }

    /// Build a POST request with Feishu authorization headers.
    fn post_request(&self, url: &str, token: &str) -> reqwest::RequestBuilder {
        self.client
            .post(url)
            .header("Authorization", format!("Bearer {token}"))
            .header("Content-Type", "application/json; charset=utf-8")
    }

    // -----------------------------------------------------------------------
    // Message format helpers
    // -----------------------------------------------------------------------

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

    // -----------------------------------------------------------------------
    // Tool implementations
    // -----------------------------------------------------------------------

    /// Send a message to a user or group chat.
    async fn tool_send_message(&self, params: Value) -> Result<Value> {
        let token = self.resolve_token()?;

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

        let url = Self::build_send_message_url(&self.base_url, receive_id_type);
        let body = Self::build_message_body(receive_id, msg_type, content);

        debug!(url = %url, receive_id = %receive_id, msg_type = %msg_type, "sending Feishu message");

        let response = self
            .post_request(&url, &token)
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

        Self::parse_feishu_response(&json_resp, "feishu_send_message")?;

        Ok(json!({
            "success": true,
            "data": json_resp.get("data").cloned().unwrap_or(json!({})),
        }))
    }

    /// List available group chats.
    async fn tool_list_chats(&self, params: Value) -> Result<Value> {
        let token = self.resolve_token()?;

        let page_size = params
            .get("page_size")
            .and_then(|v| v.as_u64())
            .unwrap_or(20);
        let page_token = params
            .get("page_token")
            .and_then(|v| v.as_str())
            .unwrap_or("");

        let mut url = format!("{}/im/v1/chats?page_size={}", self.base_url, page_size);
        if !page_token.is_empty() {
            url.push_str(&format!("&page_token={}", page_token));
        }

        debug!(url = %url, "listing Feishu chats");

        let response = self.get_request(&url, &token).send().await.map_err(|e| {
            AdapterError::ExecutionFailed {
                tool_name: "feishu_list_chats".into(),
                reason: format!("failed to list chats: {e}"),
            }
        })?;

        let json_resp: Value =
            response
                .json()
                .await
                .map_err(|e| AdapterError::ExecutionFailed {
                    tool_name: "feishu_list_chats".into(),
                    reason: format!("failed to parse response: {e}"),
                })?;

        Self::parse_feishu_response(&json_resp, "feishu_list_chats")?;

        Ok(json!({
            "success": true,
            "data": json_resp.get("data").cloned().unwrap_or(json!({})),
        }))
    }

    /// Get recent messages from a chat.
    async fn tool_get_chat_messages(&self, params: Value) -> Result<Value> {
        let token = self.resolve_token()?;

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
            self.base_url, container_id, page_size
        );

        debug!(url = %url, container_id = %container_id, "getting Feishu chat messages");

        let response = self.get_request(&url, &token).send().await.map_err(|e| {
            AdapterError::ExecutionFailed {
                tool_name: "feishu_get_chat_messages".into(),
                reason: format!("failed to get chat messages: {e}"),
            }
        })?;

        let json_resp: Value =
            response
                .json()
                .await
                .map_err(|e| AdapterError::ExecutionFailed {
                    tool_name: "feishu_get_chat_messages".into(),
                    reason: format!("failed to parse response: {e}"),
                })?;

        Self::parse_feishu_response(&json_resp, "feishu_get_chat_messages")?;

        Ok(json!({
            "success": true,
            "data": json_resp.get("data").cloned().unwrap_or(json!({})),
        }))
    }

    /// Create a document in Feishu Docs.
    async fn tool_create_doc(&self, params: Value) -> Result<Value> {
        let token = self.resolve_token()?;

        let title = params
            .get("title")
            .and_then(|v| v.as_str())
            .ok_or_else(|| AdapterError::InvalidParams {
                tool_name: "feishu_create_doc".into(),
                reason: "missing required string field `title`".into(),
            })?;

        let folder_token = params.get("folder_token").and_then(|v| v.as_str());

        let url = self.api_url("/docx/v1/documents");

        let mut body = json!({ "title": title });
        if let Some(ft) = folder_token {
            body["folder_token"] = json!(ft);
        }

        debug!(url = %url, title = %title, "creating Feishu document");

        let response = self
            .post_request(&url, &token)
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

        Self::parse_feishu_response(&json_resp, "feishu_create_doc")?;

        Ok(json!({
            "success": true,
            "data": json_resp.get("data").cloned().unwrap_or(json!({})),
        }))
    }

    /// Search for users by name or email.
    async fn tool_search_users(&self, params: Value) -> Result<Value> {
        let token = self.resolve_token()?;

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

        let url = self.api_url("/search/v1/user");

        let body = json!({
            "query": query,
            "page_size": page_size
        });

        debug!(url = %url, query = %query, "searching Feishu users");

        let response = self
            .post_request(&url, &token)
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

        Self::parse_feishu_response(&json_resp, "feishu_search_users")?;

        Ok(json!({
            "success": true,
            "data": json_resp.get("data").cloned().unwrap_or(json!({})),
        }))
    }

    /// Get user details by user ID.
    async fn tool_get_user_info(&self, params: Value) -> Result<Value> {
        let token = self.resolve_token()?;

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
            self.base_url, user_id, user_id_type
        );

        debug!(url = %url, user_id = %user_id, "getting Feishu user info");

        let response = self.get_request(&url, &token).send().await.map_err(|e| {
            AdapterError::ExecutionFailed {
                tool_name: "feishu_get_user_info".into(),
                reason: format!("failed to get user info: {e}"),
            }
        })?;

        let json_resp: Value =
            response
                .json()
                .await
                .map_err(|e| AdapterError::ExecutionFailed {
                    tool_name: "feishu_get_user_info".into(),
                    reason: format!("failed to parse response: {e}"),
                })?;

        Self::parse_feishu_response(&json_resp, "feishu_get_user_info")?;

        Ok(json!({
            "success": true,
            "data": json_resp.get("data").cloned().unwrap_or(json!({})),
        }))
    }
}

// ---------------------------------------------------------------------------
// Adapter trait implementation
// ---------------------------------------------------------------------------

#[async_trait]
impl Adapter for FeishuAdapter {
    fn id(&self) -> &str {
        &self.id
    }

    fn adapter_type(&self) -> AdapterType {
        AdapterType::Messaging
    }

    async fn connect(&mut self) -> Result<()> {
        if self.app_id.is_some() && self.app_secret.is_some() {
            match self.fetch_tenant_access_token().await {
                Ok(token) => {
                    info!(id = %self.id, "Feishu adapter connected with tenant access token");
                    self.tenant_access_token = Some(token);
                }
                Err(e) => {
                    warn!(id = %self.id, error = %e, "failed to fetch tenant access token, connecting without auth");
                }
            }
        } else {
            info!(id = %self.id, "Feishu adapter connected without credentials");
        }
        self.connected = true;
        Ok(())
    }

    async fn disconnect(&mut self) -> Result<()> {
        info!(id = %self.id, "Feishu adapter disconnected");
        self.tenant_access_token = None;
        self.connected = false;
        Ok(())
    }

    async fn health_check(&self) -> Result<HealthStatus> {
        if !self.connected {
            return Ok(HealthStatus::Unhealthy);
        }
        if self.tenant_access_token.is_some() {
            Ok(HealthStatus::Healthy)
        } else {
            Ok(HealthStatus::Degraded)
        }
    }

    fn tools(&self) -> Vec<ToolDefinition> {
        vec![
            ToolDefinition {
                name: "feishu_send_message".into(),
                description: "Send a text or interactive message to a Feishu user or group chat"
                    .into(),
                parameters: json!({
                    "type": "object",
                    "properties": {
                        "receive_id": {
                            "type": "string",
                            "description": "The ID of the message recipient (user or chat)"
                        },
                        "receive_id_type": {
                            "type": "string",
                            "description": "Type of receive_id: open_id, user_id, or chat_id",
                            "enum": ["open_id", "user_id", "chat_id"]
                        },
                        "msg_type": {
                            "type": "string",
                            "description": "Message type: text or interactive",
                            "enum": ["text", "interactive"]
                        },
                        "content": {
                            "type": "string",
                            "description": "Message content as JSON string"
                        }
                    },
                    "required": ["receive_id", "receive_id_type", "msg_type", "content"]
                }),
            },
            ToolDefinition {
                name: "feishu_list_chats".into(),
                description: "List available group chats the bot has joined".into(),
                parameters: json!({
                    "type": "object",
                    "properties": {
                        "page_size": {
                            "type": "integer",
                            "description": "Number of chats per page (default: 20)"
                        },
                        "page_token": {
                            "type": "string",
                            "description": "Pagination token for the next page"
                        }
                    },
                    "required": []
                }),
            },
            ToolDefinition {
                name: "feishu_get_chat_messages".into(),
                description: "Get recent messages from a Feishu group chat".into(),
                parameters: json!({
                    "type": "object",
                    "properties": {
                        "container_id": {
                            "type": "string",
                            "description": "The chat ID to retrieve messages from"
                        },
                        "page_size": {
                            "type": "integer",
                            "description": "Number of messages to retrieve (default: 20)"
                        }
                    },
                    "required": ["container_id"]
                }),
            },
            ToolDefinition {
                name: "feishu_create_doc".into(),
                description: "Create a new document in Feishu Docs".into(),
                parameters: json!({
                    "type": "object",
                    "properties": {
                        "title": {
                            "type": "string",
                            "description": "Title of the document"
                        },
                        "folder_token": {
                            "type": "string",
                            "description": "Optional folder token to create the document in"
                        }
                    },
                    "required": ["title"]
                }),
            },
            ToolDefinition {
                name: "feishu_search_users".into(),
                description: "Search for Feishu users by name or email".into(),
                parameters: json!({
                    "type": "object",
                    "properties": {
                        "query": {
                            "type": "string",
                            "description": "Search query (name, email, etc.)"
                        },
                        "page_size": {
                            "type": "integer",
                            "description": "Number of results per page (default: 20)"
                        }
                    },
                    "required": ["query"]
                }),
            },
            ToolDefinition {
                name: "feishu_get_user_info".into(),
                description: "Get detailed information about a Feishu user".into(),
                parameters: json!({
                    "type": "object",
                    "properties": {
                        "user_id": {
                            "type": "string",
                            "description": "The user ID to look up"
                        },
                        "user_id_type": {
                            "type": "string",
                            "description": "Type of user_id: open_id or user_id (default: open_id)",
                            "enum": ["open_id", "user_id"]
                        }
                    },
                    "required": ["user_id"]
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
            "feishu_send_message" => self.tool_send_message(params).await,
            "feishu_list_chats" => self.tool_list_chats(params).await,
            "feishu_get_chat_messages" => self.tool_get_chat_messages(params).await,
            "feishu_create_doc" => self.tool_create_doc(params).await,
            "feishu_search_users" => self.tool_search_users(params).await,
            "feishu_get_user_info" => self.tool_get_user_info(params).await,
            _ => Err(AdapterError::ToolNotFound {
                adapter_id: self.id.clone(),
                tool_name: name.to_string(),
            }),
        }
    }

    fn required_auth(&self) -> Option<AuthRequirement> {
        Some(AuthRequirement {
            provider: "feishu".into(),
            scopes: vec![
                "im:message".into(),
                "im:chat".into(),
                "contact:user.base".into(),
                "docx:document".into(),
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

    // -- Construction tests --

    #[test]
    fn new_creates_adapter_with_defaults() {
        let adapter = FeishuAdapter::new("feishu-test");
        assert_eq!(adapter.id, "feishu-test");
        assert!(!adapter.connected);
        assert!(adapter.app_id.is_none());
        assert!(adapter.app_secret.is_none());
        assert!(adapter.tenant_access_token.is_none());
        assert_eq!(adapter.base_url, DEFAULT_BASE_URL);
    }

    #[test]
    fn with_credentials_sets_app_id_and_secret() {
        let adapter = FeishuAdapter::with_credentials("feishu-test", "my_app_id", "my_app_secret");
        assert_eq!(adapter.id, "feishu-test");
        assert_eq!(adapter.app_id.as_deref(), Some("my_app_id"));
        assert_eq!(adapter.app_secret.as_deref(), Some("my_app_secret"));
        assert!(adapter.tenant_access_token.is_none());
        assert_eq!(adapter.base_url, DEFAULT_BASE_URL);
    }

    // -- Adapter trait basics --

    #[test]
    fn adapter_id_returns_id() {
        let adapter = FeishuAdapter::new("my-feishu");
        assert_eq!(adapter.id(), "my-feishu");
    }

    #[test]
    fn adapter_type_is_messaging() {
        let adapter = FeishuAdapter::new("feishu");
        assert_eq!(adapter.adapter_type(), AdapterType::Messaging);
    }

    #[test]
    fn required_auth_returns_feishu_scopes() {
        let adapter = FeishuAdapter::new("feishu");
        let auth = adapter.required_auth().expect("should require auth");
        assert_eq!(auth.provider, "feishu");
        assert!(auth.scopes.contains(&"im:message".to_string()));
        assert!(auth.scopes.contains(&"im:chat".to_string()));
        assert!(auth.scopes.contains(&"contact:user.base".to_string()));
        assert!(auth.scopes.contains(&"docx:document".to_string()));
    }

    // -- Tool definitions --

    #[test]
    fn tools_returns_exactly_six() {
        let adapter = FeishuAdapter::new("feishu");
        let tools = adapter.tools();
        assert_eq!(tools.len(), 6);
    }

    #[test]
    fn tools_have_expected_names() {
        let adapter = FeishuAdapter::new("feishu");
        let names: Vec<String> = adapter.tools().iter().map(|t| t.name.clone()).collect();
        let expected = vec![
            "feishu_send_message",
            "feishu_list_chats",
            "feishu_get_chat_messages",
            "feishu_create_doc",
            "feishu_search_users",
            "feishu_get_user_info",
        ];
        assert_eq!(names, expected);
    }

    #[test]
    fn tool_send_message_has_required_fields() {
        let adapter = FeishuAdapter::new("feishu");
        let tools = adapter.tools();
        let send_msg = tools
            .iter()
            .find(|t| t.name == "feishu_send_message")
            .expect("should have feishu_send_message");
        let required = send_msg.parameters["required"]
            .as_array()
            .expect("required should be an array");
        assert!(required.contains(&json!("receive_id")));
        assert!(required.contains(&json!("receive_id_type")));
        assert!(required.contains(&json!("msg_type")));
        assert!(required.contains(&json!("content")));
    }

    #[test]
    fn tool_list_chats_has_no_required_fields() {
        let adapter = FeishuAdapter::new("feishu");
        let tools = adapter.tools();
        let list_chats = tools
            .iter()
            .find(|t| t.name == "feishu_list_chats")
            .expect("should have feishu_list_chats");
        let required = list_chats.parameters["required"]
            .as_array()
            .expect("required should be an array");
        assert!(required.is_empty());
    }

    // -- Connect / disconnect --

    #[tokio::test]
    async fn connect_succeeds_without_credentials() {
        let mut adapter = FeishuAdapter::new("feishu");
        let result = adapter.connect().await;
        assert!(result.is_ok());
        assert!(adapter.connected);
    }

    #[tokio::test]
    async fn disconnect_clears_token_and_sets_disconnected() {
        let mut adapter = FeishuAdapter::new("feishu");
        adapter.connected = true;
        adapter.tenant_access_token = Some("test-token".into());
        adapter.disconnect().await.unwrap();
        assert!(!adapter.connected);
        assert!(adapter.tenant_access_token.is_none());
    }

    // -- Health check --

    #[tokio::test]
    async fn health_check_returns_unhealthy_when_disconnected() {
        let adapter = FeishuAdapter::new("feishu");
        let status = adapter.health_check().await.unwrap();
        assert_eq!(status, HealthStatus::Unhealthy);
    }

    #[tokio::test]
    async fn health_check_returns_degraded_when_connected_without_token() {
        let mut adapter = FeishuAdapter::new("feishu");
        adapter.connected = true;
        let status = adapter.health_check().await.unwrap();
        assert_eq!(status, HealthStatus::Degraded);
    }

    #[tokio::test]
    async fn health_check_returns_healthy_when_connected_with_token() {
        let mut adapter = FeishuAdapter::new("feishu");
        adapter.connected = true;
        adapter.tenant_access_token = Some("valid-token".into());
        let status = adapter.health_check().await.unwrap();
        assert_eq!(status, HealthStatus::Healthy);
    }

    // -- Token resolution --

    #[test]
    fn resolve_token_succeeds_with_token() {
        let mut adapter = FeishuAdapter::new("feishu");
        adapter.tenant_access_token = Some("my-token".into());
        let token = adapter.resolve_token().unwrap();
        assert_eq!(token, "my-token");
    }

    #[test]
    fn resolve_token_fails_without_token() {
        let adapter = FeishuAdapter::new("feishu");
        let result = adapter.resolve_token();
        assert!(result.is_err());
    }

    // -- Token request body building --

    #[test]
    fn build_token_request_body_contains_credentials() {
        let body = FeishuAdapter::build_token_request_body("app123", "secret456");
        assert_eq!(body["app_id"], "app123");
        assert_eq!(body["app_secret"], "secret456");
    }

    // -- Message format building --

    #[test]
    fn build_message_body_has_correct_fields() {
        let body = FeishuAdapter::build_message_body("ou_abc123", "text", r#"{"text":"hello"}"#);
        assert_eq!(body["receive_id"], "ou_abc123");
        assert_eq!(body["msg_type"], "text");
        assert_eq!(body["content"], r#"{"text":"hello"}"#);
    }

    // -- API URL construction --

    #[test]
    fn api_url_constructs_correct_urls() {
        let adapter = FeishuAdapter::new("feishu");
        assert_eq!(
            adapter.api_url("/auth/v3/tenant_access_token/internal"),
            "https://open.feishu.cn/open-apis/auth/v3/tenant_access_token/internal"
        );
        assert_eq!(
            adapter.api_url("/im/v1/messages"),
            "https://open.feishu.cn/open-apis/im/v1/messages"
        );
    }

    #[test]
    fn build_send_message_url_includes_receive_id_type() {
        let url = FeishuAdapter::build_send_message_url(DEFAULT_BASE_URL, "open_id");
        assert_eq!(
            url,
            "https://open.feishu.cn/open-apis/im/v1/messages?receive_id_type=open_id"
        );
        let url2 = FeishuAdapter::build_send_message_url(DEFAULT_BASE_URL, "chat_id");
        assert!(url2.contains("receive_id_type=chat_id"));
    }

    // -- Response parsing --

    #[test]
    fn parse_feishu_response_succeeds_on_code_zero() {
        let resp = json!({ "code": 0, "msg": "success", "data": {} });
        let result = FeishuAdapter::parse_feishu_response(&resp, "test_tool");
        assert!(result.is_ok());
    }

    #[test]
    fn parse_feishu_response_fails_on_nonzero_code() {
        let resp = json!({ "code": 99991, "msg": "invalid token" });
        let result = FeishuAdapter::parse_feishu_response(&resp, "test_tool");
        assert!(result.is_err());
        let err_msg = result.unwrap_err().to_string();
        assert!(err_msg.contains("99991"));
        assert!(err_msg.contains("invalid token"));
    }

    // -- Execute tool when not connected --

    #[tokio::test]
    async fn execute_tool_rejects_when_not_connected() {
        let adapter = FeishuAdapter::with_credentials("feishu", "app", "secret");
        let result = adapter.execute_tool("feishu_send_message", json!({})).await;
        assert!(result.is_err());
        let err = result.unwrap_err();
        assert!(err.to_string().contains("not connected"));
    }

    // -- Execute tool rejects unknown tool --

    #[tokio::test]
    async fn execute_tool_rejects_unknown_tool() {
        let mut adapter = FeishuAdapter::new("feishu");
        adapter.connected = true;
        let result = adapter.execute_tool("nonexistent_tool", json!({})).await;
        assert!(result.is_err());
        let err = result.unwrap_err();
        assert!(err.to_string().contains("tool not found"));
    }

    // -- Missing required parameters --

    #[tokio::test]
    async fn send_message_rejects_missing_receive_id() {
        let mut adapter = FeishuAdapter::new("feishu");
        adapter.connected = true;
        adapter.tenant_access_token = Some("token".into());
        let result = adapter
            .execute_tool(
                "feishu_send_message",
                json!({
                    "receive_id_type": "open_id",
                    "msg_type": "text",
                    "content": "hello"
                }),
            )
            .await;
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("receive_id"));
    }

    #[tokio::test]
    async fn get_chat_messages_rejects_missing_container_id() {
        let mut adapter = FeishuAdapter::new("feishu");
        adapter.connected = true;
        adapter.tenant_access_token = Some("token".into());
        let result = adapter
            .execute_tool("feishu_get_chat_messages", json!({}))
            .await;
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("container_id"));
    }

    #[tokio::test]
    async fn create_doc_rejects_missing_title() {
        let mut adapter = FeishuAdapter::new("feishu");
        adapter.connected = true;
        adapter.tenant_access_token = Some("token".into());
        let result = adapter.execute_tool("feishu_create_doc", json!({})).await;
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("title"));
    }

    #[tokio::test]
    async fn search_users_rejects_missing_query() {
        let mut adapter = FeishuAdapter::new("feishu");
        adapter.connected = true;
        adapter.tenant_access_token = Some("token".into());
        let result = adapter.execute_tool("feishu_search_users", json!({})).await;
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("query"));
    }

    #[tokio::test]
    async fn get_user_info_rejects_missing_user_id() {
        let mut adapter = FeishuAdapter::new("feishu");
        adapter.connected = true;
        adapter.tenant_access_token = Some("token".into());
        let result = adapter
            .execute_tool("feishu_get_user_info", json!({}))
            .await;
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("user_id"));
    }
}
