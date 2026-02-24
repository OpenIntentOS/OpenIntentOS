//! High-level authentication session manager.
//!
//! The [`AuthManager`] orchestrates OAuth and device code flows end-to-end,
//! storing tokens in the vault and handling automatic refresh. It is the
//! primary entry point for consuming code that needs to authenticate with
//! third-party services.

use std::sync::Mutex;

use openintent_vault::store::{CredentialType, Vault};

use crate::callback::CallbackServer;
use crate::device_code::{DeviceCodeConfig, DeviceCodeFlow};
use crate::error::{AuthEngineError, Result};
use crate::oauth::{OAuthConfig, OAuthFlow, OAuthTokens, generate_pkce_verifier, pkce_challenge};

/// Default port for the local OAuth callback server.
const DEFAULT_CALLBACK_PORT: u16 = 8400;

/// Default timeout for the callback server in seconds (5 minutes).
const DEFAULT_CALLBACK_TIMEOUT_SECS: u64 = 300;

/// Default timeout for device code polling in seconds (15 minutes).
const DEFAULT_DEVICE_CODE_TIMEOUT_SECS: u64 = 900;

/// Vault key prefix for stored OAuth tokens.
const TOKEN_KEY_PREFIX: &str = "oauth_tokens:";

// ---------------------------------------------------------------------------
// AuthManager
// ---------------------------------------------------------------------------

/// High-level manager for authentication flows.
///
/// Coordinates OAuth and device code flows, stores tokens in the vault,
/// and handles automatic token refresh.
///
/// The `Vault` is wrapped in a `Mutex` because `rusqlite::Connection` is
/// `!Send`. All vault operations are performed synchronously inside the
/// mutex lock, which is held briefly for each operation.
pub struct AuthManager {
    vault: Mutex<Vault>,
}

// Safety: AuthManager is Send + Sync because the Vault is wrapped in a Mutex.
// The Mutex ensures that only one thread accesses the Connection at a time.
unsafe impl Send for AuthManager {}
unsafe impl Sync for AuthManager {}

impl AuthManager {
    /// Create a new authentication manager backed by the given vault.
    pub fn new(vault: Vault) -> Self {
        Self {
            vault: Mutex::new(vault),
        }
    }

    /// Perform a full OAuth 2.0 authorization code flow with PKCE.
    ///
    /// This method:
    /// 1. Generates a PKCE code verifier and challenge.
    /// 2. Generates a random state parameter for CSRF protection.
    /// 3. Starts a local callback server on port 8400.
    /// 4. Logs the authorization URL for the user to visit.
    /// 5. Waits for the callback with the authorization code.
    /// 6. Verifies the state parameter matches.
    /// 7. Exchanges the code for tokens.
    /// 8. Stores the tokens in the vault.
    ///
    /// # Errors
    ///
    /// Returns errors if any step of the flow fails (network, callback
    /// timeout, state mismatch, token exchange, vault storage).
    pub async fn authenticate_oauth(
        &self,
        provider: &str,
        config: OAuthConfig,
    ) -> Result<OAuthTokens> {
        tracing::info!(
            provider = provider,
            "starting OAuth authorization code flow"
        );

        // Step 1: Generate PKCE verifier and challenge.
        let code_verifier = generate_pkce_verifier()?;
        let code_challenge = pkce_challenge(&code_verifier);

        // Step 2: Generate random state for CSRF protection.
        let state = uuid::Uuid::now_v7().to_string();

        // Step 3: Build the authorization URL.
        let flow = OAuthFlow::new(config.clone());
        let auth_url = flow.authorization_url(&state, &code_challenge)?;

        // Step 4: Log the URL for the user to visit.
        tracing::info!(
            url = %auth_url,
            "open this URL in your browser to authorize"
        );

        // Step 5: Start callback server and wait for redirect.
        let (code, returned_state) =
            CallbackServer::start(DEFAULT_CALLBACK_PORT, DEFAULT_CALLBACK_TIMEOUT_SECS).await?;

        // Step 6: Verify state matches to prevent CSRF.
        if returned_state != state {
            return Err(AuthEngineError::FlowFailed {
                reason: format!("state mismatch: expected {state}, got {returned_state}"),
            });
        }

        tracing::debug!("state parameter verified, exchanging code for tokens");

        // Step 7: Exchange the authorization code for tokens.
        let tokens = flow.exchange_code(&code, &code_verifier).await?;

        // Step 8: Store tokens in the vault.
        self.store_tokens(provider, &tokens)?;

        tracing::info!(provider = provider, "OAuth flow completed successfully");
        Ok(tokens)
    }

    /// Perform an RFC 8628 device authorization grant flow.
    ///
    /// This method:
    /// 1. Requests a device code from the authorization server.
    /// 2. Logs the user code and verification URI for the user.
    /// 3. Polls the token endpoint until the user completes authorization.
    /// 4. Stores the tokens in the vault.
    ///
    /// # Errors
    ///
    /// Returns errors if any step of the flow fails (network, user denial,
    /// device code expiry, vault storage).
    pub async fn authenticate_device_code(
        &self,
        provider: &str,
        config: DeviceCodeConfig,
    ) -> Result<OAuthTokens> {
        tracing::info!(provider = provider, "starting device code flow");

        let flow = DeviceCodeFlow::new(config);

        // Step 1: Request a device code.
        let device_response = flow.request_device_code().await?;

        // Step 2: Display the code and URI to the user.
        tracing::info!(
            user_code = %device_response.user_code,
            verification_uri = %device_response.verification_uri,
            "enter this code at the URL shown to authorize"
        );

        if let Some(ref complete_uri) = device_response.verification_uri_complete {
            tracing::info!(
                url = %complete_uri,
                "or open this URL directly"
            );
        }

        // Step 3: Poll for token.
        let tokens = flow
            .poll_for_token(
                &device_response.device_code,
                device_response.interval,
                DEFAULT_DEVICE_CODE_TIMEOUT_SECS,
            )
            .await?;

        // Step 4: Store tokens in the vault.
        self.store_tokens(provider, &tokens)?;

        tracing::info!(
            provider = provider,
            "device code flow completed successfully"
        );
        Ok(tokens)
    }

    /// Get a valid access token for the given provider.
    ///
    /// If the stored token is expired and a refresh config is provided,
    /// this method automatically refreshes the token and updates the vault.
    ///
    /// # Errors
    ///
    /// Returns [`AuthEngineError::ProviderNotFound`] if no tokens are stored
    /// for the provider, [`AuthEngineError::TokenExpired`] if the token is
    /// expired and cannot be refreshed.
    pub async fn get_valid_token(
        &self,
        provider: &str,
        refresh_config: Option<&OAuthConfig>,
    ) -> Result<String> {
        let tokens = self.load_tokens(provider)?;

        // If the token is not expired, return it directly.
        if !OAuthFlow::is_expired(&tokens) {
            return Ok(tokens.access_token);
        }

        tracing::debug!(
            provider = provider,
            "access token expired, attempting refresh"
        );

        // Try to refresh if we have a refresh token and a config.
        let refresh_token =
            tokens
                .refresh_token
                .as_deref()
                .ok_or_else(|| AuthEngineError::TokenExpired {
                    provider: provider.to_string(),
                })?;

        let config = refresh_config.ok_or_else(|| AuthEngineError::TokenExpired {
            provider: provider.to_string(),
        })?;

        let flow = OAuthFlow::new(config.clone());
        let new_tokens = flow.refresh_token(refresh_token).await?;

        // Update the vault with fresh tokens.
        self.update_tokens(provider, &new_tokens)?;

        tracing::info!(provider = provider, "token refreshed successfully");
        Ok(new_tokens.access_token)
    }

    /// Revoke (delete) stored tokens for a provider.
    ///
    /// # Errors
    ///
    /// Returns [`AuthEngineError::ProviderNotFound`] if no tokens exist for
    /// the provider.
    pub fn revoke(&self, provider: &str) -> Result<()> {
        let key = format!("{TOKEN_KEY_PREFIX}{provider}");
        let vault = self.vault.lock().map_err(|e| AuthEngineError::FlowFailed {
            reason: format!("vault lock poisoned: {e}"),
        })?;

        vault.delete_credential(&key).map_err(|e| match e {
            openintent_vault::VaultError::CredentialNotFound { .. } => {
                AuthEngineError::ProviderNotFound {
                    provider: provider.to_string(),
                }
            }
            other => AuthEngineError::VaultError(other),
        })?;

        tracing::info!(provider = provider, "tokens revoked");
        Ok(())
    }

    // -- Internal helpers ---------------------------------------------------

    /// Store tokens in the vault as a JSON credential.
    fn store_tokens(&self, provider: &str, tokens: &OAuthTokens) -> Result<()> {
        let key = format!("{TOKEN_KEY_PREFIX}{provider}");
        let data = serde_json::to_value(tokens)?;
        let expires_at = tokens
            .expires_at
            .and_then(|ts| chrono::DateTime::from_timestamp(ts, 0));

        let vault = self.vault.lock().map_err(|e| AuthEngineError::FlowFailed {
            reason: format!("vault lock poisoned: {e}"),
        })?;

        // Try to store; if it already exists, update instead.
        let scopes: Vec<String> = tokens.scopes.clone();
        match vault.store_credential(
            &key,
            CredentialType::OAuth,
            &data,
            if scopes.is_empty() {
                None
            } else {
                Some(scopes.as_slice())
            },
            Some(provider),
            expires_at,
        ) {
            Ok(()) => Ok(()),
            Err(openintent_vault::VaultError::CredentialAlreadyExists { .. }) => {
                vault.update_credential(&key, &data, expires_at)?;
                Ok(())
            }
            Err(e) => Err(AuthEngineError::VaultError(e)),
        }
    }

    /// Update existing tokens in the vault.
    fn update_tokens(&self, provider: &str, tokens: &OAuthTokens) -> Result<()> {
        let key = format!("{TOKEN_KEY_PREFIX}{provider}");
        let data = serde_json::to_value(tokens)?;
        let expires_at = tokens
            .expires_at
            .and_then(|ts| chrono::DateTime::from_timestamp(ts, 0));

        let vault = self.vault.lock().map_err(|e| AuthEngineError::FlowFailed {
            reason: format!("vault lock poisoned: {e}"),
        })?;

        vault.update_credential(&key, &data, expires_at)?;
        Ok(())
    }

    /// Load tokens from the vault.
    fn load_tokens(&self, provider: &str) -> Result<OAuthTokens> {
        let key = format!("{TOKEN_KEY_PREFIX}{provider}");

        let vault = self.vault.lock().map_err(|e| AuthEngineError::FlowFailed {
            reason: format!("vault lock poisoned: {e}"),
        })?;

        let credential = vault.get_credential(&key).map_err(|e| match e {
            openintent_vault::VaultError::CredentialNotFound { .. } => {
                AuthEngineError::ProviderNotFound {
                    provider: provider.to_string(),
                }
            }
            other => AuthEngineError::VaultError(other),
        })?;

        let tokens: OAuthTokens = serde_json::from_value(credential.data)?;
        Ok(tokens)
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use openintent_vault::crypto;

    fn test_vault() -> Vault {
        let key = crypto::random_bytes(crypto::KEY_LEN).unwrap();
        Vault::open_in_memory(&key).unwrap()
    }

    fn test_tokens() -> OAuthTokens {
        OAuthTokens {
            access_token: "access_tok_123".to_string(),
            refresh_token: Some("refresh_tok_456".to_string()),
            expires_at: Some(chrono::Utc::now().timestamp() + 3600),
            token_type: "Bearer".to_string(),
            scopes: vec!["read".to_string(), "write".to_string()],
        }
    }

    #[test]
    fn auth_manager_construction() {
        let vault = test_vault();
        let _manager = AuthManager::new(vault);
    }

    #[test]
    fn auth_manager_is_send_sync() {
        fn assert_send_sync<T: Send + Sync>() {}
        assert_send_sync::<AuthManager>();
    }

    #[test]
    fn store_and_load_tokens() {
        let vault = test_vault();
        let manager = AuthManager::new(vault);
        let tokens = test_tokens();

        manager.store_tokens("github", &tokens).unwrap();
        let loaded = manager.load_tokens("github").unwrap();

        assert_eq!(loaded.access_token, "access_tok_123");
        assert_eq!(loaded.refresh_token.as_deref(), Some("refresh_tok_456"));
        assert_eq!(loaded.token_type, "Bearer");
        assert_eq!(loaded.scopes, vec!["read", "write"]);
    }

    #[test]
    fn store_tokens_upsert() {
        let vault = test_vault();
        let manager = AuthManager::new(vault);

        let tokens1 = OAuthTokens {
            access_token: "first".to_string(),
            refresh_token: None,
            expires_at: None,
            token_type: "Bearer".to_string(),
            scopes: vec![],
        };

        let tokens2 = OAuthTokens {
            access_token: "second".to_string(),
            refresh_token: None,
            expires_at: None,
            token_type: "Bearer".to_string(),
            scopes: vec![],
        };

        manager.store_tokens("provider", &tokens1).unwrap();
        manager.store_tokens("provider", &tokens2).unwrap();

        let loaded = manager.load_tokens("provider").unwrap();
        assert_eq!(loaded.access_token, "second");
    }

    #[test]
    fn load_tokens_not_found() {
        let vault = test_vault();
        let manager = AuthManager::new(vault);

        let result = manager.load_tokens("nonexistent");
        assert!(matches!(
            result,
            Err(AuthEngineError::ProviderNotFound { .. })
        ));
    }

    #[test]
    fn revoke_tokens() {
        let vault = test_vault();
        let manager = AuthManager::new(vault);
        let tokens = test_tokens();

        manager.store_tokens("github", &tokens).unwrap();
        manager.revoke("github").unwrap();

        let result = manager.load_tokens("github");
        assert!(matches!(
            result,
            Err(AuthEngineError::ProviderNotFound { .. })
        ));
    }

    #[test]
    fn revoke_nonexistent_provider() {
        let vault = test_vault();
        let manager = AuthManager::new(vault);

        let result = manager.revoke("nonexistent");
        assert!(matches!(
            result,
            Err(AuthEngineError::ProviderNotFound { .. })
        ));
    }

    #[tokio::test]
    async fn get_valid_token_not_expired() {
        let vault = test_vault();
        let manager = AuthManager::new(vault);
        let tokens = test_tokens();

        manager.store_tokens("github", &tokens).unwrap();
        let token = manager.get_valid_token("github", None).await.unwrap();
        assert_eq!(token, "access_tok_123");
    }

    #[tokio::test]
    async fn get_valid_token_expired_no_refresh() {
        let vault = test_vault();
        let manager = AuthManager::new(vault);

        let tokens = OAuthTokens {
            access_token: "expired_tok".to_string(),
            refresh_token: None,
            expires_at: Some(chrono::Utc::now().timestamp() - 100),
            token_type: "Bearer".to_string(),
            scopes: vec![],
        };

        manager.store_tokens("github", &tokens).unwrap();

        let result = manager.get_valid_token("github", None).await;
        assert!(matches!(result, Err(AuthEngineError::TokenExpired { .. })));
    }

    #[tokio::test]
    async fn get_valid_token_provider_not_found() {
        let vault = test_vault();
        let manager = AuthManager::new(vault);

        let result = manager.get_valid_token("nonexistent", None).await;
        assert!(matches!(
            result,
            Err(AuthEngineError::ProviderNotFound { .. })
        ));
    }

    #[test]
    fn update_tokens() {
        let vault = test_vault();
        let manager = AuthManager::new(vault);

        let tokens1 = test_tokens();
        manager.store_tokens("github", &tokens1).unwrap();

        let tokens2 = OAuthTokens {
            access_token: "new_access".to_string(),
            refresh_token: Some("new_refresh".to_string()),
            expires_at: Some(chrono::Utc::now().timestamp() + 7200),
            token_type: "Bearer".to_string(),
            scopes: vec!["repo".to_string()],
        };

        manager.update_tokens("github", &tokens2).unwrap();

        let loaded = manager.load_tokens("github").unwrap();
        assert_eq!(loaded.access_token, "new_access");
        assert_eq!(loaded.refresh_token.as_deref(), Some("new_refresh"));
    }

    #[test]
    fn token_key_prefix_format() {
        let key = format!("{TOKEN_KEY_PREFIX}github");
        assert_eq!(key, "oauth_tokens:github");
    }
}
