//! Service adapters for OpenIntentOS — filesystem, shell, browser, email.
//!
//! Each adapter implements the [`Adapter`] trait defined in [`traits`],
//! providing a uniform interface for tool discovery and execution.

pub mod error;
pub mod filesystem;
pub mod shell;
pub mod traits;

pub use error::{AdapterError, Result};
pub use filesystem::FilesystemAdapter;
pub use shell::ShellAdapter;
pub use traits::{Adapter, AdapterType, AuthRequirement, HealthStatus, ToolDefinition};
