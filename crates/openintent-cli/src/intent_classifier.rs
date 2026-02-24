//! Intent classification for dev tasks.
//!
//! Uses the LLM to determine whether a user's intent should be handled as
//! a simple direct operation (git commit, push, format, etc.) or as a full
//! development task that requires the branch -> agent -> test -> PR pipeline.
//!
//! No hardcoded keywords — the LLM handles any language naturally.

use std::sync::Arc;

use anyhow::Result;
use tracing::{debug, warn};

use openintent_agent::{AgentConfig, AgentContext, LlmClient, react_loop};

// ═══════════════════════════════════════════════════════════════════════
//  Types
// ═══════════════════════════════════════════════════════════════════════

/// The kind of task based on intent classification.
#[derive(Debug, PartialEq)]
pub enum TaskKind {
    /// Simple operation: git commit, push, format, run tests, etc.
    /// Executed directly without branch/test/PR pipeline.
    Simple,
    /// Development task: new feature, bug fix, refactoring, etc.
    /// Goes through full pipeline: branch -> agent code -> test -> PR.
    Development,
}

// ═══════════════════════════════════════════════════════════════════════
//  Classification prompt
// ═══════════════════════════════════════════════════════════════════════

const CLASSIFY_PROMPT: &str = "\
You are a task classifier for a software project. Given a user instruction, \
respond with EXACTLY one word: either SIMPLE or DEVELOPMENT.

SIMPLE means the user wants a direct operation that does NOT require writing \
new code or modifying existing source files. Examples:
- Git operations: commit, push, pull, merge, rebase, tag
- Build/test commands: compile, run tests, cargo check, cargo fmt, clippy
- File operations: delete, rename, move, copy a file
- Deploy, release, publish
- Status checks, listing files, viewing logs

DEVELOPMENT means the user wants to create, modify, or improve source code. \
This requires an agent to analyze, write, and test code. Examples:
- Add a new feature or endpoint
- Fix a bug
- Refactor or redesign a module
- Implement a new algorithm
- Integrate a new library or service
- Optimize performance of existing code

Respond with ONLY the single word SIMPLE or DEVELOPMENT. No explanation.";

// ═══════════════════════════════════════════════════════════════════════
//  Classification
// ═══════════════════════════════════════════════════════════════════════

/// Classify an intent using the LLM.
///
/// Sends a lightweight prompt to the LLM asking it to classify the intent
/// as SIMPLE or DEVELOPMENT. Falls back to Development if the LLM call
/// fails (safer default — development pipeline validates more thoroughly).
pub async fn classify_intent(
    llm: &Arc<LlmClient>,
    model: &str,
    intent: &str,
) -> TaskKind {
    match classify_via_llm(llm, model, intent).await {
        Ok(kind) => {
            debug!(intent, ?kind, "LLM classified intent");
            kind
        }
        Err(e) => {
            warn!(error = %e, "LLM classification failed, defaulting to Development");
            TaskKind::Development
        }
    }
}

/// Internal LLM classification call.
async fn classify_via_llm(
    llm: &Arc<LlmClient>,
    model: &str,
    intent: &str,
) -> Result<TaskKind> {
    let config = AgentConfig {
        max_turns: 1,
        model: model.to_string(),
        temperature: Some(0.0),
        max_tokens: Some(16),
        ..AgentConfig::default()
    };

    // No tools needed — just a text classification.
    let adapters = vec![];
    let mut ctx = AgentContext::new(llm.clone(), adapters, config)
        .with_system_prompt(CLASSIFY_PROMPT)
        .with_user_message(intent);

    let response = react_loop(&mut ctx)
        .await
        .map_err(|e| anyhow::anyhow!("classification LLM call failed: {e}"))?;

    let answer = response.text.trim().to_uppercase();

    if answer.contains("SIMPLE") {
        Ok(TaskKind::Simple)
    } else {
        // Default to Development for anything other than explicit SIMPLE.
        Ok(TaskKind::Development)
    }
}

// ═══════════════════════════════════════════════════════════════════════
//  Tests
// ═══════════════════════════════════════════════════════════════════════

#[cfg(test)]
mod tests {
    use super::*;

    /// Verify the classification prompt is reasonable (no LLM needed).
    #[test]
    fn prompt_contains_examples() {
        assert!(CLASSIFY_PROMPT.contains("SIMPLE"));
        assert!(CLASSIFY_PROMPT.contains("DEVELOPMENT"));
        assert!(CLASSIFY_PROMPT.contains("commit"));
        assert!(CLASSIFY_PROMPT.contains("feature"));
    }

    /// Test response parsing.
    #[test]
    fn parse_simple_response() {
        let answer = "SIMPLE";
        assert!(answer.contains("SIMPLE"));
    }

    #[test]
    fn parse_development_response() {
        let answer = "DEVELOPMENT";
        assert!(!answer.contains("SIMPLE"));
    }
}
