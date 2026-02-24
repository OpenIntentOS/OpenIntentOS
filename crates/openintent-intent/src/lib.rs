//! Intent parsing and workflow engine for OpenIntentOS.
//!
//! This crate provides:
//!
//! - **Intent parsing**: Two-tier intent resolution (fast local matching +
//!   LLM fallback) via [`parser::IntentParser`].
//! - **Workflow engine**: Multi-step workflow definition and sequential
//!   execution via [`workflow::WorkflowEngine`].
//! - **Trigger system**: Manual, cron, and event-based workflow triggers
//!   via [`trigger::TriggerManager`].

pub mod error;
pub mod parser;
pub mod trigger;
pub mod workflow;

pub use error::{IntentError, Result};
pub use parser::{IntentParser, ParseSource, ParsedIntent};
pub use trigger::{TriggerManager, TriggerType};
pub use workflow::{
    StepResult, Workflow, WorkflowEngine, WorkflowResult, WorkflowStatus, WorkflowStep,
};
