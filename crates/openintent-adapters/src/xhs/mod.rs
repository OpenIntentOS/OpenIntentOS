//! XiaoHongShu (小红书) adapter for OpenIntentOS.
//!
//! Integrates with the XiaoHongShu Ark Platform API for publishing notes and
//! querying note data.
//!
//! ## Authentication
//!
//! XiaoHongShu uses HMAC-SHA256 request signing — there is no OAuth2 bearer
//! token.  Set the following environment variables from the Ark console at
//! school.xiaohongshu.com:
//!
//! ```env
//! XHS_APP_KEY=your_app_key
//! XHS_APP_SECRET=your_app_secret
//! ```
//!
//! Every request is signed with a timestamp and HMAC-SHA256 signature derived
//! from the path, sorted query string, request body, and app_secret.
//!
//! Tools:
//!   - `xhs_publish_note`   — publish a note (image or text type)
//!   - `xhs_search_notes`   — search notes by keyword
//!   - `xhs_get_note_stats` — get stats and details for a note
//!   - `xhs_get_comments`   — get comments for a note

pub mod auth;
pub mod notes;
pub mod tools;

use std::time::Duration;

use async_trait::async_trait;
use serde_json::Value;
use tracing::{info, warn};

use crate::error::{AdapterError, Result};
use crate::proxy;
use crate::traits::{Adapter, AdapterType, AuthRequirement, HealthStatus, ToolDefinition};

use tools::{
    build_tool_definitions, tool_get_comments, tool_get_note_stats, tool_publish_note,
    tool_search_notes,
};

/// XiaoHongShu API base URL.
const XHS_API_BASE: &str = "https://api.xiaohongshu.com";

/// XiaoHongShu Ark Platform adapter.
pub struct XhsAdapter {
    id: String,
    connected: bool,
    app_key: Option<String>,
    app_secret: Option<String>,
    pub(crate) client: reqwest::Client,
    pub(crate) api_base: String,
}

impl XhsAdapter {
    /// Create a new XHS adapter from environment variables.
    pub fn new(id: impl Into<String>) -> Self {
        let client = proxy::build_client(Duration::from_secs(30))
            .user_agent("OpenIntentOS/0.1")
            .build()
            .unwrap_or_default();

        Self {
            id: id.into(),
            connected: false,
            app_key: std::env::var("XHS_APP_KEY").ok(),
            app_secret: std::env::var("XHS_APP_SECRET").ok(),
            client,
            api_base: XHS_API_BASE.to_string(),
        }
    }

    /// Return app_key and app_secret, or error if not configured.
    fn credentials(&self) -> Result<(&str, &str)> {
        let key = self.app_key.as_deref().ok_or_else(|| AdapterError::AuthRequired {
            adapter_id: self.id.clone(),
            provider: "xhs".to_string(),
        })?;
        let secret = self.app_secret.as_deref().ok_or_else(|| AdapterError::AuthRequired {
            adapter_id: self.id.clone(),
            provider: "xhs".to_string(),
        })?;
        Ok((key, secret))
    }
}

#[async_trait]
impl Adapter for XhsAdapter {
    fn id(&self) -> &str {
        &self.id
    }

    fn adapter_type(&self) -> AdapterType {
        AdapterType::Messaging
    }

    async fn connect(&mut self) -> Result<()> {
        if self.app_key.is_none() || self.app_secret.is_none() {
            warn!(
                adapter = %self.id,
                "XHS_APP_KEY or XHS_APP_SECRET not set — adapter inactive"
            );
            return Ok(());
        }
        self.connected = true;
        info!(adapter = %self.id, "XiaoHongShu adapter connected");
        Ok(())
    }

    async fn disconnect(&mut self) -> Result<()> {
        self.connected = false;
        Ok(())
    }

    async fn health_check(&self) -> Result<HealthStatus> {
        if self.connected && self.app_key.is_some() && self.app_secret.is_some() {
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
            provider: "xhs".into(),
            scopes: vec!["XHS_APP_KEY".into(), "XHS_APP_SECRET".into()],
        })
    }

    async fn execute_tool(&self, name: &str, params: Value) -> Result<Value> {
        let (key, secret) = self.credentials()?;

        match name {
            "xhs_publish_note" => {
                tool_publish_note(&self.client, &self.api_base, key, secret, &params).await
            }
            "xhs_search_notes" => {
                tool_search_notes(&self.client, &self.api_base, key, secret, &params).await
            }
            "xhs_get_note_stats" => {
                tool_get_note_stats(&self.client, &self.api_base, key, secret, &params).await
            }
            "xhs_get_comments" => {
                tool_get_comments(&self.client, &self.api_base, key, secret, &params).await
            }
            _ => Err(AdapterError::ToolNotFound {
                tool_name: name.to_string(),
                adapter_id: self.id.clone(),
            }),
        }
    }
}
