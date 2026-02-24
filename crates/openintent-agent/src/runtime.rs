//! Core ReAct loop runtime.
//!
//! Implements the **Reason + Act** loop that drives the AI agent.  The agent
//! sends messages to the LLM, and when the LLM responds with tool calls, the
//! runtime executes them and feeds the results back.  This continues until the
//! LLM produces a final text response or the turn limit is exceeded.

use std::sync::Arc;

use async_trait::async_trait;
use serde_json::Value;
use uuid::Uuid;

use crate::error::{AgentError, Result};
use crate::llm::LlmClient;
use crate::llm::types::{ChatRequest, LlmResponse, Message, ToolCall, ToolDefinition, ToolResult};

// ---------------------------------------------------------------------------
// Tool adapter trait
// ---------------------------------------------------------------------------

/// Trait for components that can execute tool calls on behalf of the agent.
///
/// Adapters (filesystem, shell, browser, etc.) implement this trait so the
/// ReAct loop can invoke their tools uniformly.
#[async_trait]
pub trait ToolAdapter: Send + Sync {
    /// The unique identifier for this adapter.
    fn adapter_id(&self) -> &str;

    /// Returns the tool definitions this adapter exposes to the LLM.
    fn tool_definitions(&self) -> Vec<ToolDefinition>;

    /// Execute a named tool with the given arguments.
    ///
    /// Returns the result as a string suitable for feeding back to the LLM.
    async fn execute(&self, tool_name: &str, arguments: Value) -> Result<String>;
}

// ---------------------------------------------------------------------------
// Agent context
// ---------------------------------------------------------------------------

/// Configuration for the ReAct loop.
#[derive(Debug, Clone)]
pub struct AgentConfig {
    /// Maximum number of ReAct turns (LLM call + tool execution = 1 turn).
    /// Prevents infinite loops.
    pub max_turns: u32,

    /// Model identifier to use for the LLM requests.
    pub model: String,

    /// Optional temperature for sampling.
    pub temperature: Option<f32>,

    /// Optional max tokens per response.
    pub max_tokens: Option<u32>,
}

impl Default for AgentConfig {
    fn default() -> Self {
        Self {
            max_turns: 20,
            model: String::new(),
            temperature: Some(0.0),
            max_tokens: Some(4096),
        }
    }
}

/// Holds the state for a single agent invocation.
///
/// The context accumulates the conversation history and provides access to
/// the LLM client and registered tool adapters.
pub struct AgentContext {
    /// Unique identifier for this agent run.
    pub task_id: Uuid,

    /// Conversation message history.
    pub messages: Vec<Message>,

    /// Tool adapters available for this run.
    pub adapters: Vec<Arc<dyn ToolAdapter>>,

    /// The LLM client to use.
    pub llm: Arc<LlmClient>,

    /// Runtime configuration.
    pub config: AgentConfig,
}

impl AgentContext {
    /// Create a new agent context.
    pub fn new(
        llm: Arc<LlmClient>,
        adapters: Vec<Arc<dyn ToolAdapter>>,
        config: AgentConfig,
    ) -> Self {
        Self {
            task_id: Uuid::now_v7(),
            messages: Vec::new(),
            adapters,
            llm,
            config,
        }
    }

    /// Add a system prompt to the conversation.
    pub fn with_system_prompt(mut self, prompt: impl Into<String>) -> Self {
        self.messages.insert(0, Message::system(prompt));
        self
    }

    /// Add a user message to the conversation.
    pub fn with_user_message(mut self, message: impl Into<String>) -> Self {
        self.messages.push(Message::user(message));
        self
    }

    /// Collect all tool definitions from registered adapters.
    fn all_tool_definitions(&self) -> Vec<ToolDefinition> {
        self.adapters
            .iter()
            .flat_map(|a| a.tool_definitions())
            .collect()
    }

    /// Find the adapter that owns a given tool name.
    fn find_adapter_for_tool(&self, tool_name: &str) -> Option<&Arc<dyn ToolAdapter>> {
        self.adapters
            .iter()
            .find(|a| a.tool_definitions().iter().any(|td| td.name == tool_name))
    }
}

// ---------------------------------------------------------------------------
// Agent response
// ---------------------------------------------------------------------------

/// The final response from an agent invocation.
#[derive(Debug, Clone)]
pub struct AgentResponse {
    /// The final text output from the agent.
    pub text: String,

    /// Number of ReAct turns that were executed.
    pub turns_used: u32,

    /// The task ID for this invocation.
    pub task_id: Uuid,
}

impl AgentResponse {
    /// Create a new agent response.
    pub fn new(text: String, turns_used: u32, task_id: Uuid) -> Self {
        Self {
            text,
            turns_used,
            task_id,
        }
    }
}

// ---------------------------------------------------------------------------
// ReAct loop
// ---------------------------------------------------------------------------

/// Execute the ReAct (Reason + Act) loop.
///
/// 1. Sends the current conversation to the LLM.
/// 2. If the LLM returns tool calls, executes them via adapters.
/// 3. Appends tool results to the conversation.
/// 4. Repeats until the LLM returns a text response or `max_turns` is hit.
///
/// # Errors
///
/// Returns [`AgentError::MaxTurnsExceeded`] if the loop hits the turn limit.
/// Other errors are propagated from the LLM client or tool adapters.
pub async fn react_loop(ctx: &mut AgentContext) -> Result<AgentResponse> {
    let tools = ctx.all_tool_definitions();
    let task_id = ctx.task_id;
    let max_turns = ctx.config.max_turns;

    tracing::info!(
        task_id = %task_id,
        max_turns,
        tool_count = tools.len(),
        "starting ReAct loop"
    );

    for turn in 0..max_turns {
        tracing::debug!(turn, "ReAct turn start");

        // Build the chat request for this turn.
        let request = ChatRequest {
            model: ctx.config.model.clone(),
            messages: ctx.messages.clone(),
            tools: tools.clone(),
            temperature: ctx.config.temperature,
            max_tokens: ctx.config.max_tokens,
            stream: true,
        };

        // Call the LLM.
        let response = ctx.llm.stream_chat(&request).await?;

        match response {
            LlmResponse::Text(text) => {
                tracing::info!(
                    task_id = %task_id,
                    turns = turn + 1,
                    "ReAct loop completed with text response"
                );

                // Append the assistant's final message to history.
                ctx.messages.push(Message::assistant(&text));

                return Ok(AgentResponse::new(text, turn + 1, task_id));
            }

            LlmResponse::ToolCalls(calls) => {
                tracing::info!(
                    task_id = %task_id,
                    turn,
                    tool_count = calls.len(),
                    tools = ?calls.iter().map(|c| &c.name).collect::<Vec<_>>(),
                    "LLM requested tool calls"
                );

                // Append the assistant's tool-call message to history.
                ctx.messages
                    .push(Message::assistant_tool_calls(calls.clone()));

                // Execute all tool calls and collect results.
                let results = execute_tool_calls(&calls, ctx).await?;

                // Append each tool result to the conversation.
                for result in results {
                    ctx.messages
                        .push(Message::tool_result(&result.tool_call_id, &result.content));
                }
            }
        }
    }

    Err(AgentError::MaxTurnsExceeded { task_id, max_turns })
}

/// Execute a batch of tool calls, returning their results.
///
/// Calls are executed concurrently using `tokio::spawn` for parallelism.
async fn execute_tool_calls(calls: &[ToolCall], ctx: &AgentContext) -> Result<Vec<ToolResult>> {
    let mut handles = Vec::with_capacity(calls.len());

    for call in calls {
        let adapter = ctx
            .find_adapter_for_tool(&call.name)
            .ok_or_else(|| AgentError::UnknownTool {
                tool_name: call.name.clone(),
            })?
            .clone();

        let tool_name = call.name.clone();
        let tool_id = call.id.clone();
        let arguments = call.arguments.clone();

        handles.push(tokio::spawn(async move {
            tracing::debug!(tool = %tool_name, id = %tool_id, "executing tool");

            let result = adapter.execute(&tool_name, arguments).await;

            match result {
                Ok(content) => ToolResult {
                    tool_call_id: tool_id,
                    content,
                    is_error: false,
                },
                Err(e) => {
                    tracing::warn!(tool = %tool_name, error = %e, "tool execution failed");
                    ToolResult {
                        tool_call_id: tool_id,
                        content: format!("Error: {e}"),
                        is_error: true,
                    }
                }
            }
        }));
    }

    let mut results = Vec::with_capacity(handles.len());
    for handle in handles {
        let result = handle
            .await
            .map_err(|e| AgentError::Internal(format!("tool execution task panicked: {e}")))?;
        results.push(result);
    }

    Ok(results)
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use crate::llm::types::ToolDefinition;

    struct MockAdapter {
        id: String,
        tools: Vec<ToolDefinition>,
    }

    #[async_trait]
    impl ToolAdapter for MockAdapter {
        fn adapter_id(&self) -> &str {
            &self.id
        }

        fn tool_definitions(&self) -> Vec<ToolDefinition> {
            self.tools.clone()
        }

        async fn execute(&self, tool_name: &str, _arguments: Value) -> Result<String> {
            Ok(format!("mock result for {tool_name}"))
        }
    }

    #[test]
    fn agent_context_collects_tools() {
        let config = AgentConfig::default();
        let llm_config =
            crate::llm::LlmClientConfig::anthropic("test-key", "claude-sonnet-4-20250514");
        let llm = Arc::new(LlmClient::new(llm_config).unwrap());

        let adapter: Arc<dyn ToolAdapter> = Arc::new(MockAdapter {
            id: "test".into(),
            tools: vec![
                ToolDefinition {
                    name: "tool_a".into(),
                    description: "Tool A".into(),
                    input_schema: serde_json::json!({"type": "object"}),
                },
                ToolDefinition {
                    name: "tool_b".into(),
                    description: "Tool B".into(),
                    input_schema: serde_json::json!({"type": "object"}),
                },
            ],
        });

        let ctx = AgentContext::new(llm, vec![adapter], config);
        let tools = ctx.all_tool_definitions();
        assert_eq!(tools.len(), 2);
        assert_eq!(tools[0].name, "tool_a");
        assert_eq!(tools[1].name, "tool_b");
    }

    #[test]
    fn agent_context_finds_adapter_for_tool() {
        let config = AgentConfig::default();
        let llm_config =
            crate::llm::LlmClientConfig::anthropic("test-key", "claude-sonnet-4-20250514");
        let llm = Arc::new(LlmClient::new(llm_config).unwrap());

        let adapter: Arc<dyn ToolAdapter> = Arc::new(MockAdapter {
            id: "fs".into(),
            tools: vec![ToolDefinition {
                name: "read_file".into(),
                description: "Read a file".into(),
                input_schema: serde_json::json!({"type": "object"}),
            }],
        });

        let ctx = AgentContext::new(llm, vec![adapter], config);
        assert!(ctx.find_adapter_for_tool("read_file").is_some());
        assert!(ctx.find_adapter_for_tool("nonexistent").is_none());
    }

    #[test]
    fn agent_context_builder_pattern() {
        let config = AgentConfig::default();
        let llm_config =
            crate::llm::LlmClientConfig::anthropic("test-key", "claude-sonnet-4-20250514");
        let llm = Arc::new(LlmClient::new(llm_config).unwrap());

        let ctx = AgentContext::new(llm, vec![], config)
            .with_system_prompt("You are helpful.")
            .with_user_message("Hello");

        assert_eq!(ctx.messages.len(), 2);
        assert_eq!(ctx.messages[0].role, crate::llm::Role::System);
        assert_eq!(ctx.messages[1].role, crate::llm::Role::User);
    }
}
