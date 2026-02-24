//! Intent parser — transforms raw user text into structured intents.
//!
//! The parser uses a two-tier approach:
//!
//! 1. **Fast path**: Pattern matching via the kernel's `IntentRouter` for
//!    well-known commands (e.g. "open file X", "run command Y").
//! 2. **Slow path**: Falls back to LLM-based parsing for complex or
//!    ambiguous intents.

use std::collections::HashMap;

use serde::{Deserialize, Serialize};
use tracing::{debug, info};

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
// Parser
// ---------------------------------------------------------------------------

/// The intent parser.
///
/// Attempts fast pattern matching first, then falls back to LLM-based
/// parsing for intents that cannot be resolved locally.
pub struct IntentParser {
    /// Minimum confidence threshold.  Intents below this score are rejected.
    confidence_threshold: f64,
}

impl IntentParser {
    /// Create a new intent parser with the given confidence threshold.
    pub fn new(confidence_threshold: f64) -> Self {
        Self {
            confidence_threshold,
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
        if let Some(intent) = self.try_fast_match(text) {
            if intent.confidence >= self.confidence_threshold {
                info!(
                    action = %intent.action,
                    confidence = intent.confidence,
                    source = ?intent.source,
                    "intent parsed via fast path"
                );
                return Ok(intent);
            }
        }

        // Tier 2: LLM fallback.
        // TODO: Integrate with the agent's LLM client for complex parsing.
        // For now, return a generic intent to keep the pipeline flowing.
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
        let intent = parser
            .parse("what is the meaning of life")
            .await
            .unwrap();
        assert_eq!(intent.source, ParseSource::Llm);
    }
}
