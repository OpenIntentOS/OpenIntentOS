//! OpenAI Chat Completions API format conversion and request handling.
//!
//! Also covers OpenAI-compatible endpoints (DeepSeek, Ollama, Together, etc.).

use futures::StreamExt;
use reqwest::header::{AUTHORIZATION, CONTENT_TYPE, HeaderMap, HeaderValue};
use serde_json::{Value, json};

use crate::error::{AgentError, Result};
use crate::llm::streaming_openai::OpenAiStreamAccumulator;
use crate::llm::types::{
    LlmResponse, Message, Role, ToolCall, ToolDefinition, Usage,
};

/// Default OpenAI API base URL.
pub const BASE_URL: &str = "https://api.openai.com/v1";

// ---------------------------------------------------------------------------
// Request building
// ---------------------------------------------------------------------------

/// Build the JSON body for the OpenAI Chat Completions API.
pub fn build_request_body(
    messages: &[Message],
    model: &str,
    max_tokens: u32,
    temperature: Option<f32>,
    tools: &[ToolDefinition],
    stream: bool,
) -> Value {
    let wire_messages = messages_to_openai(messages);

    let mut body = json!({
        "model": model,
        "max_tokens": max_tokens,
        "messages": wire_messages,
    });

    if let Some(temp) = temperature {
        body["temperature"] = json!(temp);
    }

    if !tools.is_empty() {
        body["tools"] = tools_to_openai(tools);
    }

    if stream {
        body["stream"] = json!(true);
    }

    body
}

/// Send the HTTP request to the OpenAI Chat Completions API endpoint.
pub async fn send_request(
    http: &reqwest::Client,
    base_url: &str,
    api_key: &str,
    body: &Value,
) -> Result<reqwest::Response> {
    let url = format!("{base_url}/chat/completions");

    let mut headers = HeaderMap::new();
    let auth_value = format!("Bearer {api_key}");
    headers.insert(
        AUTHORIZATION,
        HeaderValue::from_str(&auth_value).map_err(|e| AgentError::LlmRequestFailed {
            reason: format!("invalid authorization header: {e}"),
        })?,
    );
    headers.insert(CONTENT_TYPE, HeaderValue::from_static("application/json"));

    tracing::debug!(
        url = %url,
        model = %body["model"],
        provider = "openai",
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

/// Consume an OpenAI SSE stream and aggregate into a final response with
/// usage.
pub async fn consume_stream<F>(
    resp: reqwest::Response,
    on_text: &mut F,
) -> Result<(LlmResponse, Usage)>
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

// ---------------------------------------------------------------------------
// Response parsing
// ---------------------------------------------------------------------------

/// Parse a non-streaming OpenAI Chat Completions API response.
pub fn parse_response(v: &Value) -> Result<LlmResponse> {
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
                        reason: format!(
                            "invalid JSON in OpenAI tool call `{name}` arguments: {e}"
                        ),
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
// Message format conversion
// ---------------------------------------------------------------------------

/// Convert internal messages to the OpenAI Chat Completions wire format.
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
