//! OpenIntentOS WebAssembly Plugin Sandbox.
//!
//! This crate provides a secure, resource-limited sandbox for running
//! third-party Wasm plugins within the OpenIntentOS AI operating system.
//!
//! - **[`config`]** -- [`SandboxConfig`] controls memory limits, fuel budgets,
//!   execution timeouts, and capability flags.
//! - **[`error`]** -- [`SandboxError`] enumerates every failure mode.
//! - **[`plugin`]** -- [`PluginInfo`], [`PluginTool`], and [`PluginRegistry`]
//!   manage plugin metadata and lifecycle.
//! - **[`runtime`]** -- [`SandboxRuntime`] is the main entry point: load
//!   `.wasm` bytes, invoke tools, enforce limits.
//!
//! All public types are `Send + Sync` and designed for use within a
//! multi-threaded tokio runtime.

pub mod adapter;
pub mod config;
pub mod error;
pub mod loader;
pub mod plugin;
pub mod runtime;

// Re-export the most commonly used types at the crate root.
pub use adapter::PluginAdapter;
pub use config::SandboxConfig;
pub use error::{Result, SandboxError};
pub use loader::PluginLoader;
pub use plugin::{PluginInfo, PluginRegistry, PluginTool};
pub use runtime::SandboxRuntime;
