//! LLM integration layer.
//!
//! This module provides the interface between the agent runtime and large
//! language model providers.  It is organized into:
//!
//! - [`types`] -- Core data types (messages, tool calls, streaming events).
//! - [`client`] -- HTTP client for the Anthropic Messages API.
//! - [`router`] -- Complexity-based model routing.
//! - [`streaming`] -- SSE stream parser for incremental responses.

pub mod client;
pub mod router;
pub mod streaming;
pub mod types;

// Re-export the most commonly used types for convenience.
pub use client::{LlmClient, LlmClientConfig};
pub use router::{Complexity, ModelConfig, ModelRouter};
pub use types::{
    ChatRequest, LlmResponse, Message, Role, StreamEvent, ToolCall, ToolDefinition, ToolResult,
    Usage,
};
