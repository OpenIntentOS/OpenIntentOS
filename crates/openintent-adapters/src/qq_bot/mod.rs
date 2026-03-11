//! QQ Official Bot adapter for OpenIntentOS.
//!
//! Integrates with the QQ Open Platform bot API for sending channel and C2C
//! messages. Access tokens are obtained via AppID + AppSecret OAuth and are
//! automatically refreshed 5 minutes before expiry.
//!
//! ## Environment Variables
//!
//! ```env
//! QQ_BOT_APP_ID=12345678
//! QQ_BOT_APP_SECRET=your_app_secret
//! QQ_BOT_TOKEN=pre_issued_token   # optional: skip OAuth fetch
//! ```
//!
//! Tools:
//!   - `qq_send_group_message` — send text to a channel
//!   - `qq_send_c2c_message`   — send direct message to a user
//!   - `qq_send_image`         — send image to a channel

pub mod api;
pub mod tools;
pub mod types;
pub mod webhook;

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
use tools::{build_tool_definitions, tool_send_c2c_message, tool_send_group_message,
             tool_send_image_message};

/// Cached access token entry.
#[derive(Debug, Clone)]
struct CachedToken {
    token: String,
    expires_at: u64,
}

/// QQ Official Bot adapter.
pub struct QQBotAdapter {
    id: String,
    connected: bool,
    app_id: Option<String>,
    app_secret: Option<String>,
    /// Pre-issued static token (skips OAuth if set).
    static_token: Option<String>,
    token_cache: Arc<RwLock<Option<CachedToken>>>,
    pub(crate) client: reqwest::Client,
}

impl QQBotAdapter {
    /// Create a new QQ Bot adapter from environment variables.
    pub fn new(id: impl Into<String>) -> Self {
        let client = proxy::build_client(Duration::from_secs(30))
            .user_agent("OpenIntentOS/0.1")
            .build()
            .unwrap_or_default();

        Self {
            id: id.into(),
            connected: false,
            app_id: std::env::var("QQ_BOT_APP_ID").ok(),
            app_secret: std::env::var("QQ_BOT_APP_SECRET").ok(),
            static_token: std::env::var("QQ_BOT_TOKEN").ok(),
            token_cache: Arc::new(RwLock::new(None)),
            client,
        }
    }

    fn has_credentials(&self) -> bool {
        (self.app_id.is_some() && self.app_secret.is_some()) || self.static_token.is_some()
    }

    /// Get access token, refreshing if expired.
    ///
    /// If a static token is configured, it is returned directly without
    /// performing any OAuth flow.
    pub(crate) async fn access_token(&self) -> Result<String> {
        // Static token takes precedence.
        if let Some(ref tok) = self.static_token {
            return Ok(tok.clone());
        }

        let app_id =
            self.app_id.as_deref().ok_or_else(|| AdapterError::AuthRequired {
                adapter_id: self.id.clone(),
                provider: "qq_bot".to_string(),
            })?;
        let app_secret =
            self.app_secret.as_deref().ok_or_else(|| AdapterError::AuthRequired {
                adapter_id: self.id.clone(),
                provider: "qq_bot".to_string(),
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

        debug!("refreshing QQ Bot access token");
        let (token, expires_in) = fetch_access_token(&self.client, app_id, app_secret).await?;

        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs();

        *self.token_cache.write().await = Some(CachedToken {
            token: token.clone(),
            expires_at: now + expires_in,
        });

        info!("QQ Bot access token refreshed, expires in {}s", expires_in);
        Ok(token)
    }
}

#[async_trait]
impl Adapter for QQBotAdapter {
    fn id(&self) -> &str {
        &self.id
    }

    fn adapter_type(&self) -> AdapterType {
        AdapterType::Messaging
    }

    async fn connect(&mut self) -> Result<()> {
        if !self.has_credentials() {
            warn!(
                adapter = %self.id,
                "No QQ Bot credentials found. Set QQ_BOT_APP_ID+QQ_BOT_APP_SECRET or QQ_BOT_TOKEN"
            );
            return Ok(());
        }

        match self.access_token().await {
            Ok(_) => {
                self.connected = true;
                info!(adapter = %self.id, "QQ Bot adapter connected");
            }
            Err(e) => {
                warn!(adapter = %self.id, error = %e, "QQ Bot token fetch failed");
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
        match self.access_token().await {
            Ok(_) => Ok(HealthStatus::Healthy),
            Err(_) => Ok(HealthStatus::Degraded),
        }
    }

    fn tools(&self) -> Vec<ToolDefinition> {
        build_tool_definitions()
    }

    fn required_auth(&self) -> Option<AuthRequirement> {
        Some(AuthRequirement {
            provider: "qq_bot".to_string(),
            scopes: vec![
                "QQ_BOT_APP_ID".to_string(),
                "QQ_BOT_APP_SECRET".to_string(),
            ],
        })
    }

    async fn execute_tool(&self, name: &str, params: Value) -> Result<Value> {
        if !self.has_credentials() {
            return Err(AdapterError::AuthRequired {
                adapter_id: self.id.clone(),
                provider: "qq_bot".to_string(),
            });
        }

        let token = self.access_token().await?;
        let token_str = token.as_str();

        match name {
            "qq_send_group_message" => {
                tool_send_group_message(&self.client, token_str, &params).await
            }
            "qq_send_c2c_message" => {
                tool_send_c2c_message(&self.client, token_str, &params).await
            }
            "qq_send_image" => {
                tool_send_image_message(&self.client, token_str, &params).await
            }
            _ => Err(AdapterError::ToolNotFound {
                tool_name: name.to_string(),
                adapter_id: self.id.clone(),
            }),
        }
    }
}
