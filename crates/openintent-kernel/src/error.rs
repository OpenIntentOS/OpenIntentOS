//! Kernel error types.
//!
//! All kernel subsystems surface errors through [`KernelError`], which is the
//! single error type returned by every public API in this crate.  Each variant
//! carries enough context for callers to decide how to handle the failure
//! without inspecting opaque strings.

use uuid::Uuid;

/// Unified error type for the OpenIntentOS micro-kernel.
#[derive(Debug, thiserror::Error)]
pub enum KernelError {
    // -- Scheduler errors ---------------------------------------------------
    /// The referenced task does not exist in the scheduler.
    #[error("task not found: {task_id}")]
    TaskNotFound {
        /// The [`Uuid`] that was looked up.
        task_id: Uuid,
    },

    /// The task has already been cancelled or completed and cannot be
    /// transitioned to the requested state.
    #[error("invalid task state transition for {task_id}: {reason}")]
    InvalidTaskState { task_id: Uuid, reason: String },

    /// The scheduler has been shut down and will not accept new work.
    #[error("scheduler is shut down")]
    SchedulerShutdown,

    // -- IPC errors ---------------------------------------------------------
    /// Publishing an event to the IPC bus failed (e.g. no active receivers).
    #[error("ipc publish failed: {reason}")]
    IpcPublishFailed { reason: String },

    /// Subscribing to the IPC bus failed.
    #[error("ipc subscribe failed: {reason}")]
    IpcSubscribeFailed { reason: String },

    // -- Router errors ------------------------------------------------------
    /// No route matched the given intent text at any level.
    #[error("no route matched intent: {intent}")]
    NoRouteMatched { intent: String },

    /// Building the internal automaton failed (e.g. invalid pattern).
    #[error("router build error: {reason}")]
    RouterBuildError { reason: String },

    /// A regex pattern supplied to the router is invalid.
    #[error("invalid regex pattern `{pattern}`: {reason}")]
    InvalidPattern { pattern: String, reason: String },

    // -- Registry errors ----------------------------------------------------
    /// The requested adapter/service is not registered.
    #[error("adapter not found: {adapter_id}")]
    AdapterNotFound { adapter_id: String },

    /// The adapter is registered but not in a usable state.
    #[error("adapter unavailable: {adapter_id} (status: {status})")]
    AdapterUnavailable { adapter_id: String, status: String },

    // -- Generic ------------------------------------------------------------
    /// Catch-all for unexpected internal errors that don't fit a specific
    /// variant.  Prefer a typed variant whenever possible.
    #[error("internal kernel error: {0}")]
    Internal(String),
}

/// Convenience alias used throughout the kernel crate.
pub type Result<T> = std::result::Result<T, KernelError>;
