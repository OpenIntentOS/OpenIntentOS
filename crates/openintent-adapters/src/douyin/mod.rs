//! Douyin (抖音) adapter for OpenIntentOS.
//!
//! Integrates with the Douyin Open Platform to publish videos, retrieve video
//! lists, and query video performance stats on behalf of authenticated users.
//!
//! ## Authentication
//!
//! The adapter supports two credential modes:
//!
//! ### Mode 1: Pre-obtained user token (recommended for single-account use)
//! ```env
//! DOUYIN_CLIENT_KEY=your_client_key
//! DOUYIN_CLIENT_SECRET=your_client_secret
//! DOUYIN_ACCESS_TOKEN=user_access_token
//! DOUYIN_OPEN_ID=user_open_id
//! ```
//!
//! ### Mode 2: OAuth2 flow (use `douyin_get_auth_url` to obtain a token)
//! ```env
//! DOUYIN_CLIENT_KEY=your_client_key
//! DOUYIN_CLIENT_SECRET=your_client_secret
//! ```
//!
//! Tools:
//!   - `douyin_get_auth_url`    — build the OAuth2 URL for user authorization
//!   - `douyin_publish_video`   — upload and publish a video
//!   - `douyin_get_video_list`  — list videos for the authenticated user
//!   - `douyin_get_video_stats` — get plays, likes, comments, shares for a video

pub mod content;
pub mod oauth;
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

use tools::{
    build_tool_definitions, tool_get_auth_url, tool_get_video_list, tool_get_video_stats,
    tool_publish_video,
};

/// Douyin API base URL.
const DOUYIN_API_BASE: &str = "https://open.douyin.com";

/// Cached machine-level client token with expiry.
#[derive(Debug, Clone)]
struct CachedToken {
    token: String,
    expires_at: u64,
}

/// Douyin Open Platform adapter.
pub struct DouyinAdapter {
    id: String,
    connected: bool,
    client_key: Option<String>,
    client_secret: Option<String>,
    /// Pre-configured user OAuth2 access token.
    access_token: Option<String>,
    open_id: Option<String>,
    /// Machine-level client_credential token for platform APIs.
    client_token: Arc<RwLock<Option<CachedToken>>>,
    pub(crate) client: reqwest::Client,
    pub(crate) api_base: String,
}

impl DouyinAdapter {
    /// Create a new Douyin adapter from environment variables.
    pub fn new(id: impl Into<String>) -> Self {
        let client = proxy::build_client(Duration::from_secs(30))
            .user_agent("OpenIntentOS/0.1")
            .build()
            .unwrap_or_default();

        Self {
            id: id.into(),
            connected: false,
            client_key: std::env::var("DOUYIN_CLIENT_KEY").ok(),
            client_secret: std::env::var("DOUYIN_CLIENT_SECRET").ok(),
            access_token: std::env::var("DOUYIN_ACCESS_TOKEN").ok(),
            open_id: std::env::var("DOUYIN_OPEN_ID").ok(),
            client_token: Arc::new(RwLock::new(None)),
            client,
            api_base: DOUYIN_API_BASE.to_string(),
        }
    }

    /// Fetch a machine-level client_credential token, refreshing if expired.
    pub(crate) async fn fetch_client_token(&self) -> Result<String> {
        let key = self.client_key.as_deref().ok_or_else(|| AdapterError::AuthRequired {
            adapter_id: self.id.clone(),
            provider: "douyin".to_string(),
        })?;
        let secret = self.client_secret.as_deref().ok_or_else(|| AdapterError::AuthRequired {
            adapter_id: self.id.clone(),
            provider: "douyin".to_string(),
        })?;

        // Return cached token if still valid.
        {
            let guard = self.client_token.read().await;
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

        debug!("fetching Douyin client_credential token");

        let url = format!("{}/oauth/client_token/", self.api_base);
        let body = serde_json::json!({
            "client_key": key,
            "client_secret": secret,
            "grant_type": "client_credential"
        });

        let resp: serde_json::Value = self
            .client
            .post(&url)
            .json(&body)
            .send()
            .await
            .map_err(|e| AdapterError::Internal(format!("Douyin client_token request: {e}")))?
            .json()
            .await
            .map_err(|e| AdapterError::Internal(format!("Douyin client_token parse: {e}")))?;

        let data = resp.get("data").ok_or_else(|| AdapterError::Internal(
            "Douyin client_token: missing 'data' field".to_string(),
        ))?;

        let token = data
            .get("access_token")
            .and_then(|v| v.as_str())
            .ok_or_else(|| AdapterError::Internal(
                "Douyin client_token: missing access_token".to_string(),
            ))?
            .to_string();

        let expires_in = data
            .get("expires_in")
            .and_then(|v| v.as_u64())
            .unwrap_or(86400);

        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs();

        *self.client_token.write().await = Some(CachedToken {
            token: token.clone(),
            expires_at: now + expires_in,
        });

        info!("Douyin client_credential token refreshed, expires in {expires_in}s");
        Ok(token)
    }

    /// Return the user access token, or error if not configured.
    fn user_token(&self) -> Result<(&str, &str)> {
        let token = self.access_token.as_deref().ok_or_else(|| AdapterError::AuthRequired {
            adapter_id: self.id.clone(),
            provider: "douyin".to_string(),
        })?;
        let open_id = self.open_id.as_deref().ok_or_else(|| AdapterError::AuthRequired {
            adapter_id: self.id.clone(),
            provider: "douyin".to_string(),
        })?;
        Ok((token, open_id))
    }
}

#[async_trait]
impl Adapter for DouyinAdapter {
    fn id(&self) -> &str {
        &self.id
    }

    fn adapter_type(&self) -> AdapterType {
        AdapterType::Messaging
    }

    async fn connect(&mut self) -> Result<()> {
        if self.client_key.is_none() || self.client_secret.is_none() {
            warn!(
                adapter = %self.id,
                "DOUYIN_CLIENT_KEY or DOUYIN_CLIENT_SECRET not set — adapter inactive"
            );
            return Ok(());
        }

        // If user token is already set, we can operate immediately.
        if self.access_token.is_some() {
            self.connected = true;
            info!(adapter = %self.id, "Douyin adapter connected with pre-configured user token");
        }

        // Try to fetch the machine-level client token.
        match self.fetch_client_token().await {
            Ok(_) => {
                if !self.connected {
                    // Connected at the machine level; user token needed for user actions.
                    self.connected = true;
                    info!(adapter = %self.id, "Douyin adapter connected (client_credential only)");
                }
            }
            Err(e) => {
                warn!(adapter = %self.id, error = %e, "Douyin client_token fetch failed at connect");
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
        match self.fetch_client_token().await {
            Ok(_) => Ok(HealthStatus::Healthy),
            Err(_) => Ok(HealthStatus::Degraded),
        }
    }

    fn tools(&self) -> Vec<ToolDefinition> {
        build_tool_definitions()
    }

    fn required_auth(&self) -> Option<AuthRequirement> {
        Some(AuthRequirement {
            provider: "douyin".into(),
            scopes: vec![
                "DOUYIN_CLIENT_KEY".into(),
                "DOUYIN_CLIENT_SECRET".into(),
            ],
        })
    }

    async fn execute_tool(&self, name: &str, params: Value) -> Result<Value> {
        match name {
            "douyin_get_auth_url" => {
                let key = self.client_key.as_deref().ok_or_else(|| AdapterError::AuthRequired {
                    adapter_id: self.id.clone(),
                    provider: "douyin".to_string(),
                })?;
                tool_get_auth_url(key, &params)
            }
            "douyin_publish_video" | "douyin_get_video_list" | "douyin_get_video_stats" => {
                let (token, open_id) = self.user_token()?;
                match name {
                    "douyin_publish_video" => {
                        tool_publish_video(&self.client, &self.api_base, token, open_id, &params)
                            .await
                    }
                    "douyin_get_video_list" => {
                        tool_get_video_list(&self.client, &self.api_base, token, open_id, &params)
                            .await
                    }
                    "douyin_get_video_stats" => {
                        tool_get_video_stats(&self.client, &self.api_base, token, open_id, &params)
                            .await
                    }
                    _ => unreachable!(),
                }
            }
            _ => Err(AdapterError::ToolNotFound {
                tool_name: name.to_string(),
                adapter_id: self.id.clone(),
            }),
        }
    }
}
