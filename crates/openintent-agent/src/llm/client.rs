//! Multi-provider LLM client.
//!
//! Supports the **Anthropic Messages API** and the **OpenAI Chat Completions
//! API** (including OpenAI-compatible endpoints such as Ollama, Together, and
//! vLLM) with both streaming SSE and non-streaming modes.

use std::sync::{Arc, RwLock};

use futures::StreamExt;
use reqwest::header::{AUTHORIZATION, CONTENT_TYPE, HeaderMap, HeaderValue};
use serde_json::{Value, json};

use crate::error::{AgentError, Result};
use crate::llm::streaming::SseParser;
use crate::llm::streaming_openai::OpenAiStreamAccumulator;
use crate::llm::types::{
    ChatRequest, LlmResponse, Message, Role, StreamDelta, StreamEvent, ToolCall, ToolDefinition,
    Usage,
};

// ---------------------------------------------------------------------------
// Constants
// ---------------------------------------------------------------------------

/// Default Anthropic API base URL.
const ANTHROPIC_BASE_URL: &str = "https://api.anthropic.com";

/// Default OpenAI API base URL.
const OPENAI_BASE_URL: &str = "https://api.openai.com/v1";

/// Anthropic API version header value.
const ANTHROPIC_VERSION: &str = "2023-06-01";

/// Anthropic beta header required for OAuth token authentication.
const ANTHROPIC_OAUTH_BETA: &str = "oauth-2025-04-20";

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
}

// ---------------------------------------------------------------------------
// Client configuration
// ---------------------------------------------------------------------------

/// Configuration for connecting to a single LLM provider endpoint.
#[derive(Debug, Clone)]
pub struct LlmClientConfig {
    /// Which provider this configuration targets.
    pub provider: LlmProvider,
    /// API key for authentication.
    pub api_key: String,
    /// Base URL for the API (e.g. `https://api.anthropic.com`).
    pub base_url: String,
    /// Default model identifier.
    pub default_model: String,
    /// Default maximum tokens per response.
    pub max_tokens: u32,
}

impl LlmClientConfig {
    /// Create a configuration for the Anthropic Claude API.
    pub fn anthropic(api_key: impl Into<String>, model: impl Into<String>) -> Self {
        Self {
            provider: LlmProvider::Anthropic,
            api_key: api_key.into(),
            base_url: ANTHROPIC_BASE_URL.to_owned(),
            default_model: model.into(),
            max_tokens: 4096,
        }
    }

    /// Create a configuration for the OpenAI API.
    pub fn openai(api_key: impl Into<String>, model: impl Into<String>) -> Self {
        Self {
            provider: LlmProvider::OpenAI,
            api_key: api_key.into(),
            base_url: OPENAI_BASE_URL.to_owned(),
            default_model: model.into(),
            max_tokens: 4096,
        }
    }

    /// Create a configuration for any OpenAI-compatible API (e.g. Ollama,
    /// Together, vLLM).
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
}

// ---------------------------------------------------------------------------
// Client
// ---------------------------------------------------------------------------

/// An LLM client that communicates with either the Anthropic Messages API or
/// the OpenAI Chat Completions API.
///
/// Supports both streaming and non-streaming modes, tool use, and system
/// prompts.  The API key can be hot-swapped at runtime (e.g. after an OAuth
/// token refresh) via [`update_api_key`] and provider failover via
/// [`switch_provider`].
#[derive(Debug, Clone)]
pub struct LlmClient {
    config: Arc<LlmClientConfig>,
    /// Swappable runtime overrides — allows token refresh and provider failover
    /// without re-creating the client.
    overrides: Arc<RwLock<RuntimeOverrides>>,
    http: reqwest::Client,
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
        if config.api_key.is_empty() {
            let provider_name = match config.provider {
                LlmProvider::Anthropic => "anthropic",
                LlmProvider::OpenAI => "openai",
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
        })
    }

    /// Returns the current provider (respects runtime overrides).
    pub fn provider(&self) -> LlmProvider {
        self.overrides
            .read()
            .ok()
            .and_then(|o| o.provider.clone())
            .unwrap_or_else(|| self.config.provider.clone())
    }

    /// Hot-swap the API key at runtime (e.g. after an OAuth token refresh).
    pub fn update_api_key(&self, new_key: String) {
        if let Ok(mut o) = self.overrides.write() {
            o.api_key = new_key;
        }
    }

    /// Switch to a different provider at runtime (e.g. failover from
    /// Anthropic to DeepSeek when the OAuth token cannot be refreshed).
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

    /// Read the current API key (snapshot).
    fn current_api_key(&self) -> String {
        self.overrides
            .read()
            .map(|o| o.api_key.clone())
            .unwrap_or_else(|_| self.config.api_key.clone())
    }

    /// Read the current base URL (snapshot, respects overrides).
    fn current_base_url(&self) -> String {
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

    // -----------------------------------------------------------------------
    // Public API
    // -----------------------------------------------------------------------

    /// Send a chat request and return the full response (non-streaming).
    ///
    /// This blocks until the entire response is received and then parses it
    /// into an [`LlmResponse`].
    pub async fn chat(&self, request: &ChatRequest) -> Result<LlmResponse> {
        match self.provider() {
            LlmProvider::Anthropic => self.chat_anthropic(request).await,
            LlmProvider::OpenAI => self.chat_openai(request).await,
        }
    }

    /// Send a chat request using streaming SSE and return the aggregated
    /// response.
    ///
    /// Internally consumes the SSE stream, accumulating text and tool-call
    /// fragments until the message is complete.
    pub async fn stream_chat(&self, request: &ChatRequest) -> Result<LlmResponse> {
        match self.provider() {
            LlmProvider::Anthropic => self.stream_chat_anthropic(request).await,
            LlmProvider::OpenAI => self.stream_chat_openai(request, &mut |_| {}).await,
        }
    }

    /// Send a chat request using streaming SSE, invoking a callback for each
    /// text delta so callers can render incremental output.
    pub async fn stream_chat_with_callback<F>(
        &self,
        request: &ChatRequest,
        mut on_text: F,
    ) -> Result<LlmResponse>
    where
        F: FnMut(&str) + Send,
    {
        match self.provider() {
            LlmProvider::Anthropic => {
                self.stream_chat_anthropic_with_callback(request, &mut on_text)
                    .await
            }
            LlmProvider::OpenAI => self.stream_chat_openai(request, &mut on_text).await,
        }
    }

    // =======================================================================
    // Anthropic implementation
    // =======================================================================

    /// Non-streaming Anthropic chat.
    async fn chat_anthropic(&self, request: &ChatRequest) -> Result<LlmResponse> {
        let body = self.build_anthropic_request_body(request, false);
        let resp = self.send_anthropic_request(&body).await?;

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

        let v: Value = serde_json::from_str(&text).map_err(|e| AgentError::LlmParseFailed {
            reason: format!("invalid JSON response: {e}"),
        })?;

        parse_anthropic_response(&v)
    }

    /// Streaming Anthropic chat (no callback).
    async fn stream_chat_anthropic(&self, request: &ChatRequest) -> Result<LlmResponse> {
        self.stream_chat_anthropic_with_callback(request, &mut |_| {})
            .await
    }

    /// Streaming Anthropic chat with a text callback.
    async fn stream_chat_anthropic_with_callback<F>(
        &self,
        request: &ChatRequest,
        on_text: &mut F,
    ) -> Result<LlmResponse>
    where
        F: FnMut(&str),
    {
        let body = self.build_anthropic_request_body(request, true);
        let resp = self.send_anthropic_request(&body).await?;

        let status = resp.status();
        if !status.is_success() {
            let text = resp.text().await.unwrap_or_default();
            return Err(AgentError::LlmRequestFailed {
                reason: format!("API returned {status}: {text}"),
            });
        }

        self.consume_anthropic_stream(resp, on_text).await
    }

    // -- Anthropic request building ------------------------------------------

    /// Build the JSON body for the Anthropic Messages API.
    fn build_anthropic_request_body(&self, request: &ChatRequest, stream: bool) -> Value {
        let (system_text, messages) = messages_to_anthropic(&request.messages);
        let default_model = self.current_default_model();

        let mut body = json!({
            "model": if request.model.is_empty() {
                &default_model
            } else {
                &request.model
            },
            "max_tokens": request.max_tokens.unwrap_or(self.config.max_tokens),
            "messages": messages,
        });

        if let Some(system) = system_text {
            body["system"] = json!(system);
        }

        if let Some(temp) = request.temperature {
            body["temperature"] = json!(temp);
        }

        if !request.tools.is_empty() {
            body["tools"] = tools_to_anthropic(&request.tools);
        }

        if stream {
            body["stream"] = json!(true);
        }

        body
    }

    /// Send the HTTP request to the Anthropic Messages API endpoint.
    ///
    /// Supports both standard API keys (`x-api-key` header) and OAuth tokens
    /// (`Authorization: Bearer` header).  OAuth tokens are detected by their
    /// `sk-ant-oat` prefix.
    async fn send_anthropic_request(&self, body: &Value) -> Result<reqwest::Response> {
        let url = format!("{}/v1/messages", self.current_base_url());

        let mut headers = HeaderMap::new();

        // Snapshot the current API key (may have been refreshed at runtime).
        let api_key = self.current_api_key();

        // OAuth tokens (from Claude Code) use Bearer auth + the oauth beta
        // header; regular API keys use the x-api-key header.
        let is_oauth = api_key.starts_with("sk-ant-oat");
        if is_oauth {
            headers.insert(
                AUTHORIZATION,
                HeaderValue::from_str(&format!("Bearer {api_key}")).map_err(|e| {
                    AgentError::LlmRequestFailed {
                        reason: format!("invalid authorization header: {e}"),
                    }
                })?,
            );
            headers.insert(
                "anthropic-beta",
                HeaderValue::from_static(ANTHROPIC_OAUTH_BETA),
            );
        } else {
            headers.insert(
                "x-api-key",
                HeaderValue::from_str(&api_key).map_err(|e| {
                    AgentError::LlmRequestFailed {
                        reason: format!("invalid API key header: {e}"),
                    }
                })?,
            );
        }

        headers.insert(
            "anthropic-version",
            HeaderValue::from_static(ANTHROPIC_VERSION),
        );
        headers.insert(CONTENT_TYPE, HeaderValue::from_static("application/json"));

        tracing::debug!(url = %url, model = %body["model"], provider = "anthropic", is_oauth = is_oauth, "sending LLM request");

        self.http
            .post(&url)
            .headers(headers)
            .json(body)
            .send()
            .await
            .map_err(|e| AgentError::LlmRequestFailed {
                reason: e.to_string(),
            })
    }

    // -- Anthropic streaming -------------------------------------------------

    /// Consume an Anthropic SSE stream and aggregate into a final response.
    async fn consume_anthropic_stream<F>(
        &self,
        resp: reqwest::Response,
        on_text: &mut F,
    ) -> Result<LlmResponse>
    where
        F: FnMut(&str),
    {
        let mut parser = SseParser::new();
        let mut accumulator = StreamAccumulator::new();

        let mut byte_stream = resp.bytes_stream();
        let mut line_buffer = String::new();

        while let Some(chunk_result) = byte_stream.next().await {
            let chunk = chunk_result.map_err(|e| AgentError::LlmStreamError {
                reason: format!("stream read error: {e}"),
            })?;

            let text = std::str::from_utf8(&chunk).map_err(|e| AgentError::LlmStreamError {
                reason: format!("invalid UTF-8 in stream: {e}"),
            })?;

            line_buffer.push_str(text);

            while let Some(newline_pos) = line_buffer.find('\n') {
                let line = line_buffer[..newline_pos].to_owned();
                line_buffer = line_buffer[newline_pos + 1..].to_owned();

                if let Some(event) = parser.parse_line(&line)? {
                    accumulator.apply(&event, on_text);

                    if matches!(event, StreamEvent::MessageStop) {
                        return accumulator.into_response();
                    }
                }
            }
        }

        accumulator.into_response()
    }

    // =======================================================================
    // OpenAI implementation
    // =======================================================================

    /// Non-streaming OpenAI chat.
    async fn chat_openai(&self, request: &ChatRequest) -> Result<LlmResponse> {
        let body = self.build_openai_request_body(request, false);
        let resp = self.send_openai_request(&body).await?;

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

        let v: Value = serde_json::from_str(&text).map_err(|e| AgentError::LlmParseFailed {
            reason: format!("invalid JSON response: {e}"),
        })?;

        parse_openai_response(&v)
    }

    /// Streaming OpenAI chat with a text callback.
    async fn stream_chat_openai<F>(
        &self,
        request: &ChatRequest,
        on_text: &mut F,
    ) -> Result<LlmResponse>
    where
        F: FnMut(&str),
    {
        let body = self.build_openai_request_body(request, true);
        let resp = self.send_openai_request(&body).await?;

        let status = resp.status();
        if !status.is_success() {
            let text = resp.text().await.unwrap_or_default();
            return Err(AgentError::LlmRequestFailed {
                reason: format!("API returned {status}: {text}"),
            });
        }

        self.consume_openai_stream(resp, on_text).await
    }

    // -- OpenAI request building ---------------------------------------------

    /// Build the JSON body for the OpenAI Chat Completions API.
    fn build_openai_request_body(&self, request: &ChatRequest, stream: bool) -> Value {
        let messages = messages_to_openai(&request.messages);
        let default_model = self.current_default_model();

        let mut body = json!({
            "model": if request.model.is_empty() {
                &default_model
            } else {
                &request.model
            },
            "max_tokens": request.max_tokens.unwrap_or(self.config.max_tokens),
            "messages": messages,
        });

        if let Some(temp) = request.temperature {
            body["temperature"] = json!(temp);
        }

        if !request.tools.is_empty() {
            body["tools"] = tools_to_openai(&request.tools);
        }

        if stream {
            body["stream"] = json!(true);
        }

        body
    }

    /// Send the HTTP request to the OpenAI Chat Completions API endpoint.
    async fn send_openai_request(&self, body: &Value) -> Result<reqwest::Response> {
        let url = format!("{}/chat/completions", self.current_base_url());

        let mut headers = HeaderMap::new();
        let auth_value = format!("Bearer {}", self.current_api_key());
        headers.insert(
            AUTHORIZATION,
            HeaderValue::from_str(&auth_value).map_err(|e| AgentError::LlmRequestFailed {
                reason: format!("invalid authorization header: {e}"),
            })?,
        );
        headers.insert(CONTENT_TYPE, HeaderValue::from_static("application/json"));

        tracing::debug!(url = %url, model = %body["model"], provider = "openai", "sending LLM request");

        self.http
            .post(&url)
            .headers(headers)
            .json(body)
            .send()
            .await
            .map_err(|e| AgentError::LlmRequestFailed {
                reason: e.to_string(),
            })
    }

    // -- OpenAI streaming ----------------------------------------------------

    /// Consume an OpenAI SSE stream and aggregate into a final response.
    async fn consume_openai_stream<F>(
        &self,
        resp: reqwest::Response,
        on_text: &mut F,
    ) -> Result<LlmResponse>
    where
        F: FnMut(&str),
    {
        let mut accumulator = OpenAiStreamAccumulator::new();

        let mut byte_stream = resp.bytes_stream();
        let mut line_buffer = String::new();

        while let Some(chunk_result) = byte_stream.next().await {
            let chunk = chunk_result.map_err(|e| AgentError::LlmStreamError {
                reason: format!("stream read error: {e}"),
            })?;

            let text = std::str::from_utf8(&chunk).map_err(|e| AgentError::LlmStreamError {
                reason: format!("invalid UTF-8 in stream: {e}"),
            })?;

            line_buffer.push_str(text);

            while let Some(newline_pos) = line_buffer.find('\n') {
                let line = line_buffer[..newline_pos].to_owned();
                line_buffer = line_buffer[newline_pos + 1..].to_owned();

                if let Some(delta_text) = accumulator.feed_line(&line)? {
                    on_text(&delta_text);
                }

                if accumulator.is_done() {
                    return accumulator.into_response();
                }
            }
        }

        accumulator.into_response()
    }
}

// ===========================================================================
// Anthropic format conversion (free functions)
// ===========================================================================

/// Split the system message out (Anthropic expects it as a top-level field,
/// not in the `messages` array) and convert the remaining messages to the
/// Anthropic wire format.
fn messages_to_anthropic(messages: &[Message]) -> (Option<String>, Vec<Value>) {
    let mut system: Option<String> = None;
    let mut wire_messages: Vec<Value> = Vec::with_capacity(messages.len());

    for msg in messages {
        match msg.role {
            Role::System => match &mut system {
                Some(existing) => {
                    existing.push('\n');
                    existing.push_str(&msg.content);
                }
                None => {
                    system = Some(msg.content.clone());
                }
            },
            Role::User => {
                wire_messages.push(json!({
                    "role": "user",
                    "content": msg.content,
                }));
            }
            Role::Assistant => {
                if msg.tool_calls.is_empty() {
                    wire_messages.push(json!({
                        "role": "assistant",
                        "content": msg.content,
                    }));
                } else {
                    let mut content: Vec<Value> = Vec::new();
                    if !msg.content.is_empty() {
                        content.push(json!({
                            "type": "text",
                            "text": msg.content,
                        }));
                    }
                    for tc in &msg.tool_calls {
                        content.push(json!({
                            "type": "tool_use",
                            "id": tc.id,
                            "name": tc.name,
                            "input": tc.arguments,
                        }));
                    }
                    wire_messages.push(json!({
                        "role": "assistant",
                        "content": content,
                    }));
                }
            }
            Role::Tool => {
                wire_messages.push(json!({
                    "role": "user",
                    "content": [{
                        "type": "tool_result",
                        "tool_use_id": msg.tool_call_id,
                        "content": msg.content,
                    }],
                }));
            }
        }
    }

    (system, wire_messages)
}

/// Convert tool definitions into the Anthropic API format.
fn tools_to_anthropic(tools: &[ToolDefinition]) -> Value {
    let tool_values: Vec<Value> = tools
        .iter()
        .map(|t| {
            json!({
                "name": t.name,
                "description": t.description,
                "input_schema": t.input_schema,
            })
        })
        .collect();
    json!(tool_values)
}

/// Parse a non-streaming Anthropic Messages API response.
fn parse_anthropic_response(v: &Value) -> Result<LlmResponse> {
    let content = v["content"]
        .as_array()
        .ok_or_else(|| AgentError::LlmParseFailed {
            reason: "missing `content` array in response".into(),
        })?;

    let mut text_parts: Vec<String> = Vec::new();
    let mut tool_calls: Vec<ToolCall> = Vec::new();

    for block in content {
        match block["type"].as_str() {
            Some("text") => {
                if let Some(t) = block["text"].as_str() {
                    text_parts.push(t.to_owned());
                }
            }
            Some("tool_use") => {
                tool_calls.push(ToolCall {
                    id: block["id"].as_str().unwrap_or_default().to_owned(),
                    name: block["name"].as_str().unwrap_or_default().to_owned(),
                    arguments: block["input"].clone(),
                });
            }
            _ => {}
        }
    }

    if tool_calls.is_empty() {
        Ok(LlmResponse::Text(text_parts.join("")))
    } else {
        Ok(LlmResponse::ToolCalls(tool_calls))
    }
}

// ===========================================================================
// OpenAI format conversion (free functions)
// ===========================================================================

/// Convert internal messages to the OpenAI Chat Completions wire format.
///
/// In the OpenAI format, system messages are part of the `messages` array
/// (with `role: "system"`), tool calls are in `assistant.tool_calls`, and
/// tool results use `role: "tool"` with a `tool_call_id`.
pub fn messages_to_openai(messages: &[Message]) -> Vec<Value> {
    let mut wire_messages: Vec<Value> = Vec::with_capacity(messages.len());

    for msg in messages {
        match msg.role {
            Role::System => {
                wire_messages.push(json!({
                    "role": "system",
                    "content": msg.content,
                }));
            }
            Role::User => {
                wire_messages.push(json!({
                    "role": "user",
                    "content": msg.content,
                }));
            }
            Role::Assistant => {
                if msg.tool_calls.is_empty() {
                    wire_messages.push(json!({
                        "role": "assistant",
                        "content": msg.content,
                    }));
                } else {
                    let tool_calls: Vec<Value> = msg
                        .tool_calls
                        .iter()
                        .map(|tc| {
                            json!({
                                "id": tc.id,
                                "type": "function",
                                "function": {
                                    "name": tc.name,
                                    "arguments": tc.arguments.to_string(),
                                }
                            })
                        })
                        .collect();

                    let mut m = json!({
                        "role": "assistant",
                        "tool_calls": tool_calls,
                    });

                    if !msg.content.is_empty() {
                        m["content"] = json!(msg.content);
                    }

                    wire_messages.push(m);
                }
            }
            Role::Tool => {
                wire_messages.push(json!({
                    "role": "tool",
                    "tool_call_id": msg.tool_call_id,
                    "content": msg.content,
                }));
            }
        }
    }

    wire_messages
}

/// Convert tool definitions into the OpenAI Chat Completions API format.
///
/// OpenAI wraps each tool in `{"type": "function", "function": {...}}`.
pub fn tools_to_openai(tools: &[ToolDefinition]) -> Value {
    let tool_values: Vec<Value> = tools
        .iter()
        .map(|t| {
            json!({
                "type": "function",
                "function": {
                    "name": t.name,
                    "description": t.description,
                    "parameters": t.input_schema,
                }
            })
        })
        .collect();
    json!(tool_values)
}

/// Parse a non-streaming OpenAI Chat Completions API response into an
/// [`LlmResponse`].
pub fn parse_openai_response(v: &Value) -> Result<LlmResponse> {
    let message = &v["choices"][0]["message"];

    if message.is_null() {
        return Err(AgentError::LlmParseFailed {
            reason: "missing `choices[0].message` in response".into(),
        });
    }

    // Check for tool calls first.
    if let Some(tool_calls_arr) = message["tool_calls"].as_array()
        && !tool_calls_arr.is_empty()
    {
        let calls: Result<Vec<ToolCall>> = tool_calls_arr
            .iter()
            .map(|tc| {
                let func = &tc["function"];
                let name = func["name"].as_str().unwrap_or_default().to_owned();
                let args_str = func["arguments"].as_str().unwrap_or("{}");
                let arguments: Value =
                    serde_json::from_str(args_str).map_err(|e| AgentError::LlmParseFailed {
                        reason: format!("invalid JSON in OpenAI tool call `{name}` arguments: {e}"),
                    })?;

                Ok(ToolCall {
                    id: tc["id"].as_str().unwrap_or_default().to_owned(),
                    name,
                    arguments,
                })
            })
            .collect();

        return Ok(LlmResponse::ToolCalls(calls?));
    }

    // Fall back to text content.
    let content = message["content"].as_str().unwrap_or_default();
    Ok(LlmResponse::Text(content.to_owned()))
}

// ---------------------------------------------------------------------------
// Anthropic stream accumulator (unchanged from original)
// ---------------------------------------------------------------------------

/// Accumulates fragments from Anthropic streaming events into a complete
/// response.
#[derive(Debug, Default)]
struct StreamAccumulator {
    /// Accumulated text output.
    text: String,

    /// Tool calls being built up from streaming fragments.
    tool_calls: Vec<ToolCallBuilder>,

    /// The stop reason, if received.
    stop_reason: Option<String>,

    /// Usage tracking (populated when message_start includes usage info).
    #[allow(dead_code)]
    usage: Usage,
}

/// In-progress tool call being assembled from streaming deltas.
#[derive(Debug)]
struct ToolCallBuilder {
    id: String,
    name: String,
    /// Accumulated JSON input string.
    input_json: String,
}

impl StreamAccumulator {
    fn new() -> Self {
        Self::default()
    }

    /// Apply a single stream event to the accumulator.
    fn apply<F>(&mut self, event: &StreamEvent, on_text: &mut F)
    where
        F: FnMut(&str),
    {
        match event {
            StreamEvent::ContentBlockStart {
                content_type,
                id,
                name,
                ..
            } => {
                if content_type == "tool_use" {
                    self.tool_calls.push(ToolCallBuilder {
                        id: id.clone().unwrap_or_default(),
                        name: name.clone().unwrap_or_default(),
                        input_json: String::new(),
                    });
                }
            }

            StreamEvent::ContentBlockDelta { delta, .. } => match delta {
                StreamDelta::TextDelta(t) => {
                    self.text.push_str(t);
                    on_text(t);
                }
                StreamDelta::InputJsonDelta(j) => {
                    if let Some(builder) = self.tool_calls.last_mut() {
                        builder.input_json.push_str(j);
                    }
                }
            },

            StreamEvent::MessageDelta { stop_reason } => {
                self.stop_reason = stop_reason.clone();
            }

            _ => {}
        }
    }

    /// Convert the accumulated state into a final [`LlmResponse`].
    fn into_response(self) -> Result<LlmResponse> {
        if self.tool_calls.is_empty() {
            Ok(LlmResponse::Text(self.text))
        } else {
            let calls: Result<Vec<ToolCall>> = self
                .tool_calls
                .into_iter()
                .map(|b| {
                    let arguments: Value = if b.input_json.is_empty() {
                        Value::Object(Default::default())
                    } else {
                        serde_json::from_str(&b.input_json).map_err(|e| {
                            AgentError::LlmParseFailed {
                                reason: format!(
                                    "invalid JSON in tool call `{}` input: {e}",
                                    b.name
                                ),
                            }
                        })?
                    };

                    Ok(ToolCall {
                        id: b.id,
                        name: b.name,
                        arguments,
                    })
                })
                .collect();

            Ok(LlmResponse::ToolCalls(calls?))
        }
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use crate::llm::types::Message;

    // -- Anthropic tests (preserved from original) ---------------------------

    #[test]
    fn build_anthropic_request_body_basic() {
        let config = LlmClientConfig::anthropic("test-key", "claude-sonnet-4-20250514");
        let client = LlmClient::new(config).unwrap();

        let request = ChatRequest {
            model: String::new(),
            messages: vec![Message::system("You are helpful."), Message::user("Hello")],
            tools: vec![],
            temperature: Some(0.7),
            max_tokens: Some(1024),
            stream: false,
        };

        let body = client.build_anthropic_request_body(&request, false);

        assert_eq!(body["model"], "claude-sonnet-4-20250514");
        assert_eq!(body["system"], "You are helpful.");
        assert_eq!(body["max_tokens"], 1024);
        let temp = body["temperature"].as_f64().unwrap();
        assert!((temp - 0.7).abs() < 1e-6, "temperature was {temp}");
        assert!(body.get("stream").is_none());

        let messages = body["messages"].as_array().unwrap();
        assert_eq!(messages.len(), 1);
        assert_eq!(messages[0]["role"], "user");
        assert_eq!(messages[0]["content"], "Hello");
    }

    #[test]
    fn build_anthropic_request_body_with_tools() {
        let config = LlmClientConfig::anthropic("test-key", "claude-sonnet-4-20250514");
        let client = LlmClient::new(config).unwrap();

        let request = ChatRequest {
            model: "claude-sonnet-4-20250514".into(),
            messages: vec![Message::user("Read file.txt")],
            tools: vec![ToolDefinition {
                name: "read_file".into(),
                description: "Read a file".into(),
                input_schema: serde_json::json!({
                    "type": "object",
                    "properties": {
                        "path": {"type": "string"}
                    },
                    "required": ["path"]
                }),
            }],
            temperature: None,
            max_tokens: None,
            stream: true,
        };

        let body = client.build_anthropic_request_body(&request, true);
        assert_eq!(body["stream"], true);
        assert!(body["tools"].is_array());
        assert_eq!(body["tools"][0]["name"], "read_file");
    }

    #[test]
    fn build_anthropic_request_body_tool_results() {
        let config = LlmClientConfig::anthropic("test-key", "claude-sonnet-4-20250514");
        let client = LlmClient::new(config).unwrap();

        let request = ChatRequest {
            model: String::new(),
            messages: vec![
                Message::user("Read test.txt"),
                Message::assistant_tool_calls(vec![ToolCall {
                    id: "tc_01".into(),
                    name: "read_file".into(),
                    arguments: serde_json::json!({"path": "test.txt"}),
                }]),
                Message::tool_result("tc_01", "file contents here"),
            ],
            tools: vec![],
            temperature: None,
            max_tokens: None,
            stream: false,
        };

        let body = client.build_anthropic_request_body(&request, false);
        let messages = body["messages"].as_array().unwrap();

        assert_eq!(messages[0]["role"], "user");
        assert_eq!(messages[1]["role"], "assistant");
        assert_eq!(messages[1]["content"][0]["type"], "tool_use");
        assert_eq!(messages[1]["content"][0]["id"], "tc_01");
        assert_eq!(messages[2]["role"], "user");
        assert_eq!(messages[2]["content"][0]["type"], "tool_result");
        assert_eq!(messages[2]["content"][0]["tool_use_id"], "tc_01");
    }

    #[test]
    fn empty_api_key_returns_error() {
        let config = LlmClientConfig::anthropic("", "claude-sonnet-4-20250514");
        let result = LlmClient::new(config);
        assert!(result.is_err());
    }

    #[test]
    fn parse_non_streaming_anthropic_text_response() {
        let response_json: Value = serde_json::json!({
            "id": "msg_01",
            "type": "message",
            "role": "assistant",
            "content": [
                {"type": "text", "text": "Hello, world!"}
            ],
            "stop_reason": "end_turn",
            "usage": {"input_tokens": 10, "output_tokens": 5}
        });

        let result = parse_anthropic_response(&response_json).unwrap();
        match result {
            LlmResponse::Text(text) => assert_eq!(text, "Hello, world!"),
            _ => panic!("expected Text response"),
        }
    }

    #[test]
    fn parse_non_streaming_anthropic_tool_use_response() {
        let response_json: Value = serde_json::json!({
            "id": "msg_01",
            "type": "message",
            "role": "assistant",
            "content": [
                {
                    "type": "tool_use",
                    "id": "toolu_01",
                    "name": "read_file",
                    "input": {"path": "/tmp/test.txt"}
                }
            ],
            "stop_reason": "tool_use",
            "usage": {"input_tokens": 20, "output_tokens": 15}
        });

        let result = parse_anthropic_response(&response_json).unwrap();
        match result {
            LlmResponse::ToolCalls(calls) => {
                assert_eq!(calls.len(), 1);
                assert_eq!(calls[0].id, "toolu_01");
                assert_eq!(calls[0].name, "read_file");
                assert_eq!(calls[0].arguments["path"], "/tmp/test.txt");
            }
            _ => panic!("expected ToolCalls response"),
        }
    }

    // -- OpenAI config tests -------------------------------------------------

    #[test]
    fn openai_config_construction() {
        let config = LlmClientConfig::openai("sk-test-key", "gpt-4o");
        assert_eq!(config.provider, LlmProvider::OpenAI);
        assert_eq!(config.api_key, "sk-test-key");
        assert_eq!(config.default_model, "gpt-4o");
        assert_eq!(config.base_url, "https://api.openai.com/v1");
        assert_eq!(config.max_tokens, 4096);
    }

    #[test]
    fn openai_compatible_config_construction() {
        let config =
            LlmClientConfig::openai_compatible("local-key", "llama3", "http://localhost:11434/v1");
        assert_eq!(config.provider, LlmProvider::OpenAI);
        assert_eq!(config.api_key, "local-key");
        assert_eq!(config.default_model, "llama3");
        assert_eq!(config.base_url, "http://localhost:11434/v1");
    }

    #[test]
    fn openai_empty_api_key_returns_error() {
        let config = LlmClientConfig::openai("", "gpt-4o");
        let result = LlmClient::new(config);
        assert!(result.is_err());
    }

    // -- OpenAI message conversion tests -------------------------------------

    #[test]
    fn messages_to_openai_system_message() {
        let messages = vec![Message::system("You are helpful."), Message::user("Hello")];
        let wire = messages_to_openai(&messages);

        assert_eq!(wire.len(), 2);
        assert_eq!(wire[0]["role"], "system");
        assert_eq!(wire[0]["content"], "You are helpful.");
        assert_eq!(wire[1]["role"], "user");
        assert_eq!(wire[1]["content"], "Hello");
    }

    #[test]
    fn messages_to_openai_assistant_text() {
        let messages = vec![Message::assistant("I can help with that.")];
        let wire = messages_to_openai(&messages);

        assert_eq!(wire.len(), 1);
        assert_eq!(wire[0]["role"], "assistant");
        assert_eq!(wire[0]["content"], "I can help with that.");
    }

    #[test]
    fn messages_to_openai_tool_calls() {
        let messages = vec![Message::assistant_tool_calls(vec![ToolCall {
            id: "call_abc".into(),
            name: "read_file".into(),
            arguments: serde_json::json!({"path": "test.txt"}),
        }])];
        let wire = messages_to_openai(&messages);

        assert_eq!(wire.len(), 1);
        assert_eq!(wire[0]["role"], "assistant");

        let tc = &wire[0]["tool_calls"][0];
        assert_eq!(tc["id"], "call_abc");
        assert_eq!(tc["type"], "function");
        assert_eq!(tc["function"]["name"], "read_file");
        // Arguments are serialized as a JSON string.
        let args_str = tc["function"]["arguments"].as_str().unwrap();
        let args: Value = serde_json::from_str(args_str).unwrap();
        assert_eq!(args["path"], "test.txt");
    }

    #[test]
    fn messages_to_openai_tool_result() {
        let messages = vec![Message::tool_result("call_abc", "file contents")];
        let wire = messages_to_openai(&messages);

        assert_eq!(wire.len(), 1);
        assert_eq!(wire[0]["role"], "tool");
        assert_eq!(wire[0]["tool_call_id"], "call_abc");
        assert_eq!(wire[0]["content"], "file contents");
    }

    // -- OpenAI tool definition conversion -----------------------------------

    #[test]
    fn tools_to_openai_format() {
        let tools = vec![ToolDefinition {
            name: "read_file".into(),
            description: "Read a file from disk".into(),
            input_schema: serde_json::json!({
                "type": "object",
                "properties": {
                    "path": {"type": "string"}
                },
                "required": ["path"]
            }),
        }];

        let wire = tools_to_openai(&tools);
        let arr = wire.as_array().unwrap();
        assert_eq!(arr.len(), 1);
        assert_eq!(arr[0]["type"], "function");
        assert_eq!(arr[0]["function"]["name"], "read_file");
        assert_eq!(arr[0]["function"]["description"], "Read a file from disk");
        assert_eq!(arr[0]["function"]["parameters"]["type"], "object");
    }

    // -- OpenAI non-streaming response parsing -------------------------------

    #[test]
    fn parse_openai_text_response() {
        let response_json: Value = serde_json::json!({
            "id": "chatcmpl-abc",
            "object": "chat.completion",
            "choices": [{
                "index": 0,
                "message": {
                    "role": "assistant",
                    "content": "Hello from OpenAI!"
                },
                "finish_reason": "stop"
            }],
            "usage": {"prompt_tokens": 10, "completion_tokens": 5}
        });

        let result = parse_openai_response(&response_json).unwrap();
        match result {
            LlmResponse::Text(text) => assert_eq!(text, "Hello from OpenAI!"),
            _ => panic!("expected Text response"),
        }
    }

    #[test]
    fn parse_openai_tool_call_response() {
        let response_json: Value = serde_json::json!({
            "id": "chatcmpl-abc",
            "object": "chat.completion",
            "choices": [{
                "index": 0,
                "message": {
                    "role": "assistant",
                    "content": null,
                    "tool_calls": [{
                        "id": "call_xyz",
                        "type": "function",
                        "function": {
                            "name": "read_file",
                            "arguments": "{\"path\":\"/tmp/test.txt\"}"
                        }
                    }]
                },
                "finish_reason": "tool_calls"
            }],
            "usage": {"prompt_tokens": 20, "completion_tokens": 15}
        });

        let result = parse_openai_response(&response_json).unwrap();
        match result {
            LlmResponse::ToolCalls(calls) => {
                assert_eq!(calls.len(), 1);
                assert_eq!(calls[0].id, "call_xyz");
                assert_eq!(calls[0].name, "read_file");
                assert_eq!(calls[0].arguments["path"], "/tmp/test.txt");
            }
            _ => panic!("expected ToolCalls response"),
        }
    }

    // -- Provider detection --------------------------------------------------

    #[test]
    fn provider_detection() {
        let anthropic_config = LlmClientConfig::anthropic("key", "claude-sonnet-4-20250514");
        let anthropic_client = LlmClient::new(anthropic_config).unwrap();
        assert_eq!(anthropic_client.provider(), LlmProvider::Anthropic);

        let openai_config = LlmClientConfig::openai("key", "gpt-4o");
        let openai_client = LlmClient::new(openai_config).unwrap();
        assert_eq!(openai_client.provider(), LlmProvider::OpenAI);
    }

    // -- LlmProvider equality ------------------------------------------------

    #[test]
    fn llm_provider_equality() {
        assert_eq!(LlmProvider::Anthropic, LlmProvider::Anthropic);
        assert_eq!(LlmProvider::OpenAI, LlmProvider::OpenAI);
        assert_ne!(LlmProvider::Anthropic, LlmProvider::OpenAI);
    }

    // -- OpenAI request body construction ------------------------------------

    #[test]
    fn build_openai_request_body_basic() {
        let config = LlmClientConfig::openai("sk-test", "gpt-4o");
        let client = LlmClient::new(config).unwrap();

        let request = ChatRequest {
            model: String::new(),
            messages: vec![Message::system("You are helpful."), Message::user("Hello")],
            tools: vec![],
            temperature: Some(0.5),
            max_tokens: Some(2048),
            stream: false,
        };

        let body = client.build_openai_request_body(&request, false);

        assert_eq!(body["model"], "gpt-4o");
        assert_eq!(body["max_tokens"], 2048);

        // System message should be in the messages array for OpenAI.
        let messages = body["messages"].as_array().unwrap();
        assert_eq!(messages.len(), 2);
        assert_eq!(messages[0]["role"], "system");
        assert_eq!(messages[0]["content"], "You are helpful.");
        assert_eq!(messages[1]["role"], "user");

        let temp = body["temperature"].as_f64().unwrap();
        assert!((temp - 0.5).abs() < 1e-6);
        assert!(body.get("stream").is_none());
    }

    #[test]
    fn build_openai_request_body_with_tools_and_stream() {
        let config = LlmClientConfig::openai("sk-test", "gpt-4o");
        let client = LlmClient::new(config).unwrap();

        let request = ChatRequest {
            model: "gpt-4o-mini".into(),
            messages: vec![Message::user("What is the weather?")],
            tools: vec![ToolDefinition {
                name: "get_weather".into(),
                description: "Get weather info".into(),
                input_schema: serde_json::json!({
                    "type": "object",
                    "properties": {
                        "city": {"type": "string"}
                    }
                }),
            }],
            temperature: None,
            max_tokens: None,
            stream: true,
        };

        let body = client.build_openai_request_body(&request, true);

        assert_eq!(body["model"], "gpt-4o-mini");
        assert_eq!(body["stream"], true);

        let tools = body["tools"].as_array().unwrap();
        assert_eq!(tools.len(), 1);
        assert_eq!(tools[0]["type"], "function");
        assert_eq!(tools[0]["function"]["name"], "get_weather");
    }
}
