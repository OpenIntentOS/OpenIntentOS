//! WeCom (企业微信) adapter for OpenIntentOS.
//!
//! Supports two integration modes:
//!
//! ## Mode 1: Group Robot Webhook
//!
//! Send messages to a WeCom group via a custom robot webhook URL.
//!
//! ```env
//! WECOM_WEBHOOK_URL=https://qyapi.weixin.qq.com/cgi-bin/webhook/send?key=xxx
//! WECOM_WEBHOOK_KEY=xxx   # optional signing key
//! ```
//!
//! ## Mode 2: Internal App (access_token)
//!
//! Full enterprise integration with corp access tokens.
//!
//! ```env
//! WECOM_CORP_ID=ww1234567890abcdef
//! WECOM_AGENT_ID=1000002
//! WECOM_CORP_SECRET=your_corp_secret
//! ```
//!
//! Mode detection: if WECOM_CORP_ID + WECOM_CORP_SECRET are set → App mode.
//! Otherwise, if WECOM_WEBHOOK_URL is set → Webhook mode.
//!
//! Tools:
//!   - `wecom_send_text`     — send a plain text message
//!   - `wecom_send_markdown` — send a Markdown message
//!   - `wecom_send_file`     — send a text note about a file
//!   - `wecom_get_members`   — list department members (App mode only)

pub mod api;
pub mod tools;
pub mod types;

use std::sync::Arc;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use async_trait::async_trait;
use serde_json::Value;
use tokio::sync::RwLock;
use tracing::{debug, info, warn};

use crate::error::{AdapterError, Result};
use crate::proxy;
use crate::traits::{Adapter, AdapterType, AuthRequirement, HealthStatus, ToolDefinition};

use api::fetch_access_token;
use tools::{WeComMode, build_tool_definitions, tool_get_members, tool_send_file,
             tool_send_markdown, tool_send_text};

/// WeCom API base URL.
const WECOM_API_BASE: &str = "https://qyapi.weixin.qq.com";

/// Cached access token entry.
#[derive(Debug, Clone)]
struct CachedToken {
    token: String,
    expires_at: u64,
}

#[derive(Debug, PartialEq)]
enum WeComConnectMode {
    App,
    Webhook,
    None,
}

/// WeCom adapter supporting both webhook robot and internal app integration.
pub struct WeComAdapter {
    id: String,
    connected: bool,
    corp_id: Option<String>,
    corp_secret: Option<String>,
    agent_id: Option<u64>,
    webhook_url: Option<String>,
    #[allow(dead_code)]
    webhook_key: Option<String>,
    token_cache: Arc<RwLock<Option<CachedToken>>>,
    pub(crate) client: reqwest::Client,
    pub(crate) api_base: String,
}

impl WeComAdapter {
    /// Create a new WeCom adapter from environment variables.
    pub fn new(id: impl Into<String>) -> Self {
        let client = proxy::build_client(Duration::from_secs(30))
            .user_agent("OpenIntentOS/0.1")
            .build()
            .unwrap_or_default();

        let agent_id = std::env::var("WECOM_AGENT_ID")
            .ok()
            .and_then(|v| v.parse::<u64>().ok());

        Self {
            id: id.into(),
            connected: false,
            corp_id: std::env::var("WECOM_CORP_ID").ok(),
            corp_secret: std::env::var("WECOM_CORP_SECRET").ok(),
            agent_id,
            webhook_url: std::env::var("WECOM_WEBHOOK_URL").ok(),
            webhook_key: std::env::var("WECOM_WEBHOOK_KEY").ok(),
            token_cache: Arc::new(RwLock::new(None)),
            client,
            api_base: WECOM_API_BASE.to_string(),
        }
    }

    fn connect_mode(&self) -> WeComConnectMode {
        if self.corp_id.is_some() && self.corp_secret.is_some() {
            WeComConnectMode::App
        } else if self.webhook_url.is_some() {
            WeComConnectMode::Webhook
        } else {
            WeComConnectMode::None
        }
    }

    fn tool_mode(&self) -> WeComMode {
        match self.connect_mode() {
            WeComConnectMode::App => {
                WeComMode::App { agent_id: self.agent_id.unwrap_or(0) }
            }
            _ => WeComMode::Webhook,
        }
    }

    /// Get access token, refreshing if expired or missing.
    pub(crate) async fn access_token(&self) -> Result<String> {
        let corp_id =
            self.corp_id.as_deref().ok_or_else(|| AdapterError::AuthRequired {
                adapter_id: self.id.clone(),
                provider: "wecom".to_string(),
            })?;
        let corp_secret =
            self.corp_secret.as_deref().ok_or_else(|| AdapterError::AuthRequired {
                adapter_id: self.id.clone(),
                provider: "wecom".to_string(),
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

        debug!("refreshing WeCom access token");
        let (token, expires_in) =
            fetch_access_token(&self.client, corp_id, corp_secret).await?;

        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs();

        *self.token_cache.write().await = Some(CachedToken {
            token: token.clone(),
            expires_at: now + expires_in,
        });

        info!("WeCom access token refreshed, expires in {}s", expires_in);
        Ok(token)
    }
}

#[async_trait]
impl Adapter for WeComAdapter {
    fn id(&self) -> &str {
        &self.id
    }

    fn adapter_type(&self) -> AdapterType {
        AdapterType::Messaging
    }

    async fn connect(&mut self) -> Result<()> {
        match self.connect_mode() {
            WeComConnectMode::None => {
                warn!(
                    adapter = %self.id,
                    "No WeCom credentials found. Set WECOM_CORP_ID+WECOM_CORP_SECRET or WECOM_WEBHOOK_URL"
                );
            }
            WeComConnectMode::App => match self.access_token().await {
                Ok(_) => {
                    self.connected = true;
                    info!(adapter = %self.id, "WeCom app adapter connected");
                }
                Err(e) => {
                    warn!(adapter = %self.id, error = %e, "WeCom token fetch failed");
                }
            },
            WeComConnectMode::Webhook => {
                self.connected = true;
                info!(adapter = %self.id, "WeCom webhook adapter connected");
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
        match self.connect_mode() {
            WeComConnectMode::App => match self.access_token().await {
                Ok(_) => Ok(HealthStatus::Healthy),
                Err(_) => Ok(HealthStatus::Degraded),
            },
            WeComConnectMode::Webhook => Ok(HealthStatus::Healthy),
            WeComConnectMode::None => Ok(HealthStatus::Unhealthy),
        }
    }

    fn tools(&self) -> Vec<ToolDefinition> {
        build_tool_definitions()
    }

    fn required_auth(&self) -> Option<AuthRequirement> {
        Some(AuthRequirement {
            provider: "wecom".to_string(),
            scopes: vec![
                "WECOM_CORP_ID".to_string(),
                "WECOM_CORP_SECRET".to_string(),
            ],
        })
    }

    async fn execute_tool(&self, name: &str, params: Value) -> Result<Value> {
        let mode = self.tool_mode();

        match self.connect_mode() {
            WeComConnectMode::None => {
                return Err(AdapterError::AuthRequired {
                    adapter_id: self.id.clone(),
                    provider: "wecom".to_string(),
                });
            }
            WeComConnectMode::Webhook => {
                let webhook_url =
                    self.webhook_url.as_deref().ok_or_else(|| AdapterError::AuthRequired {
                        adapter_id: self.id.clone(),
                        provider: "wecom".to_string(),
                    })?;
                match name {
                    "wecom_send_text" => {
                        tool_send_text(&self.client, webhook_url, &mode, None, &params).await
                    }
                    "wecom_send_markdown" => {
                        tool_send_markdown(&self.client, webhook_url, &mode, None, &params).await
                    }
                    "wecom_send_file" => {
                        tool_send_file(&self.client, webhook_url, &mode, None, &params).await
                    }
                    "wecom_get_members" => Err(AdapterError::ExecutionFailed {
                        tool_name: name.to_string(),
                        reason: "wecom_get_members requires App mode credentials".to_string(),
                    }),
                    _ => Err(AdapterError::ToolNotFound {
                        tool_name: name.to_string(),
                        adapter_id: self.id.clone(),
                    }),
                }
            }
            WeComConnectMode::App => {
                let token = self.access_token().await?;
                let token_str = token.as_str();
                match name {
                    "wecom_send_text" => {
                        tool_send_text(
                            &self.client,
                            &self.api_base,
                            &mode,
                            Some(token_str),
                            &params,
                        )
                        .await
                    }
                    "wecom_send_markdown" => {
                        tool_send_markdown(
                            &self.client,
                            &self.api_base,
                            &mode,
                            Some(token_str),
                            &params,
                        )
                        .await
                    }
                    "wecom_send_file" => {
                        tool_send_file(
                            &self.client,
                            &self.api_base,
                            &mode,
                            Some(token_str),
                            &params,
                        )
                        .await
                    }
                    "wecom_get_members" => {
                        tool_get_members(&self.client, &self.api_base, token_str, &params).await
                    }
                    _ => Err(AdapterError::ToolNotFound {
                        tool_name: name.to_string(),
                        adapter_id: self.id.clone(),
                    }),
                }
            }
        }
    }
}
