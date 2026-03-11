//! SSE stream parser for the ChatGPT web backend-api.
//!
//! The web API sends full message snapshots (not deltas like the OpenAI API),
//! so this accumulator diffs against the previous content to emit incremental
//! text deltas.

use serde_json::Value;

use crate::error::Result;
use crate::llm::types::{LlmResponse, ToolCall, Usage};

/// Accumulates ChatGPT web SSE stream events into a final response.
#[derive(Debug)]
pub struct ChatGptWebStreamAccumulator {
    /// Full accumulated text from the last snapshot.
    prev_text: String,
    /// Final complete text.
    final_text: String,
    /// Whether the stream is complete.
    done: bool,
    /// Whether the request included tools (for parsing tool calls from text).
    has_tools: bool,
    /// Conversation ID returned by the API.
    conversation_id: Option<String>,
}

impl ChatGptWebStreamAccumulator {
    /// Create a new accumulator.
    pub fn new(has_tools: bool) -> Self {
        Self {
            prev_text: String::new(),
            final_text: String::new(),
            done: false,
            has_tools,
            conversation_id: None,
        }
    }

    /// Feed a single SSE line to the accumulator.
    ///
    /// Returns `Some(delta)` when new text is available, `None` otherwise.
    pub fn feed_line(&mut self, line: &str) -> Result<Option<String>> {
        let line = line.trim();

        // Skip empty lines and event type lines.
        if line.is_empty() || line.starts_with("event:") {
            return Ok(None);
        }

        // Must start with "data: ".
        let data = match line.strip_prefix("data: ") {
            Some(d) => d.trim(),
            None => return Ok(None),
        };

        // Check for stream end sentinel.
        if data == "[DONE]" {
            self.done = true;
            return Ok(None);
        }

        // Parse JSON.
        let v: Value = match serde_json::from_str(data) {
            Ok(v) => v,
            Err(_) => return Ok(None), // Skip malformed lines.
        };

        // Extract conversation_id if present.
        if self.conversation_id.is_none() {
            if let Some(cid) = v["conversation_id"].as_str() {
                self.conversation_id = Some(cid.to_owned());
            }
        }

        // Check if message is complete.
        if v["message"]["status"].as_str() == Some("finished_successfully") {
            self.done = true;
        }

        // Extract message content.
        let parts = &v["message"]["content"]["parts"];
        if let Some(parts_arr) = parts.as_array() {
            // Concatenate all text parts.
            let mut current_text = String::new();
            for part in parts_arr {
                if let Some(s) = part.as_str() {
                    current_text.push_str(s);
                }
            }

            // Calculate delta (new text since last snapshot).
            if current_text.len() > self.prev_text.len() {
                let delta = current_text[self.prev_text.len()..].to_owned();
                self.prev_text = current_text.clone();
                self.final_text = current_text;
                return Ok(Some(delta));
            } else if current_text != self.prev_text {
                // Content was replaced (unusual but handle it).
                self.prev_text = current_text.clone();
                self.final_text = current_text;
            }
        }

        Ok(None)
    }

    /// Whether the stream is complete.
    pub fn is_done(&self) -> bool {
        self.done
    }

    /// Convert accumulated state into a final response.
    pub fn into_response(self) -> Result<(LlmResponse, Usage)> {
        let usage = Usage {
            input_tokens: 0,
            output_tokens: 0,
        };

        let text = self.final_text;

        // If tools were provided, try to parse tool calls from the text.
        if self.has_tools {
            if let Some(tool_call) = parse_tool_call_from_text(&text) {
                return Ok((LlmResponse::ToolCalls(vec![tool_call]), usage));
            }
        }

        Ok((LlmResponse::Text(text), usage))
    }
}

/// Try to parse a tool call from the assistant's text response.
///
/// Looks for JSON matching: `{"tool_call": {"id": "...", "name": "...",
/// "arguments": {...}}}`.
fn parse_tool_call_from_text(text: &str) -> Option<ToolCall> {
    // Scan for JSON objects in the text.
    for line in text.lines() {
        let line = line.trim();
        if !line.starts_with('{') {
            continue;
        }

        if let Ok(v) = serde_json::from_str::<Value>(line) {
            if let Some(tc) = &v["tool_call"].as_object() {
                let id = tc
                    .get("id")
                    .and_then(|v| v.as_str())
                    .unwrap_or("call_web")
                    .to_owned();
                let name = tc
                    .get("name")
                    .and_then(|v| v.as_str())?
                    .to_owned();
                let arguments = tc
                    .get("arguments")
                    .cloned()
                    .unwrap_or(Value::Object(Default::default()));

                return Some(ToolCall {
                    id,
                    name,
                    arguments,
                });
            }
        }
    }

    None
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_snapshot_deltas() {
        let mut acc = ChatGptWebStreamAccumulator::new(false);

        // First snapshot.
        let line1 = r#"data: {"message":{"id":"msg1","author":{"role":"assistant"},"content":{"content_type":"text","parts":["Hel"]},"status":"in_progress"},"conversation_id":"conv1"}"#;
        let delta1 = acc.feed_line(line1).unwrap();
        assert_eq!(delta1, Some("Hel".to_owned()));

        // Second snapshot (longer).
        let line2 = r#"data: {"message":{"id":"msg1","author":{"role":"assistant"},"content":{"content_type":"text","parts":["Hello, world!"]},"status":"in_progress"},"conversation_id":"conv1"}"#;
        let delta2 = acc.feed_line(line2).unwrap();
        assert_eq!(delta2, Some("lo, world!".to_owned()));

        // Done.
        let line3 = "data: [DONE]";
        let delta3 = acc.feed_line(line3).unwrap();
        assert_eq!(delta3, None);
        assert!(acc.is_done());

        let (resp, usage) = acc.into_response().unwrap();
        match resp {
            LlmResponse::Text(t) => assert_eq!(t, "Hello, world!"),
            _ => panic!("expected text"),
        }
        assert_eq!(usage.input_tokens, 0);
    }

    #[test]
    fn parse_tool_call() {
        let text = r#"{"tool_call": {"id": "call_1", "name": "web_search", "arguments": {"query": "rust lang"}}}"#;
        let tc = parse_tool_call_from_text(text).unwrap();
        assert_eq!(tc.name, "web_search");
        assert_eq!(tc.arguments["query"], "rust lang");
    }

    #[test]
    fn parse_tool_call_with_surrounding_text() {
        let text = "Let me search for that.\n\
            {\"tool_call\": {\"id\": \"call_2\", \"name\": \"read_file\", \"arguments\": {\"path\": \"/tmp/test\"}}}\n\
            Done.";
        let tc = parse_tool_call_from_text(text).unwrap();
        assert_eq!(tc.name, "read_file");
    }

    #[test]
    fn no_tool_call_in_plain_text() {
        let text = "I don't need any tools for this.";
        assert!(parse_tool_call_from_text(text).is_none());
    }

    #[test]
    fn finished_status_marks_done() {
        let mut acc = ChatGptWebStreamAccumulator::new(false);
        let line = r#"data: {"message":{"id":"msg1","author":{"role":"assistant"},"content":{"content_type":"text","parts":["Done!"]},"status":"finished_successfully"},"conversation_id":"conv1"}"#;
        acc.feed_line(line).unwrap();
        assert!(acc.is_done());
    }

    #[test]
    fn skip_empty_and_event_lines() {
        let mut acc = ChatGptWebStreamAccumulator::new(false);
        assert_eq!(acc.feed_line("").unwrap(), None);
        assert_eq!(acc.feed_line("event: message").unwrap(), None);
        assert_eq!(acc.feed_line("not-data-prefix").unwrap(), None);
    }
}
