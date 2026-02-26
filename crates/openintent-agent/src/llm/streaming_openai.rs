//! SSE stream parser for the OpenAI Chat Completions API.
//!
//! The OpenAI streaming format sends `data:` lines in standard SSE format
//! with JSON payloads containing `choices[].delta` objects.  The stream
//! terminates with a `data: [DONE]` sentinel.  This module parses those
//! lines into tool calls and text content that the agent runtime can consume.

use serde_json::Value;

use crate::error::{AgentError, Result};
use crate::llm::types::{LlmResponse, ToolCall, Usage};

// ---------------------------------------------------------------------------
// Stream accumulator
// ---------------------------------------------------------------------------

/// Accumulates fragments from an OpenAI SSE stream into a complete response.
///
/// OpenAI streams content and tool call deltas across many `data:` lines.
/// Text deltas are simple string concatenation.  Tool call deltas require
/// accumulating the function name and arguments across multiple chunks
/// (the name typically arrives in the first chunk, with argument fragments
/// following in subsequent chunks).
#[derive(Debug, Default)]
pub struct OpenAiStreamAccumulator {
    /// Accumulated text content from `choices[].delta.content`.
    text: String,

    /// In-progress tool calls indexed by their position in the tool_calls
    /// array.  OpenAI sends `index` to correlate chunks.
    tool_call_builders: Vec<OpenAiToolCallBuilder>,

    /// Whether the `[DONE]` sentinel has been received.
    done: bool,

    /// Token usage collected from stream chunks that include a `usage` field
    /// (OpenAI sends this in the final chunk before `[DONE]`).
    usage: Usage,
}

/// In-progress tool call being assembled from streaming deltas.
#[derive(Debug, Default)]
struct OpenAiToolCallBuilder {
    /// The tool call id (e.g. `"call_abc123"`).
    id: String,
    /// The function name.
    name: String,
    /// Accumulated function arguments JSON string.
    arguments: String,
}

impl OpenAiStreamAccumulator {
    /// Create a new empty accumulator.
    pub fn new() -> Self {
        Self::default()
    }

    /// Returns `true` if the `[DONE]` sentinel has been received.
    pub fn is_done(&self) -> bool {
        self.done
    }

    /// Feed a single SSE line from the stream.
    ///
    /// Returns `Ok(Some(text_delta))` when a text content delta is present
    /// (for incremental rendering), `Ok(None)` for non-text events, or an
    /// error if parsing fails.
    pub fn feed_line(&mut self, line: &str) -> Result<Option<String>> {
        let line = line.trim_end();

        // Skip empty lines, comments, and non-data lines.
        if line.is_empty() || line.starts_with(':') {
            return Ok(None);
        }

        // Extract the data payload.
        let data = match line.strip_prefix("data: ") {
            Some(d) => d,
            None => {
                // Could be an `event:` line or other SSE field; ignore.
                return Ok(None);
            }
        };

        let data = data.trim();

        // Check for the stream terminator.
        if data == "[DONE]" {
            self.done = true;
            return Ok(None);
        }

        // Parse the JSON payload.
        let v: Value = serde_json::from_str(data).map_err(|e| AgentError::LlmParseFailed {
            reason: format!("invalid JSON in OpenAI SSE data: {e}"),
        })?;

        // Navigate to choices[0].delta.
        let delta = &v["choices"][0]["delta"];
        if delta.is_null() {
            return Ok(None);
        }

        // Handle text content delta.
        let mut text_delta: Option<String> = None;
        if let Some(content) = delta["content"].as_str() {
            self.text.push_str(content);
            text_delta = Some(content.to_owned());
        }

        // Handle tool call deltas.
        if let Some(tool_calls) = delta["tool_calls"].as_array() {
            for tc in tool_calls {
                let index = tc["index"].as_u64().unwrap_or(0) as usize;

                // Grow the builders vector if necessary.
                while self.tool_call_builders.len() <= index {
                    self.tool_call_builders
                        .push(OpenAiToolCallBuilder::default());
                }

                let builder = &mut self.tool_call_builders[index];

                // The id is typically sent in the first chunk for each tool call.
                if let Some(id) = tc["id"].as_str() {
                    builder.id = id.to_owned();
                }

                // Function name and arguments are under `function`.
                let func = &tc["function"];
                if let Some(name) = func["name"].as_str() {
                    builder.name.push_str(name);
                }
                if let Some(args) = func["arguments"].as_str() {
                    builder.arguments.push_str(args);
                }
            }
        }

        // Some OpenAI-compatible providers include usage in stream chunks.
        if let Some(usage_obj) = v.get("usage").filter(|u| !u.is_null()) {
            if let Some(input) = usage_obj["prompt_tokens"].as_u64() {
                self.usage.input_tokens = input as u32;
            }
            if let Some(output) = usage_obj["completion_tokens"].as_u64() {
                self.usage.output_tokens = output as u32;
            }
        }

        Ok(text_delta)
    }

    /// Consume the accumulator and produce the final [`LlmResponse`] and [`Usage`].
    ///
    /// If any tool calls were accumulated, they take priority over text
    /// content (matching the non-streaming behavior).
    pub fn into_response(self) -> Result<(LlmResponse, Usage)> {
        let usage = self.usage;
        if self.tool_call_builders.is_empty() {
            return Ok((LlmResponse::Text(self.text), usage));
        }

        let calls: Result<Vec<ToolCall>> = self
            .tool_call_builders
            .into_iter()
            .map(|b| {
                let arguments: Value = if b.arguments.is_empty() {
                    Value::Object(Default::default())
                } else {
                    serde_json::from_str(&b.arguments).map_err(|e| AgentError::LlmParseFailed {
                        reason: format!(
                            "invalid JSON in OpenAI tool call `{}` arguments: {e}",
                            b.name
                        ),
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

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn text_content_accumulation() {
        let mut acc = OpenAiStreamAccumulator::new();

        let delta1 = acc
            .feed_line(
                r#"data: {"id":"chatcmpl-1","choices":[{"index":0,"delta":{"content":"Hello"}}]}"#,
            )
            .unwrap();
        assert_eq!(delta1, Some("Hello".to_owned()));

        let delta2 = acc
            .feed_line(
                r#"data: {"id":"chatcmpl-1","choices":[{"index":0,"delta":{"content":" world"}}]}"#,
            )
            .unwrap();
        assert_eq!(delta2, Some(" world".to_owned()));

        let (resp, _usage) = acc.into_response().unwrap();
        match resp {
            LlmResponse::Text(t) => assert_eq!(t, "Hello world"),
            _ => panic!("expected Text response"),
        }
    }

    #[test]
    fn done_sentinel_sets_flag() {
        let mut acc = OpenAiStreamAccumulator::new();
        assert!(!acc.is_done());

        let result = acc.feed_line("data: [DONE]").unwrap();
        assert!(result.is_none());
        assert!(acc.is_done());
    }

    #[test]
    fn blank_and_comment_lines_ignored() {
        let mut acc = OpenAiStreamAccumulator::new();
        assert!(acc.feed_line("").unwrap().is_none());
        assert!(acc.feed_line(": keepalive").unwrap().is_none());
        assert!(acc.feed_line("event: message").unwrap().is_none());
    }

    #[test]
    fn tool_call_accumulation() {
        let mut acc = OpenAiStreamAccumulator::new();

        // First chunk: id and function name.
        acc.feed_line(
            r#"data: {"id":"chatcmpl-1","choices":[{"index":0,"delta":{"tool_calls":[{"index":0,"id":"call_abc","function":{"name":"read_file","arguments":""}}]}}]}"#,
        )
        .unwrap();

        // Second chunk: argument fragment.
        acc.feed_line(
            r#"data: {"id":"chatcmpl-1","choices":[{"index":0,"delta":{"tool_calls":[{"index":0,"function":{"arguments":"{\"path\":"}}]}}]}"#,
        )
        .unwrap();

        // Third chunk: remaining argument fragment.
        acc.feed_line(
            r#"data: {"id":"chatcmpl-1","choices":[{"index":0,"delta":{"tool_calls":[{"index":0,"function":{"arguments":"\"test.txt\"}"}}]}}]}"#,
        )
        .unwrap();

        acc.feed_line("data: [DONE]").unwrap();
        assert!(acc.is_done());

        let (resp, _usage) = acc.into_response().unwrap();
        match resp {
            LlmResponse::ToolCalls(calls) => {
                assert_eq!(calls.len(), 1);
                assert_eq!(calls[0].id, "call_abc");
                assert_eq!(calls[0].name, "read_file");
                assert_eq!(calls[0].arguments["path"], "test.txt");
            }
            _ => panic!("expected ToolCalls response"),
        }
    }

    #[test]
    fn multiple_tool_calls_in_stream() {
        let mut acc = OpenAiStreamAccumulator::new();

        // Two tool calls in parallel (different indices).
        acc.feed_line(
            r#"data: {"id":"chatcmpl-1","choices":[{"index":0,"delta":{"tool_calls":[{"index":0,"id":"call_1","function":{"name":"read_file","arguments":"{\"path\":\"a.txt\"}"}}]}}]}"#,
        )
        .unwrap();

        acc.feed_line(
            r#"data: {"id":"chatcmpl-1","choices":[{"index":0,"delta":{"tool_calls":[{"index":1,"id":"call_2","function":{"name":"write_file","arguments":"{\"path\":\"b.txt\"}"}}]}}]}"#,
        )
        .unwrap();

        acc.feed_line("data: [DONE]").unwrap();

        let (resp, _usage) = acc.into_response().unwrap();
        match resp {
            LlmResponse::ToolCalls(calls) => {
                assert_eq!(calls.len(), 2);
                assert_eq!(calls[0].name, "read_file");
                assert_eq!(calls[1].name, "write_file");
            }
            _ => panic!("expected ToolCalls response"),
        }
    }

    #[test]
    fn empty_stream_returns_empty_text() {
        let acc = OpenAiStreamAccumulator::new();
        let (resp, _usage) = acc.into_response().unwrap();
        match resp {
            LlmResponse::Text(t) => assert!(t.is_empty()),
            _ => panic!("expected Text response"),
        }
    }

    #[test]
    fn invalid_json_returns_error() {
        let mut acc = OpenAiStreamAccumulator::new();
        let result = acc.feed_line("data: {invalid json}");
        assert!(result.is_err());
    }
}
