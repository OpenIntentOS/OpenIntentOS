//! Bilibili Open Live Platform adapter for OpenIntentOS.
//!
//! Integrates with the Bilibili Open Live Platform for live room management
//! and danmaku (弹幕) interaction. All requests are authenticated using
//! HMAC-SHA256 signed headers.
//!
//! ## Environment Variables
//!
//! ```env
//! BILI_ACCESS_KEY_ID=your_access_key_id
//! BILI_ACCESS_KEY_SECRET=your_access_key_secret
//! BILI_APP_ID=your_app_id
//! ```
//!
//! Tools:
//!   - `bili_get_live_info`       — get live room status and info
//!   - `bili_send_danmaku`        — send danmaku to a live room (max 30 chars)
//!   - `bili_get_danmaku_history` — get recent danmaku from a room

pub mod api;
pub mod tools;
pub mod types;

use std::time::Duration;

use async_trait::async_trait;
use serde_json::Value;
use tracing::{info, warn};

use crate::error::{AdapterError, Result};
use crate::proxy;
use crate::traits::{Adapter, AdapterType, AuthRequirement, HealthStatus, ToolDefinition};

use tools::{build_tool_definitions, tool_get_danmaku_history, tool_get_live_info,
             tool_send_danmaku};

/// Bilibili Open Live Platform adapter.
pub struct BilibiliAdapter {
    id: String,
    connected: bool,
    access_key_id: Option<String>,
    access_key_secret: Option<String>,
    app_id: Option<String>,
    pub(crate) client: reqwest::Client,
}

impl BilibiliAdapter {
    /// Create a new Bilibili adapter from environment variables.
    pub fn new(id: impl Into<String>) -> Self {
        let client = proxy::build_client(Duration::from_secs(30))
            .user_agent("OpenIntentOS/0.1")
            .build()
            .unwrap_or_default();

        Self {
            id: id.into(),
            connected: false,
            access_key_id: std::env::var("BILI_ACCESS_KEY_ID").ok(),
            access_key_secret: std::env::var("BILI_ACCESS_KEY_SECRET").ok(),
            app_id: std::env::var("BILI_APP_ID").ok(),
            client,
        }
    }

    fn has_credentials(&self) -> bool {
        self.access_key_id.is_some()
            && self.access_key_secret.is_some()
            && self.app_id.is_some()
    }

    fn key_id(&self) -> Result<&str> {
        self.access_key_id.as_deref().ok_or_else(|| AdapterError::AuthRequired {
            adapter_id: self.id.clone(),
            provider: "bilibili".to_string(),
        })
    }

    fn key_secret(&self) -> Result<&str> {
        self.access_key_secret.as_deref().ok_or_else(|| AdapterError::AuthRequired {
            adapter_id: self.id.clone(),
            provider: "bilibili".to_string(),
        })
    }

    fn app_id_str(&self) -> Result<&str> {
        self.app_id.as_deref().ok_or_else(|| AdapterError::AuthRequired {
            adapter_id: self.id.clone(),
            provider: "bilibili".to_string(),
        })
    }
}

#[async_trait]
impl Adapter for BilibiliAdapter {
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
                "No Bilibili credentials found. Set BILI_ACCESS_KEY_ID, BILI_ACCESS_KEY_SECRET, BILI_APP_ID"
            );
            return Ok(());
        }

        self.connected = true;
        info!(adapter = %self.id, "Bilibili Open Live adapter connected");
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
        if self.has_credentials() {
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
            provider: "bilibili".to_string(),
            scopes: vec![
                "BILI_ACCESS_KEY_ID".to_string(),
                "BILI_ACCESS_KEY_SECRET".to_string(),
                "BILI_APP_ID".to_string(),
            ],
        })
    }

    async fn execute_tool(&self, name: &str, params: Value) -> Result<Value> {
        if !self.has_credentials() {
            return Err(AdapterError::AuthRequired {
                adapter_id: self.id.clone(),
                provider: "bilibili".to_string(),
            });
        }

        let key_id = self.key_id()?;
        let key_secret = self.key_secret()?;
        let app_id = self.app_id_str()?;

        match name {
            "bili_get_live_info" => {
                tool_get_live_info(&self.client, key_id, key_secret, &params).await
            }
            "bili_send_danmaku" => {
                tool_send_danmaku(&self.client, key_id, key_secret, app_id, &params).await
            }
            "bili_get_danmaku_history" => {
                tool_get_danmaku_history(&self.client, &params).await
            }
            _ => Err(AdapterError::ToolNotFound {
                tool_name: name.to_string(),
                adapter_id: self.id.clone(),
            }),
        }
    }
}
