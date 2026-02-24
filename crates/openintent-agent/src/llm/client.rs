//! Multi-provider LLM client.
//!
//! Currently supports the **Anthropic Messages API** with both streaming SSE
//! and non-streaming modes.  The client is designed for easy extension to
//! additional providers (OpenAI, Ollama, etc.) in future iterations.

use std::sync::Arc;

use futures::StreamExt;
use reqwest::header::{CONTENT_TYPE, HeaderMap, HeaderValue};
use serde_json::{Value, json};

use crate::error::{AgentError, Result};
use crate::llm::streaming::SseParser;
use crate::llm::types::{
    ChatRequest, LlmResponse, Message, Role, StreamDelta, StreamEvent, ToolCall, ToolDefinition,
    Usage,
};

// ---------------------------------------------------------------------------
// Constants
// ---------------------------------------------------------------------------

/// Default Anthropic API base URL.
const ANTHROPIC_BASE_URL: &str = "https://api.anthropic.com";

/// Anthropic API version header value.
const ANTHROPIC_VERSION: &str = "2023-06-01";

// ---------------------------------------------------------------------------
// Client
// ---------------------------------------------------------------------------

/// Configuration for connecting to a single LLM provider endpoint.
#[derive(Debug, Clone)]
pub struct LlmClientConfig {
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
            api_key: api_key.into(),
            base_url: ANTHROPIC_BASE_URL.to_owned(),
            default_model: model.into(),
            max_tokens: 4096,
        }
    }
}

/// An LLM client that communicates with the Anthropic Messages API.
///
/// Supports both streaming and non-streaming modes, tool use, and system
/// prompts.
#[derive(Debug, Clone)]
pub struct LlmClient {
    config: Arc<LlmClientConfig>,
    http: reqwest::Client,
}

impl LlmClient {
    /// Create a new client with the given configuration.
    pub fn new(config: LlmClientConfig) -> Result<Self> {
        if config.api_key.is_empty() {
            return Err(AgentError::MissingApiKey {
                provider: "anthropic".into(),
            });
        }

        let http = reqwest::Client::builder()
            .timeout(std::time::Duration::from_secs(120))
            .build()
            .map_err(|e| AgentError::LlmRequestFailed {
                reason: format!("failed to build HTTP client: {e}"),
            })?;

        Ok(Self {
            config: Arc::new(config),
            http,
        })
    }

    // -----------------------------------------------------------------------
    // Public API
    // -----------------------------------------------------------------------

    /// Send a chat request and return the full response (non-streaming).
    ///
    /// This blocks until the entire response is received and then parses it
    /// into an [`LlmResponse`].
    pub async fn chat(&self, request: &ChatRequest) -> Result<LlmResponse> {
        let body = self.build_request_body(request, false);
        let resp = self.send_request(&body).await?;

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

        self.parse_non_streaming_response(&v)
    }

    /// Send a chat request using streaming SSE and return the aggregated
    /// response.
    ///
    /// Internally consumes the SSE stream, accumulating text and tool-call
    /// fragments until the message is complete.
    pub async fn stream_chat(&self, request: &ChatRequest) -> Result<LlmResponse> {
        let body = self.build_request_body(request, true);
        let resp = self.send_request(&body).await?;

        let status = resp.status();
        if !status.is_success() {
            let text = resp.text().await.unwrap_or_default();
            return Err(AgentError::LlmRequestFailed {
                reason: format!("API returned {status}: {text}"),
            });
        }

        self.consume_stream(resp).await
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
        let body = self.build_request_body(request, true);
        let resp = self.send_request(&body).await?;

        let status = resp.status();
        if !status.is_success() {
            let text = resp.text().await.unwrap_or_default();
            return Err(AgentError::LlmRequestFailed {
                reason: format!("API returned {status}: {text}"),
            });
        }

        self.consume_stream_with_callback(resp, &mut on_text).await
    }

    // -----------------------------------------------------------------------
    // Internal: request building
    // -----------------------------------------------------------------------

    /// Build the JSON body for the Anthropic Messages API.
    fn build_request_body(&self, request: &ChatRequest, stream: bool) -> Value {
        // Separate system message from the conversation.
        let (system_text, messages) = self.split_system_message(&request.messages);

        let mut body = json!({
            "model": if request.model.is_empty() {
                &self.config.default_model
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
            body["tools"] = self.build_tools_payload(&request.tools);
        }

        if stream {
            body["stream"] = json!(true);
        }

        body
    }

    /// Convert our tool definitions into the Anthropic API format.
    fn build_tools_payload(&self, tools: &[ToolDefinition]) -> Value {
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

    /// Split the system message out (Anthropic expects it as a top-level
    /// field, not in the `messages` array) and convert the remaining messages
    /// to the Anthropic wire format.
    fn split_system_message(&self, messages: &[Message]) -> (Option<String>, Vec<Value>) {
        let mut system: Option<String> = None;
        let mut wire_messages: Vec<Value> = Vec::with_capacity(messages.len());

        for msg in messages {
            match msg.role {
                Role::System => {
                    // Anthropic only supports a single system block; concat if
                    // multiple system messages exist.
                    match &mut system {
                        Some(existing) => {
                            existing.push('\n');
                            existing.push_str(&msg.content);
                        }
                        None => {
                            system = Some(msg.content.clone());
                        }
                    }
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
                        // Assistant message with tool_use content blocks.
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
                    // Anthropic represents tool results as user messages with
                    // `tool_result` content blocks.
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

    /// Send the HTTP request to the Anthropic Messages API endpoint.
    async fn send_request(&self, body: &Value) -> Result<reqwest::Response> {
        let url = format!("{}/v1/messages", self.config.base_url);

        let mut headers = HeaderMap::new();
        headers.insert(
            "x-api-key",
            HeaderValue::from_str(&self.config.api_key).map_err(|e| {
                AgentError::LlmRequestFailed {
                    reason: format!("invalid API key header: {e}"),
                }
            })?,
        );
        headers.insert(
            "anthropic-version",
            HeaderValue::from_static(ANTHROPIC_VERSION),
        );
        headers.insert(CONTENT_TYPE, HeaderValue::from_static("application/json"));

        tracing::debug!(url = %url, model = %body["model"], "sending LLM request");

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

    // -----------------------------------------------------------------------
    // Internal: response parsing (non-streaming)
    // -----------------------------------------------------------------------

    /// Parse a non-streaming Anthropic Messages API response.
    fn parse_non_streaming_response(&self, v: &Value) -> Result<LlmResponse> {
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

    // -----------------------------------------------------------------------
    // Internal: streaming consumption
    // -----------------------------------------------------------------------

    /// Consume an SSE stream and aggregate into a final [`LlmResponse`].
    async fn consume_stream(&self, resp: reqwest::Response) -> Result<LlmResponse> {
        self.consume_stream_with_callback(resp, &mut |_| {}).await
    }

    /// Consume an SSE stream, calling `on_text` for each text delta, and
    /// aggregate into a final [`LlmResponse`].
    async fn consume_stream_with_callback<F>(
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

        // Buffer for partial lines that span chunk boundaries.
        let mut line_buffer = String::new();

        while let Some(chunk_result) = byte_stream.next().await {
            let chunk = chunk_result.map_err(|e| AgentError::LlmStreamError {
                reason: format!("stream read error: {e}"),
            })?;

            let text = std::str::from_utf8(&chunk).map_err(|e| AgentError::LlmStreamError {
                reason: format!("invalid UTF-8 in stream: {e}"),
            })?;

            line_buffer.push_str(text);

            // Process complete lines.  SSE lines are delimited by `\n`.
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

        // Stream ended without a MessageStop; return what we have.
        accumulator.into_response()
    }
}

// ---------------------------------------------------------------------------
// Stream accumulator
// ---------------------------------------------------------------------------

/// Accumulates fragments from streaming events into a complete response.
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

            // Other events don't affect the accumulator.
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

    #[test]
    fn build_request_body_basic() {
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

        let body = client.build_request_body(&request, false);

        assert_eq!(body["model"], "claude-sonnet-4-20250514");
        assert_eq!(body["system"], "You are helpful.");
        assert_eq!(body["max_tokens"], 1024);
        // f32 → JSON round-trip: compare as f64 with tolerance.
        let temp = body["temperature"].as_f64().unwrap();
        assert!((temp - 0.7).abs() < 1e-6, "temperature was {temp}");
        assert!(body.get("stream").is_none());

        let messages = body["messages"].as_array().unwrap();
        assert_eq!(messages.len(), 1);
        assert_eq!(messages[0]["role"], "user");
        assert_eq!(messages[0]["content"], "Hello");
    }

    #[test]
    fn build_request_body_with_tools() {
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

        let body = client.build_request_body(&request, true);
        assert_eq!(body["stream"], true);
        assert!(body["tools"].is_array());
        assert_eq!(body["tools"][0]["name"], "read_file");
    }

    #[test]
    fn build_request_body_tool_results() {
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

        let body = client.build_request_body(&request, false);
        let messages = body["messages"].as_array().unwrap();

        // User message
        assert_eq!(messages[0]["role"], "user");

        // Assistant with tool_use block
        assert_eq!(messages[1]["role"], "assistant");
        assert_eq!(messages[1]["content"][0]["type"], "tool_use");
        assert_eq!(messages[1]["content"][0]["id"], "tc_01");

        // Tool result as user message
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
    fn parse_non_streaming_text_response() {
        let config = LlmClientConfig::anthropic("test-key", "claude-sonnet-4-20250514");
        let client = LlmClient::new(config).unwrap();

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

        let result = client.parse_non_streaming_response(&response_json).unwrap();
        match result {
            LlmResponse::Text(text) => assert_eq!(text, "Hello, world!"),
            _ => panic!("expected Text response"),
        }
    }

    #[test]
    fn parse_non_streaming_tool_use_response() {
        let config = LlmClientConfig::anthropic("test-key", "claude-sonnet-4-20250514");
        let client = LlmClient::new(config).unwrap();

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

        let result = client.parse_non_streaming_response(&response_json).unwrap();
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
}
