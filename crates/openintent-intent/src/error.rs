//! Intent engine error types.
//!
//! All intent subsystems surface errors through [`IntentError`].  Each variant
//! carries enough context for callers to decide how to handle the failure.

use uuid::Uuid;

/// Unified error type for the intent engine.
#[derive(Debug, thiserror::Error)]
pub enum IntentError {
    // -- Parser errors -------------------------------------------------------
    /// The intent text could not be parsed into a structured intent.
    #[error("failed to parse intent: {reason}")]
    ParseFailed { reason: String },

    /// The confidence score for the parsed intent is below the threshold.
    #[error("low confidence ({confidence:.2}) for intent: {intent}")]
    LowConfidence { intent: String, confidence: f64 },

    // -- Workflow errors ------------------------------------------------------
    /// The referenced workflow does not exist.
    #[error("workflow not found: {workflow_id}")]
    WorkflowNotFound { workflow_id: Uuid },

    /// A workflow step failed to execute.
    #[error("workflow step {step_index} failed: {reason}")]
    StepFailed { step_index: usize, reason: String },

    /// The workflow is in an invalid state for the requested operation.
    #[error("invalid workflow state: {reason}")]
    InvalidWorkflowState { reason: String },

    // -- Trigger errors ------------------------------------------------------
    /// A trigger could not be registered.
    #[error("failed to register trigger: {reason}")]
    TriggerRegistrationFailed { reason: String },

    /// A cron expression is invalid.
    #[error("invalid cron expression `{expression}`: {reason}")]
    InvalidCronExpression { expression: String, reason: String },

    // -- Upstream crate errors -----------------------------------------------
    /// An error propagated from the kernel crate.
    #[error("kernel error: {0}")]
    Kernel(#[from] openintent_kernel::KernelError),

    /// An error propagated from the agent crate.
    #[error("agent error: {0}")]
    Agent(#[from] openintent_agent::AgentError),

    // -- Serialization -------------------------------------------------------
    /// JSON serialization or deserialization failed.
    #[error("json error: {0}")]
    Json(#[from] serde_json::Error),

    // -- Generic -------------------------------------------------------------
    /// Catch-all for unexpected internal errors.
    #[error("internal intent error: {0}")]
    Internal(String),
}

/// Convenience alias used throughout the intent crate.
pub type Result<T> = std::result::Result<T, IntentError>;
