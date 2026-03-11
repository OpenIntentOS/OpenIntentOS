//! LLM integration layer.
//!
//! This module provides the interface between the agent runtime and large
//! language model providers.  It is organized into:
//!
//! - [`types`] -- Core data types (messages, tool calls, streaming events).
//! - [`client`] -- HTTP client for Anthropic, OpenAI, and ChatGPT Web APIs.
//! - [`anthropic`] -- Anthropic Messages API format and streaming.
//! - [`openai`] -- OpenAI Chat Completions API format and streaming.
//! - [`chatgpt_web`] -- ChatGPT Web API (Pro subscribers) format and streaming.
//! - [`router`] -- Complexity-based model routing.
//! - [`streaming`] -- SSE stream parser for Anthropic incremental responses.
//! - [`streaming_openai`] -- SSE stream parser for OpenAI incremental responses.
//! - [`streaming_chatgpt_web`] -- SSE stream parser for ChatGPT Web responses.

pub mod anthropic;
pub mod chatgpt_web;
pub mod client;
pub mod openai;
pub mod router;
pub mod streaming;
pub mod streaming_chatgpt_web;
pub mod streaming_openai;
pub mod types;

// Re-export the most commonly used types for convenience.
pub use client::{LlmClient, LlmClientConfig, LlmProvider};
pub use router::{Complexity, ModelConfig, ModelRouter};
pub use types::{
    ChatRequest, LlmResponse, Message, Role, StreamEvent, ToolCall, ToolDefinition, ToolResult,
    Usage,
};
