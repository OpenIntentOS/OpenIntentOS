//! Context compaction -- summarize old conversation messages to keep token
//! usage manageable during long-running agent sessions.
//!
//! When the conversation history exceeds [`CompactionConfig::max_messages`],
//! the compaction logic:
//!
//! 1. Extracts the system prompt (if any).
//! 2. Takes all messages *except* the most recent `keep_recent` messages.
//! 3. Asks the LLM to produce a concise summary of the older messages.
//! 4. Returns a new message list: `[system_prompt, summary, ...recent]`.
//!
//! This lets the agent maintain long conversations without exceeding the
//! model's context window or accumulating unnecessary cost.

use std::sync::Arc;

use tracing::{debug, info};

use crate::error::{AgentError, Result};
use crate::llm::client::LlmClient;
use crate::llm::types::{ChatRequest, LlmResponse, Message, Role};

// ---------------------------------------------------------------------------
// Configuration
// ---------------------------------------------------------------------------

/// Configuration for context compaction behavior.
#[derive(Debug, Clone)]
pub struct CompactionConfig {
    /// Maximum number of messages before triggering compaction.
    pub max_messages: usize,
    /// Number of recent messages to preserve after compaction.
    pub keep_recent: usize,
    /// Model to use for the summarization request.
    pub model: String,
}

impl Default for CompactionConfig {
    fn default() -> Self {
        Self {
            max_messages: 50,
            keep_recent: 10,
            model: "claude-sonnet-4-20250514".to_owned(),
        }
    }
}

// ---------------------------------------------------------------------------
// Public API
// ---------------------------------------------------------------------------

/// Check whether compaction is needed based on the current message count.
pub fn needs_compaction(messages: &[Message], config: &CompactionConfig) -> bool {
    messages.len() > config.max_messages
}

/// Compact the conversation by summarizing older messages.
///
/// Returns a new message list with:
/// 1. The original system prompt (first message if `role == System`).
/// 2. A system message containing the conversation summary.
/// 3. The most recent `keep_recent` messages from the original history.
///
/// If `keep_recent >= messages.len()` (minus the system prompt), the
/// messages are returned unchanged since there is nothing to summarize.
///
/// # Errors
///
/// Returns an error if the LLM summarization request fails.
pub async fn compact_messages(
    messages: &[Message],
    llm: &Arc<LlmClient>,
    config: &CompactionConfig,
) -> Result<Vec<Message>> {
    if messages.is_empty() {
        return Ok(Vec::new());
    }

    // Step 1: Separate the system prompt from conversation messages.
    let (system_prompt, conversation) = if messages[0].role == Role::System {
        (Some(&messages[0]), &messages[1..])
    } else {
        (None, messages)
    };

    // If there are not enough messages to justify compaction, return as-is.
    if conversation.len() <= config.keep_recent {
        debug!(
            total = messages.len(),
            keep_recent = config.keep_recent,
            "not enough messages to compact, returning as-is"
        );
        return Ok(messages.to_vec());
    }

    // Step 2: Split into old (to summarize) and recent (to keep).
    let split_point = conversation.len() - config.keep_recent;
    let old_messages = &conversation[..split_point];
    let recent_messages = &conversation[split_point..];

    info!(
        old_count = old_messages.len(),
        recent_count = recent_messages.len(),
        "compacting conversation history"
    );

    // Step 3: Format old messages and ask the LLM to summarize.
    let conversation_text = format_messages_for_summary(old_messages);
    let summary = summarize_conversation(&conversation_text, llm, config).await?;

    // Step 4: Build the compacted message list.
    let mut compacted = Vec::with_capacity(2 + recent_messages.len());

    // Preserve the original system prompt.
    if let Some(sys) = system_prompt {
        compacted.push(sys.clone());
    }

    // Insert the summary as a system message.
    compacted.push(Message::system(format!(
        "[Conversation summary of {count} earlier messages]\n{summary}",
        count = old_messages.len(),
    )));

    // Append the recent messages.
    compacted.extend_from_slice(recent_messages);

    info!(
        original = messages.len(),
        compacted = compacted.len(),
        "compaction complete"
    );

    Ok(compacted)
}

// ---------------------------------------------------------------------------
// Internal helpers
// ---------------------------------------------------------------------------

/// Format a slice of messages into a human-readable text block suitable for
/// summarization by the LLM.
fn format_messages_for_summary(messages: &[Message]) -> String {
    let mut buf = String::with_capacity(messages.len() * 200);
    for msg in messages {
        let role_label = match msg.role {
            Role::System => "System",
            Role::User => "User",
            Role::Assistant => "Assistant",
            Role::Tool => "Tool",
        };

        buf.push_str(role_label);
        buf.push_str(": ");

        if !msg.content.is_empty() {
            buf.push_str(&msg.content);
        }

        if !msg.tool_calls.is_empty() {
            for tc in &msg.tool_calls {
                buf.push_str(&format!("\n  [tool_call: {}({})]", tc.name, tc.arguments));
            }
        }

        buf.push('\n');
    }
    buf
}

/// Ask the LLM to produce a concise summary of the conversation text.
async fn summarize_conversation(
    conversation_text: &str,
    llm: &Arc<LlmClient>,
    config: &CompactionConfig,
) -> Result<String> {
    let prompt = format!(
        "Summarize the following conversation concisely, preserving key facts, decisions, \
         tool results, and context needed to continue the conversation. Be factual and brief.\n\n\
         {conversation_text}"
    );

    let request = ChatRequest {
        model: config.model.clone(),
        messages: vec![Message::user(prompt)],
        tools: vec![],
        temperature: Some(0.0),
        max_tokens: Some(1024),
        stream: false,
    };

    debug!(model = %config.model, "requesting conversation summary from LLM");

    let response = llm.chat(&request).await?;

    match response {
        LlmResponse::Text(text) => {
            debug!(summary_len = text.len(), "received conversation summary");
            Ok(text)
        }
        LlmResponse::ToolCalls(_) => Err(AgentError::Internal(
            "summarization request unexpectedly returned tool calls instead of text".to_string(),
        )),
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    fn make_messages(count: usize) -> Vec<Message> {
        let mut msgs = vec![Message::system("You are a helpful assistant.")];
        for i in 0..count {
            if i % 2 == 0 {
                msgs.push(Message::user(format!("User message {i}")));
            } else {
                msgs.push(Message::assistant(format!("Assistant response {i}")));
            }
        }
        msgs
    }

    #[test]
    fn needs_compaction_below_threshold() {
        let config = CompactionConfig {
            max_messages: 50,
            keep_recent: 10,
            model: "test".into(),
        };
        let messages = make_messages(10);
        assert!(!needs_compaction(&messages, &config));
    }

    #[test]
    fn needs_compaction_at_threshold() {
        let config = CompactionConfig {
            max_messages: 10,
            keep_recent: 5,
            model: "test".into(),
        };
        // 1 system + 10 conversation = 11 messages
        let messages = make_messages(10);
        assert!(needs_compaction(&messages, &config));
    }

    #[test]
    fn needs_compaction_above_threshold() {
        let config = CompactionConfig {
            max_messages: 5,
            keep_recent: 3,
            model: "test".into(),
        };
        let messages = make_messages(20);
        assert!(needs_compaction(&messages, &config));
    }

    #[test]
    fn format_messages_produces_readable_output() {
        let messages = vec![
            Message::user("Hello"),
            Message::assistant("Hi there!"),
            Message::user("What is 2+2?"),
            Message::assistant("4"),
        ];

        let text = format_messages_for_summary(&messages);

        assert!(text.contains("User: Hello"));
        assert!(text.contains("Assistant: Hi there!"));
        assert!(text.contains("User: What is 2+2?"));
        assert!(text.contains("Assistant: 4"));
    }

    #[test]
    fn format_messages_includes_tool_calls() {
        let messages = vec![Message {
            role: Role::Assistant,
            content: String::new(),
            tool_calls: vec![crate::llm::types::ToolCall {
                id: "tc_01".into(),
                name: "read_file".into(),
                arguments: serde_json::json!({"path": "/tmp/test"}),
            }],
            tool_call_id: None,
        }];

        let text = format_messages_for_summary(&messages);
        assert!(text.contains("[tool_call: read_file("));
    }

    #[test]
    fn format_messages_empty_input() {
        let text = format_messages_for_summary(&[]);
        assert!(text.is_empty());
    }

    #[tokio::test]
    async fn compact_messages_empty_returns_empty() {
        // We need a real-ish LlmClient but since we pass empty messages,
        // the LLM should never be called.
        let config = crate::llm::LlmClientConfig::anthropic("test-key", "test-model");
        let llm = Arc::new(LlmClient::new(config).unwrap_or_else(|e| {
            panic!("failed to create LLM client: {e}");
        }));

        let compact_config = CompactionConfig::default();
        let result = compact_messages(&[], &llm, &compact_config)
            .await
            .unwrap_or_else(|e| panic!("compact_messages failed: {e}"));
        assert!(result.is_empty());
    }

    #[tokio::test]
    async fn compact_messages_below_keep_recent_returns_as_is() {
        let config = crate::llm::LlmClientConfig::anthropic("test-key", "test-model");
        let llm = Arc::new(LlmClient::new(config).unwrap_or_else(|e| {
            panic!("failed to create LLM client: {e}");
        }));

        let compact_config = CompactionConfig {
            max_messages: 50,
            keep_recent: 20,
            model: "test".into(),
        };

        // 1 system + 5 conversation = 6 messages, well below keep_recent=20.
        let messages = make_messages(5);
        let result = compact_messages(&messages, &llm, &compact_config)
            .await
            .unwrap_or_else(|e| panic!("compact_messages failed: {e}"));
        assert_eq!(result.len(), messages.len());
    }

    #[test]
    fn default_compaction_config_values() {
        let config = CompactionConfig::default();
        assert_eq!(config.max_messages, 50);
        assert_eq!(config.keep_recent, 10);
        assert_eq!(config.model, "claude-sonnet-4-20250514");
    }
}
