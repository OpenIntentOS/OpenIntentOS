//! Multi-provider LLM client.
//!
//! Supports the **Anthropic Messages API**, the **OpenAI Chat Completions
//! API** (including OpenAI-compatible endpoints such as Ollama, Together, and
//! vLLM), and the **ChatGPT Web API** (for Pro subscribers) with both
//! streaming SSE and non-streaming modes.

use std::sync::{Arc, RwLock};

use crate::error::{AgentError, Result};
use crate::llm::types::{ChatRequest, LlmResponse, Usage};

// Re-export sub-modules used by the rest of the crate.
pub use crate::llm::openai::{messages_to_openai, parse_response as parse_openai_response, tools_to_openai};

// ---------------------------------------------------------------------------
// Provider enum
// ---------------------------------------------------------------------------

/// Identifies which LLM provider the client should target.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LlmProvider {
    /// Anthropic Messages API.
    Anthropic,
    /// OpenAI Chat Completions API (also covers OpenAI-compatible endpoints).
    OpenAI,
    /// ChatGPT Web API (for Pro subscribers, session-token based).
    ChatGptWeb,
}

// ---------------------------------------------------------------------------
// Client configuration
// ---------------------------------------------------------------------------

/// Configuration for connecting to a single LLM provider endpoint.
#[derive(Debug, Clone)]
pub struct LlmClientConfig {
    pub provider: LlmProvider,
    pub api_key: String,
    pub base_url: String,
    pub default_model: String,
    pub max_tokens: u32,
}

impl LlmClientConfig {
    /// Create a configuration for the Anthropic Claude API.
    pub fn anthropic(api_key: impl Into<String>, model: impl Into<String>) -> Self {
        Self {
            provider: LlmProvider::Anthropic,
            api_key: api_key.into(),
            base_url: crate::llm::anthropic::BASE_URL.to_owned(),
            default_model: model.into(),
            max_tokens: 4096,
        }
    }

    /// Create a configuration for the OpenAI API.
    pub fn openai(api_key: impl Into<String>, model: impl Into<String>) -> Self {
        Self {
            provider: LlmProvider::OpenAI,
            api_key: api_key.into(),
            base_url: crate::llm::openai::BASE_URL.to_owned(),
            default_model: model.into(),
            max_tokens: 4096,
        }
    }

    /// Create a configuration for any OpenAI-compatible API.
    pub fn openai_compatible(
        api_key: impl Into<String>,
        model: impl Into<String>,
        base_url: impl Into<String>,
    ) -> Self {
        Self {
            provider: LlmProvider::OpenAI,
            api_key: api_key.into(),
            base_url: base_url.into(),
            default_model: model.into(),
            max_tokens: 4096,
        }
    }

    /// Create a configuration for the ChatGPT Web API (Pro subscribers).
    ///
    /// The `session_token` is the `__Secure-next-auth.session-token` cookie
    /// value from a logged-in chatgpt.com session.
    pub fn chatgpt_web(
        session_token: impl Into<String>,
        model: impl Into<String>,
    ) -> Self {
        Self {
            provider: LlmProvider::ChatGptWeb,
            api_key: session_token.into(),
            base_url: crate::llm::chatgpt_web::BASE_URL.to_owned(),
            default_model: model.into(),
            max_tokens: 16384,
        }
    }
}

// ---------------------------------------------------------------------------
// Client
// ---------------------------------------------------------------------------

/// An LLM client that communicates with Anthropic, OpenAI, or ChatGPT Web.
#[derive(Clone)]
pub struct LlmClient {
    config: Arc<LlmClientConfig>,
    overrides: Arc<RwLock<RuntimeOverrides>>,
    http: reqwest::Client,
    /// ChatGPT Web auth manager (only set when provider is ChatGptWeb).
    chatgpt_auth: Option<Arc<crate::llm::chatgpt_web::ChatGptWebAuth>>,
    /// Optional browser adapter for proxying ChatGPT Web requests through
    /// Chrome (avoids Cloudflare TLS fingerprint blocks).
    browser_proxy: Arc<tokio::sync::RwLock<Option<Arc<dyn crate::runtime::ToolAdapter>>>>,
}

impl std::fmt::Debug for LlmClient {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("LlmClient")
            .field("config", &self.config)
            .field("chatgpt_auth", &self.chatgpt_auth.is_some())
            .field("browser_proxy", &"<...>")
            .finish()
    }
}

/// Mutable runtime overrides for the LLM client.
#[derive(Debug, Clone)]
struct RuntimeOverrides {
    api_key: String,
    provider: Option<LlmProvider>,
    base_url: Option<String>,
    default_model: Option<String>,
}

impl LlmClient {
    /// Create a new client with the given configuration.
    pub fn new(config: LlmClientConfig) -> Result<Self> {
        // ChatGPT Web uses session tokens, not API keys — allow "empty" API
        // key check to pass for it.
        if config.api_key.is_empty() && config.provider != LlmProvider::ChatGptWeb {
            let provider_name = match config.provider {
                LlmProvider::Anthropic => "anthropic",
                LlmProvider::OpenAI => "openai",
                LlmProvider::ChatGptWeb => "chatgpt-web",
            };
            return Err(AgentError::MissingApiKey {
                provider: provider_name.into(),
            });
        }

        let http = reqwest::Client::builder()
            .timeout(std::time::Duration::from_secs(120))
            .build()
            .map_err(|e| AgentError::LlmRequestFailed {
                reason: format!("failed to build HTTP client: {e}"),
            })?;

        // Initialize ChatGPT Web auth if applicable.
        let chatgpt_auth = if config.provider == LlmProvider::ChatGptWeb {
            if config.api_key.is_empty() {
                return Err(AgentError::MissingApiKey {
                    provider: "chatgpt-web (CHATGPT_SESSION_TOKEN)".into(),
                });
            }
            Some(Arc::new(crate::llm::chatgpt_web::ChatGptWebAuth::new(
                config.api_key.clone(),
                config.base_url.clone(),
            )))
        } else {
            None
        };

        let overrides = Arc::new(RwLock::new(RuntimeOverrides {
            api_key: config.api_key.clone(),
            provider: None,
            base_url: None,
            default_model: None,
        }));

        Ok(Self {
            config: Arc::new(config),
            overrides,
            http,
            chatgpt_auth,
            browser_proxy: Arc::new(tokio::sync::RwLock::new(None)),
        })
    }

    /// Set a browser adapter to proxy ChatGPT Web requests through Chrome.
    pub async fn set_browser_proxy(&self, adapter: Arc<dyn crate::runtime::ToolAdapter>) {
        *self.browser_proxy.write().await = Some(adapter);
    }

    /// Returns the current provider (respects runtime overrides).
    pub fn provider(&self) -> LlmProvider {
        self.overrides
            .read()
            .ok()
            .and_then(|o| o.provider.clone())
            .unwrap_or_else(|| self.config.provider.clone())
    }

    /// Hot-swap the API key at runtime.
    pub fn update_api_key(&self, new_key: String) {
        if let Ok(mut o) = self.overrides.write() {
            o.api_key = new_key;
        }
    }

    /// Switch to a different provider at runtime (failover).
    pub fn switch_provider(
        &self,
        provider: LlmProvider,
        base_url: String,
        default_model: String,
    ) {
        if let Ok(mut o) = self.overrides.write() {
            o.provider = Some(provider);
            o.base_url = Some(base_url);
            o.default_model = Some(default_model);
        }
    }

    /// Create a failover chain of providers for automatic fallback.
    pub fn create_failover_chain() -> Vec<(LlmProvider, String, String)> {
        vec![
            (
                LlmProvider::Anthropic,
                "https://api.anthropic.com".to_string(),
                "claude-sonnet-4-20250514".to_string(),
            ),
            (
                LlmProvider::OpenAI,
                "https://api.openai.com/v1".to_string(),
                "gpt-4o".to_string(),
            ),
            (
                LlmProvider::OpenAI,
                "https://api.deepseek.com/v1".to_string(),
                "deepseek-chat".to_string(),
            ),
            (
                LlmProvider::OpenAI,
                "http://localhost:11434/v1".to_string(),
                "qwen2.5:latest".to_string(),
            ),
        ]
    }

    /// Attempt to failover to the next provider in the chain.
    pub async fn attempt_failover(&self, current_provider_index: usize) -> bool {
        let chain = Self::create_failover_chain();
        if current_provider_index + 1 >= chain.len() {
            tracing::warn!("Failover chain exhausted, no more providers available");
            return false;
        }
        let (provider, base_url, model) = &chain[current_provider_index + 1];
        tracing::info!(
            provider = ?provider,
            base_url = %base_url,
            model = %model,
            "Failing over to next provider"
        );
        self.switch_provider(provider.clone(), base_url.clone(), model.clone());
        true
    }

    /// Resolve the API key for a provider URL from environment variables.
    fn env_api_key_for_url(base_url: &str) -> String {
        if base_url.contains("anthropic.com") {
            std::env::var("ANTHROPIC_API_KEY").unwrap_or_default()
        } else if base_url.contains("openai.com") {
            std::env::var("OPENAI_API_KEY").unwrap_or_default()
        } else if base_url.contains("deepseek.com") {
            std::env::var("DEEPSEEK_API_KEY").unwrap_or_default()
        } else if base_url.contains("generativelanguage.googleapis.com")
            || base_url.contains("googleapis.com")
        {
            std::env::var("GOOGLE_API_KEY").unwrap_or_default()
        } else if base_url.contains("api.nvidia.com") {
            std::env::var("NVIDIA_API_KEY").unwrap_or_default()
        } else if base_url.contains("chatgpt.com") {
            std::env::var("CHATGPT_SESSION_TOKEN").unwrap_or_default()
        } else {
            String::new()
        }
    }

    /// Switch to the next available provider when the current one returns a
    /// quota / rate-limit error (HTTP 429).
    pub fn failover_on_quota(&self) -> bool {
        let chain = Self::create_failover_chain();
        let current_url = self.current_base_url();

        for (provider, base_url, model) in &chain {
            if *base_url == current_url {
                continue;
            }

            let api_key = Self::env_api_key_for_url(base_url);
            let is_local = base_url.contains("localhost") || base_url.contains("127.0.0.1");

            if api_key.is_empty() && !is_local {
                tracing::debug!(
                    url = %base_url,
                    "skipping failover candidate: no API key in environment"
                );
                continue;
            }

            tracing::warn!(
                from = %current_url,
                to   = %base_url,
                model = %model,
                "quota exceeded — failing over to next provider"
            );

            if let Ok(mut o) = self.overrides.write() {
                o.provider = Some(provider.clone());
                o.base_url = Some(base_url.clone());
                o.default_model = Some(model.clone());
                o.api_key = api_key;
            }
            return true;
        }

        tracing::warn!("failover chain exhausted: no provider with a valid API key found");
        false
    }

    /// Reset all runtime overrides back to the original config defaults.
    pub fn restore_defaults(&self) {
        if let Ok(mut o) = self.overrides.write() {
            o.provider = None;
            o.base_url = None;
            o.default_model = None;
            o.api_key = self.config.api_key.clone();
        }
    }

    /// Read the current API key (snapshot).
    fn current_api_key(&self) -> String {
        self.overrides
            .read()
            .map(|o| o.api_key.clone())
            .unwrap_or_else(|_| self.config.api_key.clone())
    }

    /// Read the current base URL (snapshot, respects overrides).
    pub fn current_base_url(&self) -> String {
        self.overrides
            .read()
            .ok()
            .and_then(|o| o.base_url.clone())
            .unwrap_or_else(|| self.config.base_url.clone())
    }

    /// Read the current default model (snapshot, respects overrides).
    fn current_default_model(&self) -> String {
        self.overrides
            .read()
            .ok()
            .and_then(|o| o.default_model.clone())
            .unwrap_or_else(|| self.config.default_model.clone())
    }

    /// Resolve the effective model for a request.
    fn effective_model(&self, request: &ChatRequest) -> String {
        if request.model.is_empty() {
            self.current_default_model()
        } else {
            request.model.clone()
        }
    }

    /// Resolve the effective max_tokens for a request.
    fn effective_max_tokens(&self, request: &ChatRequest) -> u32 {
        request.max_tokens.unwrap_or(self.config.max_tokens)
    }

    // -----------------------------------------------------------------------
    // Public API
    // -----------------------------------------------------------------------

    /// Send a chat request and return the full response (non-streaming).
    pub async fn chat(&self, request: &ChatRequest) -> Result<LlmResponse> {
        match self.provider() {
            LlmProvider::Anthropic => self.chat_anthropic(request).await,
            LlmProvider::OpenAI => self.chat_openai(request).await,
            LlmProvider::ChatGptWeb => self.chat_chatgpt_web(request).await,
        }
    }

    /// Send a chat request using streaming SSE (no callback).
    pub async fn stream_chat(&self, request: &ChatRequest) -> Result<(LlmResponse, Usage)> {
        match self.provider() {
            LlmProvider::Anthropic => {
                crate::llm::anthropic::consume_stream(
                    self.send_anthropic(request, true).await?,
                    &mut |_| {},
                )
                .await
            }
            LlmProvider::OpenAI => {
                self.stream_chat_with_callback(request, |_| {}).await
            }
            LlmProvider::ChatGptWeb => {
                self.stream_chat_with_callback(request, |_| {}).await
            }
        }
    }

    /// Send a chat request using streaming SSE, invoking a callback for each
    /// text delta.
    pub async fn stream_chat_with_callback<F>(
        &self,
        request: &ChatRequest,
        mut on_text: F,
    ) -> Result<(LlmResponse, Usage)>
    where
        F: FnMut(&str) + Send,
    {
        match self.provider() {
            LlmProvider::Anthropic => {
                let resp = self.send_anthropic(request, true).await?;
                crate::llm::anthropic::consume_stream(resp, &mut on_text).await
            }
            LlmProvider::OpenAI => {
                let resp = self.send_openai(request, true).await?;
                crate::llm::openai::consume_stream(resp, &mut on_text).await
            }
            LlmProvider::ChatGptWeb => {
                // Prefer browser proxy to bypass Cloudflare TLS fingerprinting.
                let has_browser = self.browser_proxy.read().await.is_some();
                if has_browser {
                    let raw_sse = self.chatgpt_web_via_browser(request).await?;
                    let has_tools = !request.tools.is_empty();
                    crate::llm::chatgpt_web::parse_sse_text(&raw_sse, &mut on_text, has_tools)
                } else {
                    let resp = self.send_chatgpt_web(request).await?;
                    let has_tools = !request.tools.is_empty();
                    crate::llm::chatgpt_web::consume_stream(resp, &mut on_text, has_tools)
                        .await
                }
            }
        }
    }

    // -----------------------------------------------------------------------
    // Provider-specific send helpers
    // -----------------------------------------------------------------------

    async fn send_anthropic(
        &self,
        request: &ChatRequest,
        stream: bool,
    ) -> Result<reqwest::Response> {
        let model = self.effective_model(request);
        let max_tokens = self.effective_max_tokens(request);
        let body = crate::llm::anthropic::build_request_body(
            &request.messages,
            &model,
            max_tokens,
            request.temperature,
            &request.tools,
            stream,
        );
        let resp = crate::llm::anthropic::send_request(
            &self.http,
            &self.current_base_url(),
            &self.current_api_key(),
            &body,
        )
        .await?;

        let status = resp.status();
        if !status.is_success() {
            let text = resp.text().await.unwrap_or_default();
            return Err(AgentError::LlmRequestFailed {
                reason: format!("API returned {status}: {text}"),
            });
        }
        Ok(resp)
    }

    async fn send_openai(
        &self,
        request: &ChatRequest,
        stream: bool,
    ) -> Result<reqwest::Response> {
        let model = self.effective_model(request);
        let max_tokens = self.effective_max_tokens(request);
        let body = crate::llm::openai::build_request_body(
            &request.messages,
            &model,
            max_tokens,
            request.temperature,
            &request.tools,
            stream,
        );
        let resp = crate::llm::openai::send_request(
            &self.http,
            &self.current_base_url(),
            &self.current_api_key(),
            &body,
        )
        .await?;

        let status = resp.status();
        if !status.is_success() {
            let text = resp.text().await.unwrap_or_default();
            return Err(AgentError::LlmRequestFailed {
                reason: format!("API returned {status}: {text}"),
            });
        }
        Ok(resp)
    }

    async fn send_chatgpt_web(
        &self,
        request: &ChatRequest,
    ) -> Result<reqwest::Response> {
        let auth = self.chatgpt_auth.as_ref().ok_or_else(|| {
            AgentError::MissingApiKey {
                provider: "chatgpt-web (no auth initialized)".into(),
            }
        })?;
        let model = self.effective_model(request);
        let body = crate::llm::chatgpt_web::build_request_body(request, &model);
        let resp = crate::llm::chatgpt_web::send_request(
            &self.http,
            auth,
            &self.current_base_url(),
            &body,
        )
        .await?;

        let status = resp.status();
        if !status.is_success() {
            let text = resp.text().await.unwrap_or_default();
            return Err(AgentError::LlmRequestFailed {
                reason: format!("ChatGPT web API returned {status}: {text}"),
            });
        }
        Ok(resp)
    }

    /// Send a ChatGPT Web request via the browser adapter (Chrome fetch proxy).
    /// Collects all SSE events in the browser and returns them as raw text.
    async fn chatgpt_web_via_browser(
        &self,
        request: &ChatRequest,
    ) -> Result<String> {
        let auth = self.chatgpt_auth.as_ref().ok_or_else(|| {
            AgentError::MissingApiKey {
                provider: "chatgpt-web (no auth initialized)".into(),
            }
        })?;
        let browser = self.browser_proxy.read().await.clone()
            .ok_or_else(|| AgentError::LlmRequestFailed {
                reason: "browser proxy not available".into(),
            })?;
        let model = self.effective_model(request);
        let body = crate::llm::chatgpt_web::build_request_body(request, &model);
        crate::llm::chatgpt_web::fetch_via_browser(
            &browser,
            auth,
            &self.current_base_url(),
            &body,
        )
        .await
    }

    // -----------------------------------------------------------------------
    // Non-streaming helpers
    // -----------------------------------------------------------------------

    async fn chat_anthropic(&self, request: &ChatRequest) -> Result<LlmResponse> {
        let model = self.effective_model(request);
        let max_tokens = self.effective_max_tokens(request);
        let body = crate::llm::anthropic::build_request_body(
            &request.messages,
            &model,
            max_tokens,
            request.temperature,
            &request.tools,
            false,
        );
        let resp = crate::llm::anthropic::send_request(
            &self.http,
            &self.current_base_url(),
            &self.current_api_key(),
            &body,
        )
        .await?;

        let status = resp.status();
        let text = resp
            .text()
            .await
            .map_err(|e| AgentError::LlmRequestFailed {
                reason: format!("failed to read response body: {e}"),
            })?;
        if !status.is_success() {
            return Err(AgentError::LlmRequestFailed {
                reason: format!("API returned {status}: {text}"),
            });
        }
        let v: serde_json::Value =
            serde_json::from_str(&text).map_err(|e| AgentError::LlmParseFailed {
                reason: format!("invalid JSON response: {e}"),
            })?;
        crate::llm::anthropic::parse_response(&v)
    }

    async fn chat_openai(&self, request: &ChatRequest) -> Result<LlmResponse> {
        let model = self.effective_model(request);
        let max_tokens = self.effective_max_tokens(request);
        let body = crate::llm::openai::build_request_body(
            &request.messages,
            &model,
            max_tokens,
            request.temperature,
            &request.tools,
            false,
        );
        let resp = crate::llm::openai::send_request(
            &self.http,
            &self.current_base_url(),
            &self.current_api_key(),
            &body,
        )
        .await?;

        let status = resp.status();
        let text = resp
            .text()
            .await
            .map_err(|e| AgentError::LlmRequestFailed {
                reason: format!("failed to read response body: {e}"),
            })?;
        if !status.is_success() {
            return Err(AgentError::LlmRequestFailed {
                reason: format!("API returned {status}: {text}"),
            });
        }
        let v: serde_json::Value =
            serde_json::from_str(&text).map_err(|e| AgentError::LlmParseFailed {
                reason: format!("invalid JSON response: {e}"),
            })?;
        crate::llm::openai::parse_response(&v)
    }

    async fn chat_chatgpt_web(&self, request: &ChatRequest) -> Result<LlmResponse> {
        let auth = self.chatgpt_auth.as_ref().ok_or_else(|| {
            AgentError::MissingApiKey {
                provider: "chatgpt-web (no auth initialized)".into(),
            }
        })?;
        let model = self.effective_model(request);
        crate::llm::chatgpt_web::chat(
            &self.http,
            auth,
            &self.current_base_url(),
            request,
            &model,
        )
        .await
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use crate::llm::types::{Message, ToolCall, ToolDefinition};

    #[test]
    fn anthropic_config_construction() {
        let config = LlmClientConfig::anthropic("key", "claude-sonnet-4-20250514");
        let client = LlmClient::new(config).unwrap();
        assert_eq!(client.provider(), LlmProvider::Anthropic);
    }

    #[test]
    fn openai_config_construction() {
        let config = LlmClientConfig::openai("sk-test-key", "gpt-4o");
        assert_eq!(config.provider, LlmProvider::OpenAI);
        assert_eq!(config.api_key, "sk-test-key");
        assert_eq!(config.default_model, "gpt-4o");
    }

    #[test]
    fn openai_compatible_config_construction() {
        let config =
            LlmClientConfig::openai_compatible("local-key", "llama3", "http://localhost:11434/v1");
        assert_eq!(config.provider, LlmProvider::OpenAI);
        assert_eq!(config.base_url, "http://localhost:11434/v1");
    }

    #[test]
    fn chatgpt_web_config_construction() {
        let config = LlmClientConfig::chatgpt_web("session-token-abc", "gpt-4");
        assert_eq!(config.provider, LlmProvider::ChatGptWeb);
        assert_eq!(config.api_key, "session-token-abc");
        assert_eq!(config.base_url, "https://chatgpt.com");
    }

    #[test]
    fn empty_api_key_returns_error() {
        let config = LlmClientConfig::anthropic("", "claude-sonnet-4-20250514");
        assert!(LlmClient::new(config).is_err());

        let config = LlmClientConfig::openai("", "gpt-4o");
        assert!(LlmClient::new(config).is_err());
    }

    #[test]
    fn chatgpt_web_empty_session_token_returns_error() {
        let config = LlmClientConfig::chatgpt_web("", "gpt-4");
        assert!(LlmClient::new(config).is_err());
    }

    #[test]
    fn provider_detection() {
        let c1 = LlmClientConfig::anthropic("key", "claude-sonnet-4-20250514");
        assert_eq!(LlmClient::new(c1).unwrap().provider(), LlmProvider::Anthropic);

        let c2 = LlmClientConfig::openai("key", "gpt-4o");
        assert_eq!(LlmClient::new(c2).unwrap().provider(), LlmProvider::OpenAI);

        let c3 = LlmClientConfig::chatgpt_web("token", "gpt-4");
        assert_eq!(LlmClient::new(c3).unwrap().provider(), LlmProvider::ChatGptWeb);
    }

    #[test]
    fn llm_provider_equality() {
        assert_eq!(LlmProvider::Anthropic, LlmProvider::Anthropic);
        assert_eq!(LlmProvider::OpenAI, LlmProvider::OpenAI);
        assert_eq!(LlmProvider::ChatGptWeb, LlmProvider::ChatGptWeb);
        assert_ne!(LlmProvider::Anthropic, LlmProvider::OpenAI);
        assert_ne!(LlmProvider::OpenAI, LlmProvider::ChatGptWeb);
    }

    // -- Anthropic format tests (delegate to anthropic module) ----------------

    #[test]
    fn build_anthropic_request_body_basic() {
        let msgs = vec![Message::system("You are helpful."), Message::user("Hello")];
        let body =
            crate::llm::anthropic::build_request_body(&msgs, "claude-sonnet-4-20250514", 1024, Some(0.7), &[], false);

        assert_eq!(body["model"], "claude-sonnet-4-20250514");
        assert_eq!(body["system"], "You are helpful.");
        assert_eq!(body["max_tokens"], 1024);
        assert!(body.get("stream").is_none());
    }

    #[test]
    fn build_anthropic_request_body_with_tools() {
        let msgs = vec![Message::user("Read file.txt")];
        let tools = vec![ToolDefinition {
            name: "read_file".into(),
            description: "Read a file".into(),
            input_schema: serde_json::json!({
                "type": "object",
                "properties": {"path": {"type": "string"}},
                "required": ["path"]
            }),
        }];
        let body = crate::llm::anthropic::build_request_body(
            &msgs,
            "claude-sonnet-4-20250514",
            4096,
            None,
            &tools,
            true,
        );
        assert_eq!(body["stream"], true);
        assert!(body["tools"].is_array());
        assert_eq!(body["tools"][0]["name"], "read_file");
    }

    #[test]
    fn parse_non_streaming_anthropic_text_response() {
        let v: serde_json::Value = serde_json::json!({
            "id": "msg_01",
            "type": "message",
            "role": "assistant",
            "content": [{"type": "text", "text": "Hello, world!"}],
            "stop_reason": "end_turn",
            "usage": {"input_tokens": 10, "output_tokens": 5}
        });
        let result = crate::llm::anthropic::parse_response(&v).unwrap();
        match result {
            LlmResponse::Text(text) => assert_eq!(text, "Hello, world!"),
            _ => panic!("expected Text response"),
        }
    }

    // -- OpenAI format tests (delegate to openai module) ----------------------

    #[test]
    fn messages_to_openai_system_message() {
        let msgs = vec![Message::system("You are helpful."), Message::user("Hello")];
        let wire = messages_to_openai(&msgs);
        assert_eq!(wire.len(), 2);
        assert_eq!(wire[0]["role"], "system");
        assert_eq!(wire[1]["role"], "user");
    }

    #[test]
    fn messages_to_openai_tool_calls() {
        let msgs = vec![Message::assistant_tool_calls(vec![ToolCall {
            id: "call_abc".into(),
            name: "read_file".into(),
            arguments: serde_json::json!({"path": "test.txt"}),
        }])];
        let wire = messages_to_openai(&msgs);
        assert_eq!(wire[0]["tool_calls"][0]["function"]["name"], "read_file");
    }

    #[test]
    fn build_openai_request_body_basic() {
        let msgs = vec![Message::system("You are helpful."), Message::user("Hello")];
        let body = crate::llm::openai::build_request_body(
            &msgs, "gpt-4o", 2048, Some(0.5), &[], false,
        );
        assert_eq!(body["model"], "gpt-4o");
        assert_eq!(body["max_tokens"], 2048);
    }

    #[test]
    fn parse_openai_text_response() {
        let v: serde_json::Value = serde_json::json!({
            "choices": [{"message": {"role": "assistant", "content": "Hello!"}}]
        });
        let result = parse_openai_response(&v).unwrap();
        match result {
            LlmResponse::Text(text) => assert_eq!(text, "Hello!"),
            _ => panic!("expected Text"),
        }
    }

    #[test]
    fn tools_to_openai_format() {
        let tools = vec![ToolDefinition {
            name: "read_file".into(),
            description: "Read a file".into(),
            input_schema: serde_json::json!({"type": "object"}),
        }];
        let wire = tools_to_openai(&tools);
        assert_eq!(wire[0]["type"], "function");
        assert_eq!(wire[0]["function"]["name"], "read_file");
    }
}
