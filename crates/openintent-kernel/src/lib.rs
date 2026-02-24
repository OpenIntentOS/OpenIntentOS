//! OpenIntentOS Micro-Kernel.
//!
//! This crate provides the foundational kernel services for the OpenIntentOS
//! AI operating system:
//!
//! - **[`scheduler`]** -- Lock-free, priority-aware task scheduler built on
//!   [`crossbeam::queue::SegQueue`] with tokio-driven async execution.
//! - **[`ipc`]** -- Zero-copy publish/subscribe event bus backed by
//!   [`tokio::sync::broadcast`].
//! - **[`router`]** -- 3-level intent router: SIMD exact match (aho-corasick),
//!   regex pattern match with named captures, and LLM fallback.
//! - **[`registry`]** -- Concurrent adapter/service registry using [`DashMap`]
//!   with health-check tracking and status management.
//! - **[`error`]** -- Unified kernel error types via [`thiserror`].
//!
//! All public types are `Send + Sync` and designed for use within a
//! multi-threaded tokio runtime.

pub mod error;
pub mod ipc;
pub mod registry;
pub mod router;
pub mod scheduler;

// Re-export the most commonly used types at the crate root for convenience.
pub use error::{KernelError, Result};
pub use ipc::{Event, IpcBus};
pub use registry::{AdapterInfo, AdapterRegistry, AdapterStatus};
pub use router::{IntentRouter, RouteResult};
pub use scheduler::{
    SchedulePolicy, Scheduler, TaskFn, TaskId, TaskInfo, TaskPriority, TaskStatus,
};
