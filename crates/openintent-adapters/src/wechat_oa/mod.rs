//! WeChat Official Account (公众号) adapter for OpenIntentOS.
//!
//! Uses the official WeChat Public Platform API — safe, stable, no ban risk.
//!
//! ## Authentication
//!
//! Set env vars:
//! ```text
//! WECHAT_OA_APP_ID=wx1234567890abcdef
//! WECHAT_OA_APP_SECRET=your_app_secret_here
//! ```
//!
//! ## Sending Messages
//!
//! To proactively push a message to a follower, the user must have sent a
//! message within the past 48 hours (WeChat "48-hour window" rule) or you
//! must use a template message.
//!
//! ## Receiving Messages
//!
//! Configure your WeChat MP webhook URL to point at the OpenIntentOS web
//! server endpoint:  `POST /wechat/oa/webhook`
//!
//! Tools:
//!   - `wechat_oa_send_text`      — send a text message to a follower
//!   - `wechat_oa_send_image`     — send an image by media_id
//!   - `wechat_oa_send_template`  — send a template message
//!   - `wechat_oa_get_followers`  — list follower OpenIDs
//!   - `wechat_oa_get_user_info`  — get a follower's profile
//!   - `wechat_oa_upload_image`   — upload a temporary image, returns media_id

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
use tools::{
    tool_get_followers, tool_get_user_info, tool_send_image, tool_send_template, tool_send_text,
    tool_upload_image,
};

/// WeChat API base URL.
const WECHAT_API_BASE: &str = "https://api.weixin.qq.com/cgi-bin";

/// Access token cached with expiry timestamp (Unix seconds).
#[derive(Debug, Clone)]
struct CachedToken {
    token: String,
    expires_at: u64,
}

/// WeChat Official Account adapter.
pub struct WeChatOAAdapter {
    id: String,
    connected: bool,
    app_id: Option<String>,
    app_secret: Option<String>,
    token_cache: Arc<RwLock<Option<CachedToken>>>,
    pub(crate) client: reqwest::Client,
    pub(crate) api_base: String,
}

impl WeChatOAAdapter {
    /// Create a new WeChat OA adapter.
    ///
    /// Reads `WECHAT_OA_APP_ID` and `WECHAT_OA_APP_SECRET` from env.
    pub fn new(id: impl Into<String>) -> Self {
        let client = proxy::build_client(Duration::from_secs(30))
            .user_agent("OpenIntentOS/0.1")
            .build()
            .unwrap_or_default();

        Self {
            id: id.into(),
            connected: false,
            app_id: std::env::var("WECHAT_OA_APP_ID").ok(),
            app_secret: std::env::var("WECHAT_OA_APP_SECRET").ok(),
            token_cache: Arc::new(RwLock::new(None)),
            client,
            api_base: WECHAT_API_BASE.to_string(),
        }
    }

    /// Return a valid access token, refreshing it if expired or missing.
    pub(crate) async fn access_token(&self) -> Result<String> {
        let app_id = self.app_id.as_deref().ok_or_else(|| AdapterError::AuthRequired {
            adapter_id: self.id.clone(),
            provider: "wechat_oa".to_string(),
        })?;
        let app_secret = self.app_secret.as_deref().ok_or_else(|| AdapterError::AuthRequired {
            adapter_id: self.id.clone(),
            provider: "wechat_oa".to_string(),
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

        debug!("refreshing WeChat OA access token");
        let resp = fetch_access_token(&self.client, &self.api_base, app_id, app_secret).await?;

        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs();

        *self.token_cache.write().await = Some(CachedToken {
            token: resp.access_token.clone(),
            expires_at: now + resp.expires_in,
        });
        info!("WeChat OA access token refreshed, expires in {}s", resp.expires_in);

        Ok(resp.access_token)
    }
}

#[async_trait]
impl Adapter for WeChatOAAdapter {
    fn id(&self) -> &str {
        &self.id
    }

    fn adapter_type(&self) -> AdapterType {
        AdapterType::Messaging
    }

    async fn connect(&mut self) -> Result<()> {
        if self.app_id.is_none() || self.app_secret.is_none() {
            warn!(
                adapter = %self.id,
                "WECHAT_OA_APP_ID or WECHAT_OA_APP_SECRET not set — adapter inactive"
            );
            return Ok(());
        }
        match self.access_token().await {
            Ok(_) => {
                self.connected = true;
                info!(adapter = %self.id, "WeChat OA adapter connected");
            }
            Err(e) => {
                warn!(adapter = %self.id, error = %e, "WeChat OA token fetch failed at connect");
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
        vec![
            ToolDefinition {
                name: "wechat_oa_send_text".to_string(),
                description: "Send a text message to a WeChat Official Account follower. The user must have interacted within 48 hours, or use wechat_oa_send_template for proactive messages.".to_string(),
                parameters: json!({
                    "type": "object",
                    "properties": {
                        "openid": { "type": "string", "description": "The follower's OpenID" },
                        "content": { "type": "string", "description": "Text content (max 2048 characters)" }
                    },
                    "required": ["openid", "content"]
                }),
            },
            ToolDefinition {
                name: "wechat_oa_send_image".to_string(),
                description: "Send an image message to a WeChat Official Account follower by media_id.".to_string(),
                parameters: json!({
                    "type": "object",
                    "properties": {
                        "openid": { "type": "string", "description": "The follower's OpenID" },
                        "media_id": { "type": "string", "description": "media_id from wechat_oa_upload_image" }
                    },
                    "required": ["openid", "media_id"]
                }),
            },
            ToolDefinition {
                name: "wechat_oa_send_template".to_string(),
                description: "Send a WeChat template message (proactive push, no 48-hour restriction). Template must be configured in WeChat MP console.".to_string(),
                parameters: json!({
                    "type": "object",
                    "properties": {
                        "openid": { "type": "string", "description": "The follower's OpenID" },
                        "template_id": { "type": "string", "description": "Template ID from WeChat MP console" },
                        "data": { "type": "object", "description": "Template variables" },
                        "url": { "type": "string", "description": "Optional redirect URL" }
                    },
                    "required": ["openid", "template_id", "data"]
                }),
            },
            ToolDefinition {
                name: "wechat_oa_get_followers".to_string(),
                description: "List follower OpenIDs for this WeChat Official Account (paginated).".to_string(),
                parameters: json!({
                    "type": "object",
                    "properties": {
                        "next_openid": { "type": "string", "description": "Pagination cursor (omit for first page)" }
                    }
                }),
            },
            ToolDefinition {
                name: "wechat_oa_get_user_info".to_string(),
                description: "Get a WeChat follower's profile (nickname, avatar, city, etc.).".to_string(),
                parameters: json!({
                    "type": "object",
                    "properties": {
                        "openid": { "type": "string", "description": "The follower's OpenID" },
                        "lang": { "type": "string", "description": "zh_CN (default), zh_TW, en", "default": "zh_CN" }
                    },
                    "required": ["openid"]
                }),
            },
            ToolDefinition {
                name: "wechat_oa_upload_image".to_string(),
                description: "Upload a temporary image to WeChat media storage. Returns a media_id valid for 3 days.".to_string(),
                parameters: json!({
                    "type": "object",
                    "properties": {
                        "file_path": { "type": "string", "description": "Local path to image file (JPEG/PNG/GIF/BMP, max 2MB)" }
                    },
                    "required": ["file_path"]
                }),
            },
        ]
    }

    fn required_auth(&self) -> Option<AuthRequirement> {
        Some(AuthRequirement {
            provider: "wechat_oa".to_string(),
            scopes: vec!["WECHAT_OA_APP_ID".to_string(), "WECHAT_OA_APP_SECRET".to_string()],
        })
    }

    async fn execute_tool(&self, name: &str, params: Value) -> Result<Value> {
        let token = self.access_token().await?;
        match name {
            "wechat_oa_send_text" => {
                tool_send_text(&self.client, &self.api_base, &token, &params).await
            }
            "wechat_oa_send_image" => {
                tool_send_image(&self.client, &self.api_base, &token, &params).await
            }
            "wechat_oa_send_template" => {
                tool_send_template(&self.client, &self.api_base, &token, &params).await
            }
            "wechat_oa_get_followers" => {
                tool_get_followers(&self.client, &self.api_base, &token, &params).await
            }
            "wechat_oa_get_user_info" => {
                tool_get_user_info(&self.client, &self.api_base, &token, &params).await
            }
            "wechat_oa_upload_image" => {
                tool_upload_image(&self.client, &self.api_base, &token, &params).await
            }
            _ => Err(AdapterError::ToolNotFound {
                tool_name: name.to_string(),
                adapter_id: self.id.clone(),
            }),
        }
    }
}
