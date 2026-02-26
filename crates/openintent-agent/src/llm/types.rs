//! Core types for LLM interaction.
//!
//! These types model the data flowing between the agent runtime and LLM
//! providers.  They are provider-agnostic at this layer; the [`super::client`]
//! module translates them into provider-specific wire formats.

use serde::{Deserialize, Serialize};
use serde_json::Value;

// ---------------------------------------------------------------------------
// Messages
// ---------------------------------------------------------------------------

/// The role of a participant in a conversation.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Role {
    /// System-level instructions that shape model behavior.
    System,
    /// Input from the human user.
    User,
    /// Output from the LLM.
    Assistant,
    /// Result of a tool invocation, fed back to the model.
    Tool,
}

/// A single message in a conversation history.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Message {
    /// Who produced this message.
    pub role: Role,

    /// The textual content of the message.
    ///
    /// For [`Role::Tool`] messages this contains the serialized tool result.
    /// For [`Role::Assistant`] messages that contain tool calls only, this
    /// may be empty.
    #[serde(default)]
    pub content: String,

    /// Tool calls requested by the assistant (only present when
    /// `role == Role::Assistant`).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub tool_calls: Vec<ToolCall>,

    /// Identifies which tool call this message is a response to
    /// (only present when `role == Role::Tool`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tool_call_id: Option<String>,
}

impl Message {
    /// Create a system message.
    pub fn system(content: impl Into<String>) -> Self {
        Self {
            role: Role::System,
            content: content.into(),
            tool_calls: Vec::new(),
            tool_call_id: None,
        }
    }

    /// Create a user message.
    pub fn user(content: impl Into<String>) -> Self {
        Self {
            role: Role::User,
            content: content.into(),
            tool_calls: Vec::new(),
            tool_call_id: None,
        }
    }

    /// Create an assistant text message.
    pub fn assistant(content: impl Into<String>) -> Self {
        Self {
            role: Role::Assistant,
            content: content.into(),
            tool_calls: Vec::new(),
            tool_call_id: None,
        }
    }

    /// Create an assistant message that contains tool calls.
    pub fn assistant_tool_calls(tool_calls: Vec<ToolCall>) -> Self {
        Self {
            role: Role::Assistant,
            content: String::new(),
            tool_calls,
            tool_call_id: None,
        }
    }

    /// Create a tool result message.
    pub fn tool_result(tool_call_id: impl Into<String>, content: impl Into<String>) -> Self {
        Self {
            role: Role::Tool,
            content: content.into(),
            tool_calls: Vec::new(),
            tool_call_id: Some(tool_call_id.into()),
        }
    }

    /// Return the text content of the message.
    ///
    /// For messages with tool calls but no text content, returns an empty string.
    pub fn content_text(&self) -> String {
        self.content.clone()
    }
}

// ---------------------------------------------------------------------------
// Tool calls
// ---------------------------------------------------------------------------

/// A tool invocation requested by the LLM.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolCall {
    /// Unique identifier assigned by the LLM for correlating results.
    pub id: String,

    /// The name of the tool to invoke (must match a registered tool).
    pub name: String,

    /// Arguments as a JSON value.  The structure depends on the tool's schema.
    pub arguments: Value,
}

/// The result of executing a tool, ready to feed back to the LLM.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolResult {
    /// The [`ToolCall::id`] this result corresponds to.
    pub tool_call_id: String,

    /// Serialized result content.
    pub content: String,

    /// Whether the tool invocation was successful.
    #[serde(default)]
    pub is_error: bool,
}

// ---------------------------------------------------------------------------
// LLM response
// ---------------------------------------------------------------------------

/// The high-level response from an LLM after processing a turn.
#[derive(Debug, Clone)]
pub enum LlmResponse {
    /// The model produced a final text answer.
    Text(String),

    /// The model wants to invoke one or more tools before continuing.
    ToolCalls(Vec<ToolCall>),
}

// ---------------------------------------------------------------------------
// Chat request
// ---------------------------------------------------------------------------

/// A full request to send to an LLM provider.
#[derive(Debug, Clone, Serialize)]
pub struct ChatRequest {
    /// The model identifier (e.g. `"claude-sonnet-4-20250514"`).
    pub model: String,

    /// The conversation history.
    pub messages: Vec<Message>,

    /// Tool definitions the model may invoke.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub tools: Vec<ToolDefinition>,

    /// Sampling temperature (0.0 = deterministic, 1.0 = creative).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub temperature: Option<f32>,

    /// Maximum tokens the model may generate in this turn.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub max_tokens: Option<u32>,

    /// Whether to use streaming SSE mode.
    #[serde(skip)]
    pub stream: bool,
}

/// A tool definition exposed to the LLM so it knows what tools are available.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolDefinition {
    /// Unique tool name.
    pub name: String,

    /// Human-readable description of what the tool does.
    pub description: String,

    /// JSON Schema describing the tool's input parameters.
    pub input_schema: Value,
}

// ---------------------------------------------------------------------------
// Streaming events
// ---------------------------------------------------------------------------

/// Events emitted during SSE streaming from the Anthropic Messages API.
///
/// These map to the `event:` field in the SSE stream.
#[derive(Debug, Clone)]
pub enum StreamEvent {
    /// The stream has started; contains the message id, model info, and
    /// initial token usage (input tokens are known at stream start).
    MessageStart {
        /// The unique message id from the API.
        message_id: String,
        /// The model that is responding.
        model: String,
        /// Number of input (prompt) tokens billed for this request.
        input_tokens: u32,
    },

    /// A new content block has started.  For text blocks, `content_type` will
    /// be `"text"`.  For tool-use blocks it will be `"tool_use"`.
    ContentBlockStart {
        /// Zero-based index of the content block.
        index: u32,
        /// The type of content block (`"text"` or `"tool_use"`).
        content_type: String,
        /// For tool_use blocks: the tool call id.
        id: Option<String>,
        /// For tool_use blocks: the tool name.
        name: Option<String>,
    },

    /// An incremental text delta within a content block.
    ContentBlockDelta {
        /// The content block index this delta belongs to.
        index: u32,
        /// The delta variant.
        delta: StreamDelta,
    },

    /// A content block has finished streaming.
    ContentBlockStop {
        /// The content block index that stopped.
        index: u32,
    },

    /// The overall message is complete.
    MessageDelta {
        /// The stop reason (`"end_turn"`, `"tool_use"`, `"max_tokens"`, etc.).
        stop_reason: Option<String>,
        /// Number of output tokens generated in this response.
        output_tokens: u32,
    },

    /// The stream has fully terminated.
    MessageStop,

    /// A ping / keepalive event (no payload).
    Ping,
}

/// Incremental delta within a streaming content block.
#[derive(Debug, Clone)]
pub enum StreamDelta {
    /// A chunk of text.
    TextDelta(String),

    /// A chunk of JSON for a tool-use input.
    InputJsonDelta(String),
}

// ---------------------------------------------------------------------------
// Usage tracking
// ---------------------------------------------------------------------------

/// Token usage information returned by the LLM.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct Usage {
    /// Number of tokens in the input (prompt).
    pub input_tokens: u32,
    /// Number of tokens generated by the model.
    pub output_tokens: u32,
}
