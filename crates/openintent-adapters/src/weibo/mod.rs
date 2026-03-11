//! Weibo adapter for OpenIntentOS.
//!
//! Integrates with the Sina Weibo API v2 to post statuses, read mentions,
//! and reply to comments.
//!
//! ## Authentication
//!
//! The adapter expects a pre-obtained OAuth2 access token:
//!
//! ```env
//! WEIBO_ACCESS_TOKEN=your_access_token
//! WEIBO_APP_KEY=your_app_key         # required for weibo_get_auth_url
//! WEIBO_APP_SECRET=your_app_secret   # required for OAuth code exchange
//! ```
//!
//! Use `weibo_get_auth_url` + `exchange_code` to obtain the initial token.
//!
//! Tools:
//!   - `weibo_get_auth_url`    — build the OAuth2 authorization URL
//!   - `weibo_post`            — post a status update (max 140 chars)
//!   - `weibo_get_mentions`    — get recent mentions
//!   - `weibo_reply_comment`   — reply to a Weibo post

pub mod oauth;
pub mod tools;
pub mod types;

use std::time::Duration;

use async_trait::async_trait;
use serde_json::Value;
use tracing::{info, warn};

use crate::error::{AdapterError, Result};
use crate::proxy;
use crate::traits::{Adapter, AdapterType, AuthRequirement, HealthStatus, ToolDefinition};

use tools::{
    build_tool_definitions, tool_get_auth_url, tool_get_mentions, tool_post, tool_reply_comment,
};

/// Weibo API base URL.
const WEIBO_API_BASE: &str = "https://api.weibo.com/2";

/// Weibo adapter.
pub struct WeiboAdapter {
    id: String,
    connected: bool,
    access_token: Option<String>,
    app_key: Option<String>,
    app_secret: Option<String>,
    pub(crate) client: reqwest::Client,
    pub(crate) api_base: String,
}

impl WeiboAdapter {
    /// Create a new Weibo adapter from environment variables.
    pub fn new(id: impl Into<String>) -> Self {
        let client = proxy::build_client(Duration::from_secs(30))
            .user_agent("OpenIntentOS/0.1")
            .build()
            .unwrap_or_default();

        Self {
            id: id.into(),
            connected: false,
            access_token: std::env::var("WEIBO_ACCESS_TOKEN").ok(),
            app_key: std::env::var("WEIBO_APP_KEY").ok(),
            app_secret: std::env::var("WEIBO_APP_SECRET").ok(),
            client,
            api_base: WEIBO_API_BASE.to_string(),
        }
    }

    /// Return the user access token, or error if not configured.
    fn token(&self) -> Result<&str> {
        self.access_token.as_deref().ok_or_else(|| AdapterError::AuthRequired {
            adapter_id: self.id.clone(),
            provider: "weibo".to_string(),
        })
    }
}

#[async_trait]
impl Adapter for WeiboAdapter {
    fn id(&self) -> &str {
        &self.id
    }

    fn adapter_type(&self) -> AdapterType {
        AdapterType::Messaging
    }

    async fn connect(&mut self) -> Result<()> {
        if self.access_token.is_none() {
            warn!(
                adapter = %self.id,
                "WEIBO_ACCESS_TOKEN not set — adapter inactive. Use weibo_get_auth_url to obtain a token."
            );
            return Ok(());
        }
        self.connected = true;
        info!(adapter = %self.id, "Weibo adapter connected");
        Ok(())
    }

    async fn disconnect(&mut self) -> Result<()> {
        self.connected = false;
        Ok(())
    }

    async fn health_check(&self) -> Result<HealthStatus> {
        if self.connected && self.access_token.is_some() {
            Ok(HealthStatus::Healthy)
        } else {
            Ok(HealthStatus::Unhealthy)
        }
    }

    fn tools(&self) -> Vec<ToolDefinition> {
        build_tool_definitions()
    }

    fn required_auth(&self) -> Option<AuthRequirement> {
        Some(AuthRequirement {
            provider: "weibo".into(),
            scopes: vec!["WEIBO_ACCESS_TOKEN".into()],
        })
    }

    async fn execute_tool(&self, name: &str, params: Value) -> Result<Value> {
        match name {
            "weibo_get_auth_url" => tool_get_auth_url(self.app_key.as_deref(), &params),
            "weibo_post" => {
                let token = self.token()?;
                tool_post(&self.client, &self.api_base, token, &params).await
            }
            "weibo_get_mentions" => {
                let token = self.token()?;
                tool_get_mentions(&self.client, &self.api_base, token, &params).await
            }
            "weibo_reply_comment" => {
                let token = self.token()?;
                tool_reply_comment(&self.client, &self.api_base, token, &params).await
            }
            _ => Err(AdapterError::ToolNotFound {
                tool_name: name.to_string(),
                adapter_id: self.id.clone(),
            }),
        }
    }
}
