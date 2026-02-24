//! Core adapter trait and supporting types.
//!
//! Every service adapter (filesystem, shell, email, browser, etc.) implements
//! the [`Adapter`] trait, providing a uniform interface for the agent runtime
//! and intent engine to discover and invoke tools.

use async_trait::async_trait;
use serde::{Deserialize, Serialize};

use crate::error::Result;

// ---------------------------------------------------------------------------
// Supporting types
// ---------------------------------------------------------------------------

/// The category of service an adapter provides.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AdapterType {
    /// Messaging services (Slack, Discord, email, etc.).
    Messaging,
    /// Productivity tools (calendar, documents, project management).
    Productivity,
    /// Developer tools (Git, CI/CD, IDE integration).
    DevTools,
    /// System-level services (filesystem, shell, processes).
    System,
}

impl std::fmt::Display for AdapterType {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Messaging => write!(f, "messaging"),
            Self::Productivity => write!(f, "productivity"),
            Self::DevTools => write!(f, "devtools"),
            Self::System => write!(f, "system"),
        }
    }
}

/// The health status of an adapter.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum HealthStatus {
    /// The adapter is fully operational.
    Healthy,
    /// The adapter is working but with reduced capability or elevated latency.
    Degraded,
    /// The adapter is not functional.
    Unhealthy,
}

impl std::fmt::Display for HealthStatus {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Healthy => write!(f, "healthy"),
            Self::Degraded => write!(f, "degraded"),
            Self::Unhealthy => write!(f, "unhealthy"),
        }
    }
}

/// A tool exposed by an adapter that the agent can invoke.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolDefinition {
    /// Machine-readable tool name (e.g. `fs_read_file`, `shell_execute`).
    pub name: String,
    /// Human-readable description of what the tool does.
    pub description: String,
    /// JSON Schema describing the tool's input parameters.
    pub parameters: serde_json::Value,
}

/// Authentication requirements for an adapter.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AuthRequirement {
    /// The credential provider name (e.g. `github`, `google`, `anthropic`).
    pub provider: String,
    /// The scopes or permissions required.
    pub scopes: Vec<String>,
}

// ---------------------------------------------------------------------------
// Core trait
// ---------------------------------------------------------------------------

/// The universal adapter interface.
///
/// Every service adapter must implement this trait.  The agent runtime
/// discovers available tools via [`Adapter::tools`] and executes them via
/// [`Adapter::execute_tool`].
#[async_trait]
pub trait Adapter: Send + Sync {
    /// Return the unique identifier for this adapter instance.
    fn id(&self) -> &str;

    /// Return the category of service this adapter provides.
    fn adapter_type(&self) -> AdapterType;

    /// Establish a connection to the backing service.
    async fn connect(&mut self) -> Result<()>;

    /// Gracefully disconnect from the backing service.
    async fn disconnect(&mut self) -> Result<()>;

    /// Check whether the adapter is healthy and operational.
    async fn health_check(&self) -> Result<HealthStatus>;

    /// Return the list of tools this adapter exposes.
    fn tools(&self) -> Vec<ToolDefinition>;

    /// Execute a named tool with the given JSON parameters.
    ///
    /// Returns a JSON value representing the tool's output.
    async fn execute_tool(
        &self,
        name: &str,
        params: serde_json::Value,
    ) -> Result<serde_json::Value>;

    /// Return the authentication requirements for this adapter, if any.
    fn required_auth(&self) -> Option<AuthRequirement>;
}
