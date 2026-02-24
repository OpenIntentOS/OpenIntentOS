//! SSE stream parser for the Anthropic Messages API.
//!
//! The Anthropic streaming format sends `event:` and `data:` lines in
//! standard SSE format.  This module parses those lines into typed
//! [`StreamEvent`] values that the rest of the agent runtime can consume.

use serde_json::Value;

use crate::error::{AgentError, Result};
use crate::llm::types::{StreamDelta, StreamEvent};

/// Parses raw SSE lines from the Anthropic Messages API stream.
///
/// Accumulates partial state across calls because SSE events span multiple
/// lines (`event:` followed by `data:`).
#[derive(Debug, Default)]
pub struct SseParser {
    /// The most recently seen `event:` type.
    current_event_type: Option<String>,
}

impl SseParser {
    /// Create a new parser.
    pub fn new() -> Self {
        Self::default()
    }

    /// Feed a single line from the SSE stream.
    ///
    /// Returns `Some(event)` when a complete event has been parsed, `None` for
    /// comment lines, blank lines, or the `event:` prefix line (which just
    /// sets internal state for the next `data:` line).
    pub fn parse_line(&mut self, line: &str) -> Result<Option<StreamEvent>> {
        let line = line.trim_end();

        // SSE comment lines start with `:`.
        if line.starts_with(':') || line.is_empty() {
            return Ok(None);
        }

        // `event: <type>` — stash the type for the next `data:` line.
        if let Some(event_type) = line.strip_prefix("event: ") {
            self.current_event_type = Some(event_type.to_owned());
            return Ok(None);
        }

        // `data: <json>` — combine with the stashed event type.
        if let Some(data) = line.strip_prefix("data: ") {
            let event_type = self
                .current_event_type
                .take()
                .unwrap_or_else(|| "unknown".into());

            return self.parse_event(&event_type, data);
        }

        // Unknown line format; ignore gracefully.
        tracing::trace!(line, "ignoring unrecognised SSE line");
        Ok(None)
    }

    /// Parse a (event_type, data_json) pair into a [`StreamEvent`].
    fn parse_event(&self, event_type: &str, data: &str) -> Result<Option<StreamEvent>> {
        match event_type {
            "message_start" => {
                let v: Value = parse_json(data)?;
                let message = &v["message"];
                Ok(Some(StreamEvent::MessageStart {
                    message_id: json_string(message, "id"),
                    model: json_string(message, "model"),
                }))
            }

            "content_block_start" => {
                let v: Value = parse_json(data)?;
                let index = v["index"].as_u64().unwrap_or(0) as u32;
                let block = &v["content_block"];
                let content_type = json_string(block, "type");
                let id = block["id"].as_str().map(String::from);
                let name = block["name"].as_str().map(String::from);

                Ok(Some(StreamEvent::ContentBlockStart {
                    index,
                    content_type,
                    id,
                    name,
                }))
            }

            "content_block_delta" => {
                let v: Value = parse_json(data)?;
                let index = v["index"].as_u64().unwrap_or(0) as u32;
                let delta_obj = &v["delta"];
                let delta_type = json_string(delta_obj, "type");

                let delta = match delta_type.as_str() {
                    "text_delta" => StreamDelta::TextDelta(json_string(delta_obj, "text")),
                    "input_json_delta" => {
                        StreamDelta::InputJsonDelta(json_string(delta_obj, "partial_json"))
                    }
                    other => {
                        tracing::warn!(delta_type = other, "unknown delta type");
                        return Ok(None);
                    }
                };

                Ok(Some(StreamEvent::ContentBlockDelta { index, delta }))
            }

            "content_block_stop" => {
                let v: Value = parse_json(data)?;
                let index = v["index"].as_u64().unwrap_or(0) as u32;
                Ok(Some(StreamEvent::ContentBlockStop { index }))
            }

            "message_delta" => {
                let v: Value = parse_json(data)?;
                let stop_reason = v["delta"]["stop_reason"].as_str().map(String::from);
                Ok(Some(StreamEvent::MessageDelta { stop_reason }))
            }

            "message_stop" => Ok(Some(StreamEvent::MessageStop)),

            "ping" => Ok(Some(StreamEvent::Ping)),

            // `[DONE]` or any unrecognised event type.
            _ => {
                if data.trim() == "[DONE]" {
                    Ok(Some(StreamEvent::MessageStop))
                } else {
                    tracing::trace!(event_type, "ignoring unknown SSE event type");
                    Ok(None)
                }
            }
        }
    }
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

/// Parse a JSON string, mapping errors to [`AgentError::LlmParseFailed`].
fn parse_json(data: &str) -> Result<Value> {
    serde_json::from_str(data).map_err(|e| AgentError::LlmParseFailed {
        reason: format!("invalid JSON in SSE data: {e}"),
    })
}

/// Extract a string field from a JSON value, returning an empty string if
/// missing.
fn json_string(v: &Value, field: &str) -> String {
    v[field].as_str().unwrap_or_default().to_owned()
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_message_start() {
        let mut parser = SseParser::new();
        assert!(parser.parse_line("event: message_start").unwrap().is_none());
        let event = parser
            .parse_line(r#"data: {"type":"message_start","message":{"id":"msg_01","model":"claude-sonnet-4-20250514","role":"assistant","content":[],"stop_reason":null,"usage":{"input_tokens":10,"output_tokens":0}}}"#)
            .unwrap()
            .unwrap();

        match event {
            StreamEvent::MessageStart { message_id, model } => {
                assert_eq!(message_id, "msg_01");
                assert_eq!(model, "claude-sonnet-4-20250514");
            }
            other => panic!("unexpected event: {other:?}"),
        }
    }

    #[test]
    fn parse_text_delta() {
        let mut parser = SseParser::new();
        assert!(
            parser
                .parse_line("event: content_block_delta")
                .unwrap()
                .is_none()
        );
        let event = parser
            .parse_line(r#"data: {"type":"content_block_delta","index":0,"delta":{"type":"text_delta","text":"Hello"}}"#)
            .unwrap()
            .unwrap();

        match event {
            StreamEvent::ContentBlockDelta { index, delta } => {
                assert_eq!(index, 0);
                match delta {
                    StreamDelta::TextDelta(t) => assert_eq!(t, "Hello"),
                    other => panic!("unexpected delta: {other:?}"),
                }
            }
            other => panic!("unexpected event: {other:?}"),
        }
    }

    #[test]
    fn parse_input_json_delta() {
        let mut parser = SseParser::new();
        assert!(
            parser
                .parse_line("event: content_block_delta")
                .unwrap()
                .is_none()
        );
        let event = parser
            .parse_line(r#"data: {"type":"content_block_delta","index":1,"delta":{"type":"input_json_delta","partial_json":"{\"path\":"}}"#)
            .unwrap()
            .unwrap();

        match event {
            StreamEvent::ContentBlockDelta { index, delta } => {
                assert_eq!(index, 1);
                match delta {
                    StreamDelta::InputJsonDelta(j) => assert_eq!(j, r#"{"path":"#),
                    other => panic!("unexpected delta: {other:?}"),
                }
            }
            other => panic!("unexpected event: {other:?}"),
        }
    }

    #[test]
    fn parse_message_stop() {
        let mut parser = SseParser::new();
        assert!(parser.parse_line("event: message_stop").unwrap().is_none());
        let event = parser.parse_line("data: {}").unwrap().unwrap();
        assert!(matches!(event, StreamEvent::MessageStop));
    }

    #[test]
    fn blank_and_comment_lines_ignored() {
        let mut parser = SseParser::new();
        assert!(parser.parse_line("").unwrap().is_none());
        assert!(parser.parse_line(": keepalive").unwrap().is_none());
    }

    #[test]
    fn ping_event() {
        let mut parser = SseParser::new();
        assert!(parser.parse_line("event: ping").unwrap().is_none());
        let event = parser.parse_line("data: {}").unwrap().unwrap();
        assert!(matches!(event, StreamEvent::Ping));
    }
}
