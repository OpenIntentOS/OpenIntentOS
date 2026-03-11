//! Anthropic Messages API format conversion and request handling.

use futures::StreamExt;
use reqwest::header::{AUTHORIZATION, CONTENT_TYPE, HeaderMap, HeaderValue};
use serde_json::{Value, json};

use crate::error::{AgentError, Result};
use crate::llm::streaming::SseParser;
use crate::llm::types::{
    LlmResponse, Message, Role, StreamDelta, StreamEvent, ToolCall, ToolDefinition, Usage,
};

/// Default Anthropic API base URL.
pub const BASE_URL: &str = "https://api.anthropic.com";

/// Anthropic API version header value.
const API_VERSION: &str = "2023-06-01";

/// Anthropic beta header required for OAuth token authentication.
const OAUTH_BETA: &str = "oauth-2025-04-20";

// ---------------------------------------------------------------------------
// Request building
// ---------------------------------------------------------------------------

/// Build the JSON body for the Anthropic Messages API.
pub fn build_request_body(
    messages: &[Message],
    model: &str,
    max_tokens: u32,
    temperature: Option<f32>,
    tools: &[ToolDefinition],
    stream: bool,
) -> Value {
    let (system_text, wire_messages) = messages_to_anthropic(messages);

    let mut body = json!({
        "model": model,
        "max_tokens": max_tokens,
        "messages": wire_messages,
    });

    if let Some(system) = system_text {
        body["system"] = json!(system);
    }

    if let Some(temp) = temperature {
        body["temperature"] = json!(temp);
    }

    if !tools.is_empty() {
        body["tools"] = tools_to_anthropic(tools);
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
pub async fn send_request(
    http: &reqwest::Client,
    base_url: &str,
    api_key: &str,
    body: &Value,
) -> Result<reqwest::Response> {
    let url = format!("{base_url}/v1/messages");

    let mut headers = HeaderMap::new();

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
        headers.insert("anthropic-beta", HeaderValue::from_static(OAUTH_BETA));
    } else {
        headers.insert(
            "x-api-key",
            HeaderValue::from_str(api_key).map_err(|e| AgentError::LlmRequestFailed {
                reason: format!("invalid API key header: {e}"),
            })?,
        );
    }

    headers.insert("anthropic-version", HeaderValue::from_static(API_VERSION));
    headers.insert(CONTENT_TYPE, HeaderValue::from_static("application/json"));

    tracing::debug!(
        url = %url,
        model = %body["model"],
        provider = "anthropic",
        is_oauth = is_oauth,
        "sending LLM request"
    );

    http.post(&url)
        .headers(headers)
        .json(body)
        .send()
        .await
        .map_err(|e| AgentError::LlmRequestFailed {
            reason: e.to_string(),
        })
}

// ---------------------------------------------------------------------------
// Streaming
// ---------------------------------------------------------------------------

/// Consume an Anthropic SSE stream and aggregate into a final response with
/// usage.
pub async fn consume_stream<F>(
    resp: reqwest::Response,
    on_text: &mut F,
) -> Result<(LlmResponse, Usage)>
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

// ---------------------------------------------------------------------------
// Response parsing
// ---------------------------------------------------------------------------

/// Parse a non-streaming Anthropic Messages API response.
pub fn parse_response(v: &Value) -> Result<LlmResponse> {
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

// ---------------------------------------------------------------------------
// Message format conversion
// ---------------------------------------------------------------------------

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

// ---------------------------------------------------------------------------
// Stream accumulator
// ---------------------------------------------------------------------------

/// Accumulates fragments from Anthropic streaming events into a complete
/// response.
#[derive(Debug, Default)]
struct StreamAccumulator {
    text: String,
    tool_calls: Vec<ToolCallBuilder>,
    stop_reason: Option<String>,
    usage: Usage,
}

#[derive(Debug)]
struct ToolCallBuilder {
    id: String,
    name: String,
    input_json: String,
}

impl StreamAccumulator {
    fn new() -> Self {
        Self::default()
    }

    fn apply<F>(&mut self, event: &StreamEvent, on_text: &mut F)
    where
        F: FnMut(&str),
    {
        match event {
            StreamEvent::MessageStart { input_tokens, .. } => {
                self.usage.input_tokens = *input_tokens;
            }
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
            StreamEvent::MessageDelta {
                stop_reason,
                output_tokens,
            } => {
                self.stop_reason = stop_reason.clone();
                self.usage.output_tokens = *output_tokens;
            }
            _ => {}
        }
    }

    fn into_response(self) -> Result<(LlmResponse, Usage)> {
        let usage = self.usage;
        if self.tool_calls.is_empty() {
            Ok((LlmResponse::Text(self.text), usage))
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

            Ok((LlmResponse::ToolCalls(calls?), usage))
        }
    }
}
