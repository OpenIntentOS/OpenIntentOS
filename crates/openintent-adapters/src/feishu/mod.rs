//! Feishu (Lark) API adapter for OpenIntentOS.
//!
//! Provides tools for interacting with the Feishu enterprise messenger by
//! ByteDance.  Supports sending messages, listing chats, retrieving messages,
//! creating documents, and searching users via the Feishu Open Platform REST
//! API.

pub mod api;
pub mod tools;
pub mod types;

use async_trait::async_trait;
use serde_json::Value;
use tracing::{info, warn};

use crate::error::{AdapterError, Result};
use crate::traits::{Adapter, AdapterType, AuthRequirement, HealthStatus, ToolDefinition};

use api::{
    fetch_tenant_access_token, tool_create_doc, tool_get_chat_messages, tool_get_user_info,
    tool_list_chats, tool_search_users, tool_send_message,
};

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

    /// Build a full API URL from a path segment.
    fn api_url(&self, path: &str) -> String {
        format!("{}{}", self.base_url, path)
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
}

#[async_trait]
impl Adapter for FeishuAdapter {
    fn id(&self) -> &str {
        &self.id
    }

    fn adapter_type(&self) -> AdapterType {
        AdapterType::Messaging
    }

    async fn connect(&mut self) -> Result<()> {
        if let (Some(app_id), Some(app_secret)) =
            (self.app_id.as_deref(), self.app_secret.as_deref())
        {
            match fetch_tenant_access_token(
                &self.client,
                &self.base_url,
                &self.id,
                app_id,
                app_secret,
            )
            .await
            {
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
        tools::build_tool_definitions()
    }

    async fn execute_tool(&self, name: &str, params: Value) -> Result<Value> {
        if !self.connected {
            return Err(AdapterError::ExecutionFailed {
                tool_name: name.to_string(),
                reason: format!("adapter `{}` is not connected", self.id),
            });
        }

        let token = self.resolve_token()?;

        match name {
            "feishu_send_message" => {
                tool_send_message(&self.client, &self.base_url, &token, &params).await
            }
            "feishu_list_chats" => {
                tool_list_chats(&self.client, &self.base_url, &token, &params).await
            }
            "feishu_get_chat_messages" => {
                tool_get_chat_messages(&self.client, &self.base_url, &token, &params).await
            }
            "feishu_create_doc" => {
                tool_create_doc(&self.client, &self.base_url, &token, &params).await
            }
            "feishu_search_users" => {
                tool_search_users(&self.client, &self.base_url, &token, &params).await
            }
            "feishu_get_user_info" => {
                tool_get_user_info(&self.client, &self.base_url, &token, &params).await
            }
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
    use api::{build_message_body, build_send_message_url, build_token_request_body, parse_feishu_response};
    use serde_json::json;

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

    #[test]
    fn build_token_request_body_contains_credentials() {
        let body = build_token_request_body("app123", "secret456");
        assert_eq!(body["app_id"], "app123");
        assert_eq!(body["app_secret"], "secret456");
    }

    #[test]
    fn build_message_body_has_correct_fields() {
        let body = build_message_body("ou_abc123", "text", r#"{"text":"hello"}"#);
        assert_eq!(body["receive_id"], "ou_abc123");
        assert_eq!(body["msg_type"], "text");
        assert_eq!(body["content"], r#"{"text":"hello"}"#);
    }

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
        let url = build_send_message_url(DEFAULT_BASE_URL, "open_id");
        assert_eq!(
            url,
            "https://open.feishu.cn/open-apis/im/v1/messages?receive_id_type=open_id"
        );
        let url2 = build_send_message_url(DEFAULT_BASE_URL, "chat_id");
        assert!(url2.contains("receive_id_type=chat_id"));
    }

    #[test]
    fn parse_feishu_response_succeeds_on_code_zero() {
        let resp = json!({ "code": 0, "msg": "success", "data": {} });
        let result = parse_feishu_response(&resp, "test_tool");
        assert!(result.is_ok());
    }

    #[test]
    fn parse_feishu_response_fails_on_nonzero_code() {
        let resp = json!({ "code": 99991, "msg": "invalid token" });
        let result = parse_feishu_response(&resp, "test_tool");
        assert!(result.is_err());
        let err_msg = result.unwrap_err().to_string();
        assert!(err_msg.contains("99991"));
        assert!(err_msg.contains("invalid token"));
    }

    #[tokio::test]
    async fn execute_tool_rejects_when_not_connected() {
        let adapter = FeishuAdapter::with_credentials("feishu", "app", "secret");
        let result = adapter.execute_tool("feishu_send_message", json!({})).await;
        assert!(result.is_err());
        let err = result.unwrap_err();
        assert!(err.to_string().contains("not connected"));
    }

    #[tokio::test]
    async fn execute_tool_rejects_unknown_tool() {
        let mut adapter = FeishuAdapter::new("feishu");
        adapter.connected = true;
        let result = adapter.execute_tool("nonexistent_tool", json!({})).await;
        assert!(result.is_err());
        let err = result.unwrap_err();
        assert!(err.to_string().contains("tool not found"));
    }

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
