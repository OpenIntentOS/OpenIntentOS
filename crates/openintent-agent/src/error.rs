//! Agent error types.
//!
//! All agent subsystems surface errors through [`AgentError`].  Each variant
//! carries enough context for callers to decide how to handle the failure.

use uuid::Uuid;

/// Unified error type for the agent runtime.
#[derive(Debug, thiserror::Error)]
pub enum AgentError {
    // -- LLM errors ----------------------------------------------------------
    /// An HTTP request to the LLM provider failed.
    #[error("llm request failed: {reason}")]
    LlmRequestFailed { reason: String },

    /// The LLM response could not be parsed into the expected format.
    #[error("llm response parse error: {reason}")]
    LlmParseFailed { reason: String },

    /// The streaming SSE connection was interrupted or produced invalid data.
    #[error("llm stream error: {reason}")]
    LlmStreamError { reason: String },

    /// No suitable model configuration found for the requested provider.
    #[error("no model configured for provider: {provider}")]
    NoModelConfigured { provider: String },

    /// The API key is missing for a provider that requires one.
    #[error("missing api key for provider: {provider}")]
    MissingApiKey { provider: String },

    // -- Runtime errors ------------------------------------------------------
    /// The ReAct loop exceeded the maximum number of allowed turns.
    #[error("react loop exceeded max turns ({max_turns}) for task {task_id}")]
    MaxTurnsExceeded { task_id: Uuid, max_turns: u32 },

    /// A tool call referenced by the LLM does not exist in the registry.
    #[error("unknown tool: {tool_name}")]
    UnknownTool { tool_name: String },

    /// A tool invocation failed.
    #[error("tool execution failed for `{tool_name}`: {reason}")]
    ToolExecutionFailed { tool_name: String, reason: String },

    // -- Planner errors ------------------------------------------------------
    /// The planner could not decompose the given intent into actionable steps.
    #[error("planning failed for intent: {reason}")]
    PlanningFailed { reason: String },

    // -- Executor errors -----------------------------------------------------
    /// A step execution failed after exhausting retries.
    #[error("step execution failed after {attempts} attempts: {reason}")]
    StepExecutionFailed { attempts: u32, reason: String },

    /// The step references an adapter or tool that is not available.
    #[error("adapter not available: {adapter_id}")]
    AdapterNotAvailable { adapter_id: String },

    // -- Configuration errors ------------------------------------------------
    /// Configuration validation or loading failed.
    #[error("config error: {reason}")]
    ConfigError { reason: String },

    /// Validation failed for input data.
    #[error("validation error: {reason}")]
    ValidationError { reason: String },

    // -- Serialization -------------------------------------------------------
    /// JSON serialization or deserialization failed.
    #[error("json error: {0}")]
    Json(#[from] serde_json::Error),

    // -- Upstream crate errors -----------------------------------------------
    /// An error propagated from the kernel crate.
    #[error("kernel error: {0}")]
    Kernel(#[from] openintent_kernel::KernelError),

    /// File system notification error.
    #[error("notify error: {0}")]
    Notify(#[from] notify::Error),

    // -- Generic -------------------------------------------------------------
    /// Catch-all for unexpected internal errors.  Prefer a typed variant
    /// whenever possible.
    #[error("internal agent error: {0}")]
    Internal(String),
}

/// Convenience alias used throughout the agent crate.
pub type Result<T> = std::result::Result<T, AgentError>;

impl From<reqwest::Error> for AgentError {
    fn from(err: reqwest::Error) -> Self {
        Self::LlmRequestFailed {
            reason: err.to_string(),
        }
    }
}
