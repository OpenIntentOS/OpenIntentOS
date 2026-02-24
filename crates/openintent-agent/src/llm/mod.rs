//! LLM integration layer.
//!
//! This module provides the interface between the agent runtime and large
//! language model providers.  It is organized into:
//!
//! - [`types`] -- Core data types (messages, tool calls, streaming events).
//! - [`client`] -- HTTP client for Anthropic and OpenAI APIs.
//! - [`router`] -- Complexity-based model routing.
//! - [`streaming`] -- SSE stream parser for Anthropic incremental responses.
//! - [`streaming_openai`] -- SSE stream parser for OpenAI incremental responses.

pub mod client;
pub mod router;
pub mod streaming;
pub mod streaming_openai;
pub mod types;

// Re-export the most commonly used types for convenience.
pub use client::{LlmClient, LlmClientConfig, LlmProvider};
pub use router::{Complexity, ModelConfig, ModelRouter};
pub use types::{
    ChatRequest, LlmResponse, Message, Role, StreamEvent, ToolCall, ToolDefinition, ToolResult,
    Usage,
};
