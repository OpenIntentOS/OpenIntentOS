//! Intent parser — transforms raw user text into structured intents.
//!
//! The parser uses a two-tier approach:
//!
//! 1. **Fast path**: Pattern matching via the kernel's `IntentRouter` for
//!    well-known commands (e.g. "open file X", "run command Y").
//! 2. **Slow path**: Falls back to LLM-based parsing for complex or
//!    ambiguous intents.

use std::collections::HashMap;
use std::sync::Arc;

use serde::{Deserialize, Serialize};
use tracing::{debug, info};

use openintent_agent::{ChatRequest, LlmClient, LlmResponse, Message};

use crate::error::{IntentError, Result};

// ---------------------------------------------------------------------------
// Types
// ---------------------------------------------------------------------------

/// A structured representation of a parsed user intent.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ParsedIntent {
    /// The high-level action (e.g. "fs_read_file", "send_message", "search").
    pub action: String,

    /// Named entities extracted from the intent (e.g. `{"path": "/foo/bar"}`).
    pub entities: HashMap<String, String>,

    /// The original raw text that was parsed.
    pub raw_text: String,

    /// Confidence score between 0.0 and 1.0.
    pub confidence: f64,

    /// Which parsing tier produced this result.
    pub source: ParseSource,
}

/// The tier that produced the parsed intent.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ParseSource {
    /// Matched via fast local pattern matching.
    Router,
    /// Parsed by LLM fallback.
    Llm,
}

// ---------------------------------------------------------------------------
// System prompt for LLM-based intent parsing
// ---------------------------------------------------------------------------

const LLM_SYSTEM_PROMPT: &str = r#"You are an intent parser. Given user text, extract the intent as JSON.

Respond ONLY with a JSON object:
{
  "action": "the_action_name",
  "entities": {"key": "value", ...},
  "confidence": 0.0-1.0
}

Available actions:
- fs_read_file (entities: path)
- fs_write_file (entities: path, content)
- fs_list_directory (entities: path)
- fs_delete (entities: path)
- fs_create_directory (entities: path)
- shell_execute (entities: command)
- web_search (entities: query)
- web_fetch (entities: url)
- http_request (entities: method, url, body)
- memory_save (entities: content, category)
- memory_search (entities: query)
- cron_create (entities: name, schedule, command)
- help (no entities)
- system_status (no entities)
- unknown (for unrecognizable intents)"#;

// ---------------------------------------------------------------------------
// Parser
// ---------------------------------------------------------------------------

/// The intent parser.
///
/// Attempts fast pattern matching first, then falls back to LLM-based
/// parsing for intents that cannot be resolved locally.
pub struct IntentParser {
    /// Minimum confidence threshold.  Intents below this score are rejected.
    confidence_threshold: f64,

    /// Optional LLM client for fallback parsing of complex intents.
    llm: Option<Arc<LlmClient>>,

    /// Model identifier to use for LLM-based parsing requests.
    model: String,
}

impl IntentParser {
    /// Create a new intent parser with the given confidence threshold.
    ///
    /// Without an LLM client, intents that cannot be matched via the fast
    /// path will return low-confidence "unknown" results.
    pub fn new(confidence_threshold: f64) -> Self {
        Self {
            confidence_threshold,
            llm: None,
            model: String::new(),
        }
    }

    /// Create a parser with LLM fallback capability.
    ///
    /// When fast pattern matching fails, the parser will call the provided
    /// LLM to extract structured intent information from the user text.
    pub fn with_llm(
        confidence_threshold: f64,
        llm: Arc<LlmClient>,
        model: impl Into<String>,
    ) -> Self {
        Self {
            confidence_threshold,
            llm: Some(llm),
            model: model.into(),
        }
    }

    /// Parse raw user text into a structured intent.
    ///
    /// This first tries fast local pattern matching.  If no route matches,
    /// it falls back to LLM-based parsing.
    pub async fn parse(&self, text: &str) -> Result<ParsedIntent> {
        let text = text.trim();
        if text.is_empty() {
            return Err(IntentError::ParseFailed {
                reason: "empty intent text".into(),
            });
        }

        debug!(text = text, "parsing intent");

        // Tier 1: Fast local pattern matching.
        if let Some(intent) = self.try_fast_match(text)
            && intent.confidence >= self.confidence_threshold
        {
            info!(
                action = %intent.action,
                confidence = intent.confidence,
                source = ?intent.source,
                "intent parsed via fast path"
            );
            return Ok(intent);
        }

        // Tier 2: LLM fallback.
        if let Some(llm) = &self.llm {
            return self.llm_parse(llm, text).await;
        }

        // No LLM available — return a low-confidence unknown intent.
        let intent = ParsedIntent {
            action: "unknown".into(),
            entities: HashMap::new(),
            raw_text: text.to_string(),
            confidence: 0.5,
            source: ParseSource::Llm,
        };

        if intent.confidence < self.confidence_threshold {
            return Err(IntentError::LowConfidence {
                intent: text.to_string(),
                confidence: intent.confidence,
            });
        }

        info!(
            action = %intent.action,
            confidence = intent.confidence,
            source = ?intent.source,
            "intent parsed via LLM fallback"
        );
        Ok(intent)
    }

    /// Parse intent text using the LLM client.
    ///
    /// Sends a structured prompt to the LLM requesting JSON output, then
    /// parses and validates the response.
    async fn llm_parse(&self, llm: &LlmClient, text: &str) -> Result<ParsedIntent> {
        let request = ChatRequest {
            model: self.model.clone(),
            messages: vec![Message::system(LLM_SYSTEM_PROMPT), Message::user(text)],
            tools: vec![],
            temperature: Some(0.0),
            max_tokens: Some(256),
            stream: false,
        };

        let response = llm
            .chat(&request)
            .await
            .map_err(|e| IntentError::ParseFailed {
                reason: format!("LLM call failed: {e}"),
            })?;

        match response {
            LlmResponse::Text(json_text) => self.parse_llm_json_response(&json_text, text),
            LlmResponse::ToolCalls(_) => Err(IntentError::ParseFailed {
                reason: "LLM returned tool calls instead of text".to_string(),
            }),
        }
    }

    /// Parse the raw JSON text returned by the LLM into a [`ParsedIntent`].
    ///
    /// Handles markdown code-block wrappers that models sometimes emit.
    fn parse_llm_json_response(
        &self,
        json_text: &str,
        original_text: &str,
    ) -> Result<ParsedIntent> {
        // Strip optional markdown code fences.
        let cleaned = json_text.trim();
        let cleaned = cleaned.strip_prefix("```json").unwrap_or(cleaned);
        let cleaned = cleaned.strip_prefix("```").unwrap_or(cleaned);
        let cleaned = cleaned.strip_suffix("```").unwrap_or(cleaned);
        let cleaned = cleaned.trim();

        let parsed: serde_json::Value =
            serde_json::from_str(cleaned).map_err(|e| IntentError::ParseFailed {
                reason: format!("failed to parse LLM response as JSON: {e}"),
            })?;

        let action = parsed["action"].as_str().unwrap_or("unknown").to_string();
        let confidence = parsed["confidence"].as_f64().unwrap_or(0.6);
        let entities = parsed["entities"]
            .as_object()
            .map(|obj| {
                obj.iter()
                    .filter_map(|(k, v)| v.as_str().map(|s| (k.clone(), s.to_string())))
                    .collect()
            })
            .unwrap_or_default();

        let intent = ParsedIntent {
            action,
            entities,
            raw_text: original_text.to_string(),
            confidence,
            source: ParseSource::Llm,
        };

        if intent.confidence < self.confidence_threshold {
            return Err(IntentError::LowConfidence {
                intent: original_text.to_string(),
                confidence: intent.confidence,
            });
        }

        info!(
            action = %intent.action,
            confidence = intent.confidence,
            "intent parsed via LLM fallback"
        );
        Ok(intent)
    }

    /// Attempt fast local pattern matching on the input text.
    ///
    /// Returns `Some(ParsedIntent)` if a well-known pattern matches, or
    /// `None` to signal that LLM fallback is needed.
    fn try_fast_match(&self, text: &str) -> Option<ParsedIntent> {
        let lower = text.to_lowercase();
        let words: Vec<&str> = lower.split_whitespace().collect();

        if words.is_empty() {
            return None;
        }

        // Simple keyword-based matching for common patterns.
        match words[0] {
            "read" | "cat" | "show" | "view" | "open" => {
                if words.len() >= 2 {
                    let mut entities = HashMap::new();
                    entities.insert("path".into(), words[1..].join(" "));
                    return Some(ParsedIntent {
                        action: "fs_read_file".into(),
                        entities,
                        raw_text: text.into(),
                        confidence: 0.85,
                        source: ParseSource::Router,
                    });
                }
            }
            "write" | "save" | "create" => {
                if words.len() >= 2 {
                    let mut entities = HashMap::new();
                    entities.insert("path".into(), words[1..].join(" "));
                    return Some(ParsedIntent {
                        action: "fs_write_file".into(),
                        entities,
                        raw_text: text.into(),
                        confidence: 0.80,
                        source: ParseSource::Router,
                    });
                }
            }
            "run" | "exec" | "execute" => {
                if words.len() >= 2 {
                    let mut entities = HashMap::new();
                    entities.insert("command".into(), words[1..].join(" "));
                    return Some(ParsedIntent {
                        action: "shell_execute".into(),
                        entities,
                        raw_text: text.into(),
                        confidence: 0.90,
                        source: ParseSource::Router,
                    });
                }
            }
            "ls" | "list" | "dir" => {
                let mut entities = HashMap::new();
                let path = if words.len() >= 2 {
                    words[1..].join(" ")
                } else {
                    ".".into()
                };
                entities.insert("path".into(), path);
                return Some(ParsedIntent {
                    action: "fs_list_directory".into(),
                    entities,
                    raw_text: text.into(),
                    confidence: 0.90,
                    source: ParseSource::Router,
                });
            }
            "delete" | "rm" | "remove" => {
                if words.len() >= 2 {
                    let mut entities = HashMap::new();
                    entities.insert("path".into(), words[1..].join(" "));
                    return Some(ParsedIntent {
                        action: "fs_delete".into(),
                        entities,
                        raw_text: text.into(),
                        confidence: 0.85,
                        source: ParseSource::Router,
                    });
                }
            }
            "help" => {
                return Some(ParsedIntent {
                    action: "help".into(),
                    entities: HashMap::new(),
                    raw_text: text.into(),
                    confidence: 1.0,
                    source: ParseSource::Router,
                });
            }
            "status" => {
                return Some(ParsedIntent {
                    action: "system_status".into(),
                    entities: HashMap::new(),
                    raw_text: text.into(),
                    confidence: 0.95,
                    source: ParseSource::Router,
                });
            }
            _ => {}
        }

        None
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn parse_read_file() {
        let parser = IntentParser::new(0.7);
        let intent = parser.parse("read /etc/hosts").await.unwrap();
        assert_eq!(intent.action, "fs_read_file");
        assert_eq!(intent.entities.get("path").unwrap(), "/etc/hosts");
        assert_eq!(intent.source, ParseSource::Router);
    }

    #[tokio::test]
    async fn parse_execute_command() {
        let parser = IntentParser::new(0.7);
        let intent = parser.parse("run git status").await.unwrap();
        assert_eq!(intent.action, "shell_execute");
        assert_eq!(intent.entities.get("command").unwrap(), "git status");
    }

    #[tokio::test]
    async fn parse_list_directory() {
        let parser = IntentParser::new(0.7);
        let intent = parser.parse("ls /tmp").await.unwrap();
        assert_eq!(intent.action, "fs_list_directory");
        assert_eq!(intent.entities.get("path").unwrap(), "/tmp");
    }

    #[tokio::test]
    async fn parse_help() {
        let parser = IntentParser::new(0.7);
        let intent = parser.parse("help").await.unwrap();
        assert_eq!(intent.action, "help");
        assert_eq!(intent.confidence, 1.0);
    }

    #[tokio::test]
    async fn parse_empty_text_fails() {
        let parser = IntentParser::new(0.7);
        let result = parser.parse("").await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn parse_unknown_falls_back_to_llm() {
        let parser = IntentParser::new(0.3);
        let intent = parser.parse("what is the meaning of life").await.unwrap();
        assert_eq!(intent.source, ParseSource::Llm);
    }

    #[tokio::test]
    async fn parse_with_no_llm_returns_low_confidence() {
        let parser = IntentParser::new(0.7);
        // Unknown text without LLM should fail with LowConfidence.
        let result = parser.parse("what is the meaning of life").await;
        assert!(result.is_err());
    }

    #[test]
    fn parser_with_llm_is_constructable() {
        // Verify the with_llm constructor compiles and runs.
        let config =
            openintent_agent::LlmClientConfig::anthropic("test-key", "claude-sonnet-4-20250514");
        let llm = openintent_agent::LlmClient::new(config).unwrap();
        let _parser = IntentParser::with_llm(0.7, Arc::new(llm), "claude-sonnet-4-20250514");
    }

    #[test]
    fn parse_llm_json_plain() {
        let parser = IntentParser::new(0.5);
        let json =
            r#"{"action": "web_search", "entities": {"query": "rust lang"}, "confidence": 0.9}"#;
        let intent = parser
            .parse_llm_json_response(json, "search for rust lang")
            .unwrap();
        assert_eq!(intent.action, "web_search");
        assert_eq!(intent.entities.get("query").unwrap(), "rust lang");
        assert!((intent.confidence - 0.9).abs() < f64::EPSILON);
        assert_eq!(intent.source, ParseSource::Llm);
    }

    #[test]
    fn parse_llm_json_with_code_fence() {
        let parser = IntentParser::new(0.5);
        let json = "```json\n{\"action\": \"help\", \"entities\": {}, \"confidence\": 0.95}\n```";
        let intent = parser.parse_llm_json_response(json, "help me").unwrap();
        assert_eq!(intent.action, "help");
        assert!((intent.confidence - 0.95).abs() < f64::EPSILON);
    }

    #[test]
    fn parse_llm_json_low_confidence_rejected() {
        let parser = IntentParser::new(0.8);
        let json = r#"{"action": "unknown", "entities": {}, "confidence": 0.3}"#;
        let result = parser.parse_llm_json_response(json, "gibberish");
        assert!(result.is_err());
    }

    #[test]
    fn parse_llm_json_invalid() {
        let parser = IntentParser::new(0.5);
        let result = parser.parse_llm_json_response("not json at all", "test");
        assert!(result.is_err());
    }
}
