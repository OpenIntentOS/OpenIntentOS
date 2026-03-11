//! DingTalk (钉钉) adapter for OpenIntentOS.
//!
//! Supports two integration modes:
//!
//! ## Mode 1: Outbound Webhook (简单模式)
//!
//! Send messages to a DingTalk group via a custom robot webhook URL.
//! No inbound support. Easiest to set up.
//!
//! ```env
//! DINGTALK_WEBHOOK_URL=https://oapi.dingtalk.com/robot/send?access_token=xxx
//! DINGTALK_WEBHOOK_SECRET=SECxxx   # optional signing secret
//! ```
//!
//! ## Mode 2: App Token Mode (企业内部应用)
//!
//! Full enterprise integration with access tokens.
//!
//! ```env
//! DINGTALK_APP_KEY=dingxxxxxxxxxx
//! DINGTALK_APP_SECRET=your_app_secret
//! ```
//!
//! Tools:
//!   - `dingtalk_send_text`     — send a text message to a group/user
//!   - `dingtalk_send_markdown` — send a markdown message to a group
//!   - `dingtalk_send_card`     — send an action card with buttons

pub mod api;
pub mod tools;
pub mod types;

use std::sync::Arc;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use async_trait::async_trait;
use serde_json::{Value, json};
use tokio::sync::RwLock;
use tracing::{debug, info, warn};

use crate::error::{AdapterError, Result};
use crate::proxy;
use crate::traits::{Adapter, AdapterType, AuthRequirement, HealthStatus, ToolDefinition};

use api::fetch_access_token;
use tools::{tool_send_card, tool_send_markdown, tool_send_text, tool_webhook_send};

/// DingTalk API base URL.
const DINGTALK_API_BASE: &str = "https://oapi.dingtalk.com";

/// Cached access token entry.
#[derive(Debug, Clone)]
struct CachedToken {
    token: String,
    expires_at: u64,
}

#[derive(Debug, PartialEq)]
enum DingTalkMode {
    App,
    Webhook,
    None,
}

/// DingTalk adapter supporting both webhook and app integration.
pub struct DingTalkAdapter {
    id: String,
    connected: bool,
    app_key: Option<String>,
    app_secret: Option<String>,
    webhook_url: Option<String>,
    webhook_secret: Option<String>,
    token_cache: Arc<RwLock<Option<CachedToken>>>,
    pub(crate) client: reqwest::Client,
    pub(crate) api_base: String,
}

impl DingTalkAdapter {
    /// Create a new DingTalk adapter from environment variables.
    pub fn new(id: impl Into<String>) -> Self {
        let client = proxy::build_client(Duration::from_secs(30))
            .user_agent("OpenIntentOS/0.1")
            .build()
            .unwrap_or_default();

        Self {
            id: id.into(),
            connected: false,
            app_key: std::env::var("DINGTALK_APP_KEY").ok(),
            app_secret: std::env::var("DINGTALK_APP_SECRET").ok(),
            webhook_url: std::env::var("DINGTALK_WEBHOOK_URL").ok(),
            webhook_secret: std::env::var("DINGTALK_WEBHOOK_SECRET").ok(),
            token_cache: Arc::new(RwLock::new(None)),
            client,
            api_base: DINGTALK_API_BASE.to_string(),
        }
    }

    fn mode(&self) -> DingTalkMode {
        if self.app_key.is_some() && self.app_secret.is_some() {
            DingTalkMode::App
        } else if self.webhook_url.is_some() {
            DingTalkMode::Webhook
        } else {
            DingTalkMode::None
        }
    }

    /// Get app access token, refreshing if expired.
    pub(crate) async fn access_token(&self) -> Result<String> {
        let key = self.app_key.as_deref().ok_or_else(|| AdapterError::AuthRequired {
            adapter_id: self.id.clone(),
            provider: "dingtalk".to_string(),
        })?;
        let secret = self.app_secret.as_deref().ok_or_else(|| AdapterError::AuthRequired {
            adapter_id: self.id.clone(),
            provider: "dingtalk".to_string(),
        })?;

        {
            let guard = self.token_cache.read().await;
            if let Some(ref cached) = *guard {
                let now = SystemTime::now()
                    .duration_since(UNIX_EPOCH)
                    .unwrap_or_default()
                    .as_secs();
                if cached.expires_at > now + 300 {
                    return Ok(cached.token.clone());
                }
            }
        }

        debug!("refreshing DingTalk access token");
        let resp = fetch_access_token(&self.client, &self.api_base, key, secret).await?;

        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs();

        *self.token_cache.write().await = Some(CachedToken {
            token: resp.access_token.clone(),
            expires_at: now + resp.expires_in,
        });

        info!("DingTalk access token refreshed, expires in {}s", resp.expires_in);
        Ok(resp.access_token)
    }
}

#[async_trait]
impl Adapter for DingTalkAdapter {
    fn id(&self) -> &str {
        &self.id
    }

    fn adapter_type(&self) -> AdapterType {
        AdapterType::Messaging
    }

    async fn connect(&mut self) -> Result<()> {
        match self.mode() {
            DingTalkMode::None => {
                warn!(
                    adapter = %self.id,
                    "No DingTalk credentials. Set DINGTALK_APP_KEY+SECRET or DINGTALK_WEBHOOK_URL"
                );
            }
            DingTalkMode::App => match self.access_token().await {
                Ok(_) => {
                    self.connected = true;
                    info!(adapter = %self.id, "DingTalk app adapter connected");
                }
                Err(e) => {
                    warn!(adapter = %self.id, error = %e, "DingTalk token fetch failed");
                }
            },
            DingTalkMode::Webhook => {
                self.connected = true;
                info!(adapter = %self.id, "DingTalk webhook adapter connected");
            }
        }
        Ok(())
    }

    async fn disconnect(&mut self) -> Result<()> {
        self.connected = false;
        Ok(())
    }

    async fn health_check(&self) -> Result<HealthStatus> {
        if !self.connected {
            return Ok(HealthStatus::Unhealthy);
        }
        match self.mode() {
            DingTalkMode::App => match self.access_token().await {
                Ok(_) => Ok(HealthStatus::Healthy),
                Err(_) => Ok(HealthStatus::Degraded),
            },
            DingTalkMode::Webhook => Ok(HealthStatus::Healthy),
            DingTalkMode::None => Ok(HealthStatus::Unhealthy),
        }
    }

    fn tools(&self) -> Vec<ToolDefinition> {
        vec![
            ToolDefinition {
                name: "dingtalk_send_text".to_string(),
                description: "Send a text message to a DingTalk group via robot webhook. Use @all to notify everyone.".to_string(),
                parameters: json!({
                    "type": "object",
                    "properties": {
                        "content": { "type": "string", "description": "Message text" },
                        "at_all": { "type": "boolean", "description": "Notify all group members", "default": false },
                        "at_mobiles": {
                            "type": "array",
                            "items": { "type": "string" },
                            "description": "Phone numbers to @ in the message"
                        }
                    },
                    "required": ["content"]
                }),
            },
            ToolDefinition {
                name: "dingtalk_send_markdown".to_string(),
                description: "Send a Markdown message to a DingTalk group (supports bold, headers, links).".to_string(),
                parameters: json!({
                    "type": "object",
                    "properties": {
                        "title": { "type": "string", "description": "Notification preview title" },
                        "text": { "type": "string", "description": "Markdown content" },
                        "at_all": { "type": "boolean", "description": "Notify all", "default": false }
                    },
                    "required": ["title", "text"]
                }),
            },
            ToolDefinition {
                name: "dingtalk_send_card".to_string(),
                description: "Send an action card with clickable buttons to a DingTalk group.".to_string(),
                parameters: json!({
                    "type": "object",
                    "properties": {
                        "title": { "type": "string", "description": "Card title" },
                        "text": { "type": "string", "description": "Card body (Markdown)" },
                        "buttons": {
                            "type": "array",
                            "description": "Action buttons",
                            "items": {
                                "type": "object",
                                "properties": {
                                    "title": { "type": "string" },
                                    "action_url": { "type": "string" }
                                },
                                "required": ["title", "action_url"]
                            }
                        }
                    },
                    "required": ["title", "text", "buttons"]
                }),
            },
        ]
    }

    fn required_auth(&self) -> Option<AuthRequirement> {
        Some(AuthRequirement {
            provider: "dingtalk".to_string(),
            scopes: vec![
                "DINGTALK_WEBHOOK_URL".to_string(),
                "DINGTALK_APP_KEY".to_string(),
                "DINGTALK_APP_SECRET".to_string(),
            ],
        })
    }

    async fn execute_tool(&self, name: &str, params: Value) -> Result<Value> {
        match self.mode() {
            DingTalkMode::Webhook => {
                let webhook_url = self.webhook_url.as_deref().unwrap();
                let secret = self.webhook_secret.as_deref();
                tool_webhook_send(&self.client, webhook_url, secret, name, &params).await
            }
            DingTalkMode::App => {
                let token = self.access_token().await?;
                match name {
                    "dingtalk_send_text" => {
                        tool_send_text(&self.client, &self.api_base, &token, &params).await
                    }
                    "dingtalk_send_markdown" => {
                        tool_send_markdown(&self.client, &self.api_base, &token, &params).await
                    }
                    "dingtalk_send_card" => {
                        tool_send_card(&self.client, &self.api_base, &token, &params).await
                    }
                    _ => Err(AdapterError::ToolNotFound {
                        tool_name: name.to_string(),
                        adapter_id: self.id.clone(),
                    }),
                }
            }
            DingTalkMode::None => Err(AdapterError::AuthRequired {
                adapter_id: self.id.clone(),
                provider: "dingtalk".to_string(),
            }),
        }
    }
}
