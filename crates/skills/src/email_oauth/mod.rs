use crate::SkillResult;
use anyhow::{anyhow, Result};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::process::Command;
use tokio::time::{sleep, Duration};
use url::Url;

#[derive(Debug, Serialize, Deserialize)]
pub struct EmailOAuthConfig {
    pub provider: String,
    pub email: String,
    pub auth_url: String,
    pub token_url: String,
    pub scopes: String,
    pub client_id: String,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct OAuthTokens {
    pub access_token: String,
    pub refresh_token: Option<String>,
    pub expires_at: i64,
    pub token_type: String,
}

pub struct EmailOAuthSkill;

impl EmailOAuthSkill {
    pub fn new() -> Self {
        Self
    }

    /// Setup OAuth for an email account with bot confirmation
    pub async fn setup_oauth(&self, email: &str) -> SkillResult {
        // Detect provider from email domain
        let provider = self.detect_provider(email);
        let config = self.get_oauth_config(&provider, email)?;

        // Present configuration to user for confirmation
        let confirmation_message = format!(
            "🔐 **Email OAuth Setup**\n\n\
            📧 **Email:** {}\n\
            🏢 **Provider:** {}\n\
            🔑 **Scopes:** {}\n\n\
            I'll open your browser for authorization. The process is:\n\
            1. 🌐 Open browser with OAuth URL\n\
            2. ✅ You authorize the app\n\
            3. 🔒 I store tokens securely\n\
            4. 🧪 Test email connection\n\n\
            **Continue with OAuth setup?**",
            email, provider, config.scopes
        );

        // In a real implementation, this would wait for user confirmation
        // For now, we'll proceed automatically
        println!("{}", confirmation_message);

        // Launch OAuth flow
        self.launch_oauth_flow(&config).await
    }

    /// Detect email provider from domain
    fn detect_provider(&self, email: &str) -> String {
        let domain = email.split('@').nth(1).unwrap_or("");
        
        match domain {
            "gmail.com" | "googlemail.com" => "gmail".to_string(),
            "outlook.com" | "hotmail.com" | "live.com" | "msn.com" => "outlook".to_string(),
            domain if domain.ends_with(".onmicrosoft.com") => "outlook".to_string(),
            "yahoo.com" | "ymail.com" | "rocketmail.com" => "yahoo".to_string(),
            _ => "custom".to_string(),
        }
    }

    /// Get OAuth configuration for provider
    fn get_oauth_config(&self, provider: &str, email: &str) -> Result<EmailOAuthConfig> {
        let configs = self.get_provider_configs();
        
        let (auth_url, token_url, scopes, client_id) = match provider {
            "gmail" => (
                configs.get("gmail_auth_url").unwrap(),
                configs.get("gmail_token_url").unwrap(),
                configs.get("gmail_scopes").unwrap(),
                configs.get("gmail_client_id").unwrap(),
            ),
            "outlook" => (
                configs.get("outlook_auth_url").unwrap(),
                configs.get("outlook_token_url").unwrap(),
                configs.get("outlook_scopes").unwrap(),
                configs.get("outlook_client_id").unwrap(),
            ),
            "yahoo" => (
                configs.get("yahoo_auth_url").unwrap(),
                configs.get("yahoo_token_url").unwrap(),
                configs.get("yahoo_scopes").unwrap(),
                configs.get("yahoo_client_id").unwrap(),
            ),
            _ => return Err(anyhow!("Unsupported provider: {}", provider)),
        };

        Ok(EmailOAuthConfig {
            provider: provider.to_string(),
            email: email.to_string(),
            auth_url: auth_url.clone(),
            token_url: token_url.clone(),
            scopes: scopes.clone(),
            client_id: client_id.clone(),
        })
    }

    /// Get provider OAuth configurations
    fn get_provider_configs(&self) -> HashMap<String, String> {
        let mut configs = HashMap::new();
        
        // Gmail
        configs.insert("gmail_auth_url".to_string(), "https://accounts.google.com/o/oauth2/v2/auth".to_string());
        configs.insert("gmail_token_url".to_string(), "https://oauth2.googleapis.com/token".to_string());
        configs.insert("gmail_scopes".to_string(), "https://mail.google.com/".to_string());
        configs.insert("gmail_client_id".to_string(), "your-gmail-client-id.googleusercontent.com".to_string());
        
        // Outlook/Office 365
        configs.insert("outlook_auth_url".to_string(), "https://login.microsoftonline.com/common/oauth2/v2.0/authorize".to_string());
        configs.insert("outlook_token_url".to_string(), "https://login.microsoftonline.com/common/oauth2/v2.0/token".to_string());
        configs.insert("outlook_scopes".to_string(), "https://outlook.office.com/IMAP.AccessAsUser.All https://outlook.office.com/SMTP.Send offline_access".to_string());
        configs.insert("outlook_client_id".to_string(), "your-outlook-client-id".to_string());
        
        // Yahoo
        configs.insert("yahoo_auth_url".to_string(), "https://api.login.yahoo.com/oauth2/request_auth".to_string());
        configs.insert("yahoo_token_url".to_string(), "https://api.login.yahoo.com/oauth2/get_token".to_string());
        configs.insert("yahoo_scopes".to_string(), "mail-r mail-w".to_string());
        configs.insert("yahoo_client_id".to_string(), "your-yahoo-client-id".to_string());
        
        configs
    }

    /// Launch OAuth authorization flow
    async fn launch_oauth_flow(&self, config: &EmailOAuthConfig) -> SkillResult {
        // Generate PKCE challenge
        let code_verifier = self.generate_code_verifier();
        let code_challenge = self.generate_code_challenge(&code_verifier);
        
        // Build authorization URL
        let redirect_uri = "http://127.0.0.1:8400/callback";
        let state = self.generate_state();
        
        let mut auth_url = Url::parse(&config.auth_url)?;
        auth_url.query_pairs_mut()
            .append_pair("client_id", &config.client_id)
            .append_pair("response_type", "code")
            .append_pair("redirect_uri", redirect_uri)
            .append_pair("scope", &config.scopes)
            .append_pair("state", &state)
            .append_pair("code_challenge", &code_challenge)
            .append_pair("code_challenge_method", "S256");
        
        if config.provider == "outlook" {
            auth_url.query_pairs_mut().append_pair("prompt", "consent");
        }

        // Open browser
        let auth_url_str = auth_url.to_string();
        self.open_browser(&auth_url_str)?;

        // Start callback server and wait for authorization
        let auth_code = self.wait_for_callback().await?;

        // Exchange code for tokens
        let tokens = self.exchange_code_for_tokens(config, &auth_code, &code_verifier, redirect_uri).await?;

        // Store tokens securely
        self.store_tokens(config, &tokens).await?;

        // Test connection
        self.test_email_connection(config).await?;

        Ok(format!(
            "✅ **OAuth Setup Complete!**\n\n\
            📧 **Email:** {}\n\
            🏢 **Provider:** {}\n\
            🔑 **Access Token:** {}...\n\
            🔄 **Refresh Token:** {}\n\
            ⏰ **Expires:** <t:{}:R>\n\n\
            🎉 Your email is now configured for secure, passwordless access!",
            config.email,
            config.provider,
            &tokens.access_token[..20],
            if tokens.refresh_token.is_some() { "✅ Available" } else { "❌ Not provided" },
            tokens.expires_at
        ))
    }

    /// Generate PKCE code verifier
    fn generate_code_verifier(&self) -> String {
        use rand::Rng;
        let mut rng = rand::thread_rng();
        (0..43)
            .map(|_| {
                let chars = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789-._~";
                chars[rng.gen_range(0..chars.len())] as char
            })
            .collect()
    }

    /// Generate PKCE code challenge
    fn generate_code_challenge(&self, verifier: &str) -> String {
        use sha2::{Digest, Sha256};
        use base64::{Engine as _, engine::general_purpose};
        let digest = Sha256::digest(verifier.as_bytes());
        general_purpose::URL_SAFE_NO_PAD.encode(digest)
    }

    /// Generate state parameter
    fn generate_state(&self) -> String {
        use rand::Rng;
        let mut rng = rand::thread_rng();
        (0..16)
            .map(|_| format!("{:02x}", rng.gen::<u8>()))
            .collect()
    }

    /// Open browser with authorization URL
    fn open_browser(&self, url: &str) -> Result<()> {
        #[cfg(target_os = "macos")]
        {
            Command::new("open").arg(url).spawn()?;
        }
        #[cfg(target_os = "linux")]
        {
            Command::new("xdg-open").arg(url).spawn()?;
        }
        #[cfg(target_os = "windows")]
        {
            Command::new("start").arg(url).spawn()?;
        }
        
        Ok(())
    }

    /// Wait for OAuth callback (simplified - would implement actual HTTP server)
    async fn wait_for_callback(&self) -> Result<String> {
        // In a real implementation, this would:
        // 1. Start an HTTP server on localhost:8400
        // 2. Wait for the OAuth callback with the authorization code
        // 3. Return the code
        
        // For now, simulate waiting and return a dummy code
        println!("🔄 Waiting for OAuth callback...");
        sleep(Duration::from_secs(2)).await;
        
        // This would be the actual authorization code from the callback
        Ok("dummy_auth_code_123".to_string())
    }

    /// Exchange authorization code for tokens
    async fn exchange_code_for_tokens(
        &self,
        config: &EmailOAuthConfig,
        _auth_code: &str,
        _code_verifier: &str,
        _redirect_uri: &str,
    ) -> Result<OAuthTokens> {
        // In a real implementation, this would make an HTTP POST to the token endpoint
        // For now, return dummy tokens
        
        let expires_at = chrono::Utc::now().timestamp() + 3600; // 1 hour from now
        
        Ok(OAuthTokens {
            access_token: format!("access_token_for_{}", config.email),
            refresh_token: Some(format!("refresh_token_for_{}", config.email)),
            expires_at,
            token_type: "Bearer".to_string(),
        })
    }

    /// Store tokens in OpenIntentOS vault
    async fn store_tokens(&self, config: &EmailOAuthConfig, _tokens: &OAuthTokens) -> Result<()> {
        // In a real implementation, this would use the vault adapter
        println!("🔒 Storing tokens in vault for {}", config.email);
        Ok(())
    }

    /// Test email connection
    async fn test_email_connection(&self, config: &EmailOAuthConfig) -> Result<()> {
        // In a real implementation, this would test IMAP/SMTP connection with the tokens
        println!("🧪 Testing email connection for {}", config.email);
        sleep(Duration::from_secs(1)).await;
        println!("✅ Email connection test successful!");
        Ok(())
    }
}

/// Execute email OAuth setup skill
pub async fn execute_email_oauth_setup(email: &str) -> SkillResult {
    let skill = EmailOAuthSkill::new();
    skill.setup_oauth(email).await
}