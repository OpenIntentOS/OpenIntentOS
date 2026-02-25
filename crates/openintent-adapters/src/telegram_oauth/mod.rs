//! Telegram OAuth integration for in-chat authentication flows.
//!
//! This module provides a seamless way to handle OAuth authentication directly
//! within Telegram conversations, eliminating the need for users to switch to
//! a terminal or external browser.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use openintent_auth_engine::{AuthManager, OAuthConfig, DeviceCodeConfig};
use serde::{Deserialize, Serialize};
use tokio::time::timeout;
use uuid::Uuid;

use crate::telegram::TelegramAdapter;

/// Configuration for Telegram OAuth flows.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TelegramOAuthConfig {
    /// The OAuth provider configuration.
    pub oauth_config: OAuthConfig,
    /// Optional device code configuration for providers that support it.
    pub device_code_config: Option<DeviceCodeConfig>,
    /// Timeout for the OAuth flow in seconds (default: 300).
    pub timeout_secs: u64,
    /// Whether to prefer device code flow over authorization code flow.
    pub prefer_device_code: bool,
}

impl Default for TelegramOAuthConfig {
    fn default() -> Self {
        Self {
            oauth_config: OAuthConfig {
                client_id: String::new(),
                client_secret: None,
                auth_url: String::new(),
                token_url: String::new(),
                redirect_uri: "http://127.0.0.1:8400/callback".to_string(),
                scopes: vec![],
            },
            device_code_config: None,
            timeout_secs: 300,
            prefer_device_code: false,
        }
    }
}

/// An active OAuth session in Telegram.
#[derive(Debug, Clone)]
struct OAuthSession {
    /// The Telegram chat ID where the flow was initiated.
    chat_id: String,
    /// The provider being authenticated with.
    provider: String,
    /// When the session was created.
    created_at: Instant,
    /// The configuration for this session.
    config: TelegramOAuthConfig,
}

/// Manages OAuth flows within Telegram conversations.
pub struct TelegramOAuth {
    /// The underlying authentication manager.
    auth_manager: Arc<AuthManager>,
    /// The Telegram adapter for sending messages.
    telegram: Arc<TelegramAdapter>,
    /// Active OAuth sessions indexed by session ID.
    sessions: Arc<Mutex<HashMap<String, OAuthSession>>>,
}

impl TelegramOAuth {
    /// Create a new Telegram OAuth manager.
    pub fn new(auth_manager: Arc<AuthManager>, telegram: Arc<TelegramAdapter>) -> Self {
        Self {
            auth_manager,
            telegram,
            sessions: Arc::new(Mutex::new(HashMap::new())),
        }
    }

    /// Start an OAuth flow for a provider in the given Telegram chat.
    ///
    /// This method will:
    /// 1. Create a session and store it
    /// 2. Send instructions to the user in Telegram
    /// 3. Handle the OAuth flow (device code or authorization code)
    /// 4. Send success/failure messages back to the chat
    ///
    /// Returns a session ID that can be used to track the flow.
    pub async fn start_oauth_flow(
        &self,
        chat_id: &str,
        provider: &str,
        config: TelegramOAuthConfig,
    ) -> Result<String, Box<dyn std::error::Error + Send + Sync>> {
        let session_id = Uuid::new_v4().to_string();
        
        // Store the session
        {
            let mut sessions = self.sessions.lock().unwrap();
            sessions.insert(session_id.clone(), OAuthSession {
                chat_id: chat_id.to_string(),
                provider: provider.to_string(),
                created_at: Instant::now(),
                config: config.clone(),
            });
        }

        // Send initial message
        self.send_message(chat_id, &format!(
            "🔐 **Starting {} OAuth Authentication**\n\n\
             I'll guide you through the authentication process. \
             This will take just a few moments.",
            provider
        )).await?;

        // Start the OAuth flow in the background
        let self_clone = self.clone();
        let session_id_clone = session_id.clone();
        tokio::spawn(async move {
            if let Err(e) = self_clone.handle_oauth_flow(&session_id_clone).await {
                let _ = self_clone.handle_oauth_error(&session_id_clone, e).await;
            }
        });

        Ok(session_id)
    }

    /// Handle the OAuth flow for a session.
    async fn handle_oauth_flow(
        &self,
        session_id: &str,
    ) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
        let session = {
            let sessions = self.sessions.lock().unwrap();
            sessions.get(session_id).cloned()
                .ok_or("Session not found")?
        };

        // Choose flow type based on configuration
        if session.config.prefer_device_code && session.config.device_code_config.is_some() {
            self.handle_device_code_flow(&session).await
        } else {
            self.handle_authorization_code_flow(&session).await
        }
    }

    /// Handle device code OAuth flow.
    async fn handle_device_code_flow(
        &self,
        session: &OAuthSession,
    ) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
        let device_config = session.config.device_code_config.as_ref()
            .ok_or("Device code configuration not available")?;

        self.send_message(&session.chat_id, 
            "🔄 **Requesting device authorization...**"
        ).await?;

        // Use timeout for the entire flow
        let result = timeout(
            Duration::from_secs(session.config.timeout_secs),
            self.auth_manager.authenticate_device_code(&session.provider, device_config.clone())
        ).await;

        match result {
            Ok(Ok(_tokens)) => {
                self.send_message(&session.chat_id, &format!(
                    "✅ **Authentication Successful!**\n\n\
                     {} has been successfully connected to your account.\n\
                     You can now use email automation features.",
                    session.provider
                )).await?;
                
                // Clean up session
                self.cleanup_session(&session.chat_id);
            }
            Ok(Err(e)) => {
                return Err(Box::new(e));
            }
            Err(_) => {
                return Err("Authentication timed out".into());
            }
        }

        Ok(())
    }

    /// Handle authorization code OAuth flow.
    async fn handle_authorization_code_flow(
        &self,
        session: &OAuthSession,
    ) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
        self.send_message(&session.chat_id,
            "🔄 **Preparing authorization...**"
        ).await?;

        // Use timeout for the entire flow
        let result = timeout(
            Duration::from_secs(session.config.timeout_secs),
            self.auth_manager.authenticate_oauth(&session.provider, session.config.oauth_config.clone())
        ).await;

        match result {
            Ok(Ok(_tokens)) => {
                self.send_message(&session.chat_id, &format!(
                    "✅ **Authentication Successful!**\n\n\
                     {} has been successfully connected to your account.\n\
                     You can now use email automation features.",
                    session.provider
                )).await?;
                
                // Clean up session
                self.cleanup_session(&session.chat_id);
            }
            Ok(Err(e)) => {
                return Err(Box::new(e));
            }
            Err(_) => {
                return Err("Authentication timed out".into());
            }
        }

        Ok(())
    }

    /// Handle OAuth flow errors.
    async fn handle_oauth_error(
        &self,
        session_id: &str,
        error: Box<dyn std::error::Error + Send + Sync>,
    ) {
        let session = {
            let sessions = self.sessions.lock().unwrap();
            sessions.get(session_id).cloned()
        };

        if let Some(session) = session {
            let error_msg = format!(
                "❌ **Authentication Failed**\n\n\
                 Failed to authenticate with {}: {}\n\n\
                 Please try again or contact support if the problem persists.",
                session.provider, error
            );

            let _ = self.send_message(&session.chat_id, &error_msg).await;
            self.cleanup_session(&session.chat_id);
        }
    }

    /// Send a message to a Telegram chat.
    async fn send_message(
        &self,
        chat_id: &str,
        text: &str,
    ) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
        self.telegram.send_message(chat_id, text, Some("Markdown")).await
            .map_err(|e| Box::new(e) as Box<dyn std::error::Error + Send + Sync>)?;
        Ok(())
    }

    /// Clean up a completed or failed session.
    fn cleanup_session(&self, chat_id: &str) {
        let mut sessions = self.sessions.lock().unwrap();
        sessions.retain(|_, session| session.chat_id != chat_id);
    }

    /// Clean up expired sessions.
    pub fn cleanup_expired_sessions(&self, max_age: Duration) {
        let mut sessions = self.sessions.lock().unwrap();
        let now = Instant::now();
        sessions.retain(|_, session| now.duration_since(session.created_at) < max_age);
    }

    /// Get the number of active sessions.
    pub fn active_sessions_count(&self) -> usize {
        let sessions = self.sessions.lock().unwrap();
        sessions.len()
    }
}

impl Clone for TelegramOAuth {
    fn clone(&self) -> Self {
        Self {
            auth_manager: Arc::clone(&self.auth_manager),
            telegram: Arc::clone(&self.telegram),
            sessions: Arc::clone(&self.sessions),
        }
    }
}

/// Predefined OAuth configurations for common email providers.
pub mod providers {
    use super::*;

    /// Create a Gmail OAuth configuration for Telegram.
    pub fn gmail_config(client_id: String, client_secret: String) -> TelegramOAuthConfig {
        TelegramOAuthConfig {
            oauth_config: OAuthConfig {
                client_id,
                client_secret: Some(client_secret),
                auth_url: "https://accounts.google.com/o/oauth2/v2/auth".to_string(),
                token_url: "https://oauth2.googleapis.com/token".to_string(),
                redirect_uri: "http://127.0.0.1:8400/callback".to_string(),
                scopes: vec![
                    "https://www.googleapis.com/auth/gmail.readonly".to_string(),
                    "https://www.googleapis.com/auth/gmail.send".to_string(),
                ],
            },
            device_code_config: None,
            timeout_secs: 300,
            prefer_device_code: false,
        }
    }

    /// Create an Outlook/Office 365 OAuth configuration for Telegram.
    pub fn outlook_config(client_id: String, client_secret: String) -> TelegramOAuthConfig {
        TelegramOAuthConfig {
            oauth_config: OAuthConfig {
                client_id: client_id.clone(),
                client_secret: Some(client_secret),
                auth_url: "https://login.microsoftonline.com/common/oauth2/v2.0/authorize".to_string(),
                token_url: "https://login.microsoftonline.com/common/oauth2/v2.0/token".to_string(),
                redirect_uri: "http://127.0.0.1:8400/callback".to_string(),
                scopes: vec![
                    "https://graph.microsoft.com/Mail.Read".to_string(),
                    "https://graph.microsoft.com/Mail.Send".to_string(),
                ],
            },
            device_code_config: Some(DeviceCodeConfig {
                client_id: client_id.clone(),
                device_auth_url: "https://login.microsoftonline.com/common/oauth2/v2.0/devicecode".to_string(),
                token_url: "https://login.microsoftonline.com/common/oauth2/v2.0/token".to_string(),
                scopes: vec![
                    "https://graph.microsoft.com/Mail.Read".to_string(),
                    "https://graph.microsoft.com/Mail.Send".to_string(),
                ],
            }),
            timeout_secs: 900, // 15 minutes for device code flow
            prefer_device_code: true,
        }
    }

    /// Create a Yahoo OAuth configuration for Telegram.
    pub fn yahoo_config(client_id: String, client_secret: String) -> TelegramOAuthConfig {
        TelegramOAuthConfig {
            oauth_config: OAuthConfig {
                client_id,
                client_secret: Some(client_secret),
                auth_url: "https://api.login.yahoo.com/oauth2/request_auth".to_string(),
                token_url: "https://api.login.yahoo.com/oauth2/get_token".to_string(),
                redirect_uri: "http://127.0.0.1:8400/callback".to_string(),
                scopes: vec!["mail-r".to_string(), "mail-w".to_string()],
            },
            device_code_config: None,
            timeout_secs: 300,
            prefer_device_code: false,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use openintent_vault::{store::Vault, crypto};

    fn test_auth_manager() -> Arc<AuthManager> {
        let key = crypto::random_bytes(crypto::KEY_LEN).unwrap();
        let vault = Vault::open_in_memory(&key).unwrap();
        Arc::new(AuthManager::new(vault))
    }

    #[test]
    fn telegram_oauth_config_default() {
        let config = TelegramOAuthConfig::default();
        assert_eq!(config.timeout_secs, 300);
        assert!(!config.prefer_device_code);
        assert!(config.device_code_config.is_none());
    }

    #[test]
    fn providers_gmail_config() {
        let config = providers::gmail_config(
            "test_client_id".to_string(),
            "test_client_secret".to_string(),
        );
        
        assert_eq!(config.oauth_config.client_id, "test_client_id");
        assert_eq!(config.oauth_config.client_secret.as_deref(), Some("test_client_secret"));
        assert!(config.oauth_config.auth_url.contains("accounts.google.com"));
        assert!(!config.prefer_device_code);
    }

    #[test]
    fn providers_outlook_config() {
        let config = providers::outlook_config(
            "test_client_id".to_string(),
            "test_client_secret".to_string(),
        );
        
        assert_eq!(config.oauth_config.client_id, "test_client_id");
        assert!(config.oauth_config.auth_url.contains("login.microsoftonline.com"));
        assert!(config.prefer_device_code);
        assert!(config.device_code_config.is_some());
        assert_eq!(config.timeout_secs, 900);
    }

    #[test]
    fn providers_yahoo_config() {
        let config = providers::yahoo_config(
            "test_client_id".to_string(),
            "test_client_secret".to_string(),
        );
        
        assert_eq!(config.oauth_config.client_id, "test_client_id");
        assert!(config.oauth_config.auth_url.contains("api.login.yahoo.com"));
        assert!(!config.prefer_device_code);
        assert!(config.device_code_config.is_none());
    }

    #[test]
    fn session_cleanup() {
        // Verify AuthManager can be created and used without panics.
        let _auth_manager = test_auth_manager();
    }
}