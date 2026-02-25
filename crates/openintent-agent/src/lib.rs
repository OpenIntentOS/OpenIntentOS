//! AI agent runtime for OpenIntentOS.
//!
//! This crate implements the intelligent core of OpenIntentOS: the agent that
//! interprets user intents, reasons about how to fulfill them, and executes
//! actions through tool adapters.
//!
//! ## Architecture
//!
//! ```text
//! ┌─────────────┐     ┌──────────┐     ┌──────────┐
//! │   Planner   │────>│ Executor │────>│ Adapters │
//! │ (decompose) │     │ (run)    │     │ (tools)  │
//! └──────┬──────┘     └────┬─────┘     └──────────┘
//!        │                 │
//!        └────── ReAct Loop ──────┐
//!                │                │
//!         ┌──────┴──────┐   ┌────┴─────┐
//!         │  LLM Client │   │ Streaming│
//!         │  (Anthropic)│   │  (SSE)   │
//!         └─────────────┘   └──────────┘
//! ```
//!
//! ## Modules
//!
//! - [`llm`] -- LLM client, model routing, streaming, and wire types.
//! - [`runtime`] -- The ReAct loop and tool adapter trait.
//! - [`planner`] -- Intent decomposition into executable plans.
//! - [`executor`] -- Step-by-step plan execution with retries.
//! - [`compaction`] -- Context window compaction via conversation summarization.
//! - [`error`] -- Agent error types.

pub mod compaction;
pub mod config;
pub mod error;
pub mod evolution;
pub mod executor;
pub mod llm;
pub mod memory;
pub mod planner;
pub mod runtime;

// Re-export the most commonly used types at the crate root.
pub use compaction::{CompactionConfig, compact_messages, needs_compaction};
pub use error::{AgentError, Result};
pub use evolution::{EvolutionConfig, EvolutionEngine, UnhandledIntent};
pub use executor::{Executor, ExecutorConfig, StepResult};
pub use llm::{
    ChatRequest, LlmClient, LlmClientConfig, LlmProvider, LlmResponse, Message, ModelConfig,
    ModelRouter, Role, ToolCall, ToolDefinition, ToolResult,
};
pub use memory::{AutoMemoryConfig, AutoMemoryManager, MemoryEntry, MemoryStore, MemoryType};
pub use planner::{Plan, Planner, PlannerConfig, Step, StepStatus};
pub use runtime::{
    AgentConfig, AgentContext, AgentResponse, PolicyCheckerFn, TextDeltaCallback, ToolAdapter,
    ToolPermission, ToolStartCallback, react_loop,
};
