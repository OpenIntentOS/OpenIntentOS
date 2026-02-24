//! Cron adapter -- schedule, list, delete, and toggle recurring jobs.
//!
//! This adapter provides an in-memory cron job registry for the MVP.
//! Persistence via SQLite can be added in a future iteration.  The registry
//! uses [`RwLock<HashMap>`] for thread-safe concurrent access.
//!
//! **Tools:**
//! - `cron_create` -- register a new cron job.
//! - `cron_list` -- list all registered jobs.
//! - `cron_delete` -- remove a job by ID.
//! - `cron_toggle` -- enable or disable a job.

use std::collections::HashMap;
use std::sync::{Arc, RwLock};

use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use tracing::{debug, info, warn};
use uuid::Uuid;

use crate::error::{AdapterError, Result};
use crate::traits::{Adapter, AdapterType, AuthRequirement, HealthStatus, ToolDefinition};

// ---------------------------------------------------------------------------
// Cron job model
// ---------------------------------------------------------------------------

/// A scheduled recurring job.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CronJob {
    /// Unique job identifier.
    pub id: String,
    /// Human-readable name for the job.
    pub name: String,
    /// Cron expression (e.g. `"0 */5 * * *"`).
    pub schedule: String,
    /// The command or intent to execute when the job fires.
    pub command: String,
    /// Whether the job is active.
    pub enabled: bool,
    /// Unix epoch timestamp when the job was created.
    pub created_at: i64,
    /// Unix epoch timestamp of the most recent execution, if any.
    pub last_run: Option<i64>,
    /// Unix epoch timestamp of the next planned execution, if known.
    pub next_run: Option<i64>,
}

// ---------------------------------------------------------------------------
// Adapter
// ---------------------------------------------------------------------------

/// Cron scheduling adapter backed by an in-memory registry.
pub struct CronAdapter {
    /// Unique adapter instance identifier.
    id: String,
    /// Whether the adapter has been connected (initialised).
    connected: bool,
    /// In-memory job registry.
    jobs: Arc<RwLock<HashMap<String, CronJob>>>,
}

impl CronAdapter {
    /// Create a new cron adapter.
    pub fn new(id: impl Into<String>) -> Self {
        Self {
            id: id.into(),
            connected: false,
            jobs: Arc::new(RwLock::new(HashMap::new())),
        }
    }

    /// Extract a required string field from JSON params.
    fn require_str<'a>(params: &'a Value, field: &str, tool_name: &str) -> Result<&'a str> {
        params
            .get(field)
            .and_then(|v| v.as_str())
            .ok_or_else(|| AdapterError::InvalidParams {
                tool_name: tool_name.to_string(),
                reason: format!("missing required string field `{field}`"),
            })
    }

    // -- Tool implementations ------------------------------------------------

    /// Create a new cron job.
    fn tool_cron_create(&self, params: Value) -> Result<Value> {
        let name = Self::require_str(&params, "name", "cron_create")?;
        let schedule = Self::require_str(&params, "schedule", "cron_create")?;
        let command = Self::require_str(&params, "command", "cron_create")?;

        let job_id = Uuid::now_v7().to_string();
        let now = chrono::Utc::now().timestamp();

        let job = CronJob {
            id: job_id.clone(),
            name: name.to_string(),
            schedule: schedule.to_string(),
            command: command.to_string(),
            enabled: true,
            created_at: now,
            last_run: None,
            next_run: None,
        };

        debug!(job_id = %job_id, name, schedule, "creating cron job");

        let mut jobs = self.jobs.write().map_err(|e| {
            AdapterError::Internal(format!("failed to acquire write lock on cron jobs: {e}"))
        })?;
        jobs.insert(job_id.clone(), job);

        Ok(json!({ "id": job_id, "created": true }))
    }

    /// List all registered cron jobs.
    fn tool_cron_list(&self) -> Result<Value> {
        let jobs = self.jobs.read().map_err(|e| {
            AdapterError::Internal(format!("failed to acquire read lock on cron jobs: {e}"))
        })?;

        let job_list: Vec<Value> = jobs
            .values()
            .map(|job| {
                json!({
                    "id": job.id,
                    "name": job.name,
                    "schedule": job.schedule,
                    "command": job.command,
                    "enabled": job.enabled,
                    "last_run": job.last_run,
                    "next_run": job.next_run,
                })
            })
            .collect();

        Ok(json!({ "jobs": job_list }))
    }

    /// Delete a cron job by ID.
    fn tool_cron_delete(&self, params: Value) -> Result<Value> {
        let job_id = Self::require_str(&params, "id", "cron_delete")?;

        debug!(job_id, "deleting cron job");

        let mut jobs = self.jobs.write().map_err(|e| {
            AdapterError::Internal(format!("failed to acquire write lock on cron jobs: {e}"))
        })?;

        if jobs.remove(job_id).is_none() {
            warn!(job_id, "attempted to delete non-existent cron job");
            return Err(AdapterError::ExecutionFailed {
                tool_name: "cron_delete".to_string(),
                reason: format!("cron job `{job_id}` not found"),
            });
        }

        Ok(json!({ "deleted": true }))
    }

    /// Enable or disable a cron job.
    fn tool_cron_toggle(&self, params: Value) -> Result<Value> {
        let job_id = Self::require_str(&params, "id", "cron_toggle")?;

        let enabled = params
            .get("enabled")
            .and_then(|v| v.as_bool())
            .ok_or_else(|| AdapterError::InvalidParams {
                tool_name: "cron_toggle".to_string(),
                reason: "missing required boolean field `enabled`".to_string(),
            })?;

        debug!(job_id, enabled, "toggling cron job");

        let mut jobs = self.jobs.write().map_err(|e| {
            AdapterError::Internal(format!("failed to acquire write lock on cron jobs: {e}"))
        })?;

        let job = jobs
            .get_mut(job_id)
            .ok_or_else(|| AdapterError::ExecutionFailed {
                tool_name: "cron_toggle".to_string(),
                reason: format!("cron job `{job_id}` not found"),
            })?;

        job.enabled = enabled;

        Ok(json!({ "id": job_id, "enabled": enabled }))
    }
}

#[async_trait]
impl Adapter for CronAdapter {
    fn id(&self) -> &str {
        &self.id
    }

    fn adapter_type(&self) -> AdapterType {
        AdapterType::System
    }

    async fn connect(&mut self) -> Result<()> {
        info!(id = %self.id, "cron adapter connected");
        self.connected = true;
        Ok(())
    }

    async fn disconnect(&mut self) -> Result<()> {
        info!(id = %self.id, "cron adapter disconnected");
        self.connected = false;
        Ok(())
    }

    async fn health_check(&self) -> Result<HealthStatus> {
        if !self.connected {
            return Ok(HealthStatus::Unhealthy);
        }
        Ok(HealthStatus::Healthy)
    }

    fn tools(&self) -> Vec<ToolDefinition> {
        vec![
            ToolDefinition {
                name: "cron_create".into(),
                description: "Create a new recurring cron job".into(),
                parameters: json!({
                    "type": "object",
                    "properties": {
                        "name": {
                            "type": "string",
                            "description": "Human-readable name for the job"
                        },
                        "schedule": {
                            "type": "string",
                            "description": "Cron expression (e.g. '0 */5 * * *')"
                        },
                        "command": {
                            "type": "string",
                            "description": "The command or intent to execute"
                        }
                    },
                    "required": ["name", "schedule", "command"]
                }),
            },
            ToolDefinition {
                name: "cron_list".into(),
                description: "List all registered cron jobs".into(),
                parameters: json!({
                    "type": "object",
                    "properties": {}
                }),
            },
            ToolDefinition {
                name: "cron_delete".into(),
                description: "Delete a cron job by ID".into(),
                parameters: json!({
                    "type": "object",
                    "properties": {
                        "id": {
                            "type": "string",
                            "description": "The ID of the cron job to delete"
                        }
                    },
                    "required": ["id"]
                }),
            },
            ToolDefinition {
                name: "cron_toggle".into(),
                description: "Enable or disable a cron job".into(),
                parameters: json!({
                    "type": "object",
                    "properties": {
                        "id": {
                            "type": "string",
                            "description": "The ID of the cron job to toggle"
                        },
                        "enabled": {
                            "type": "boolean",
                            "description": "Whether to enable (true) or disable (false) the job"
                        }
                    },
                    "required": ["id", "enabled"]
                }),
            },
        ]
    }

    async fn execute_tool(&self, name: &str, params: Value) -> Result<Value> {
        if !self.connected {
            return Err(AdapterError::ExecutionFailed {
                tool_name: name.to_string(),
                reason: format!("adapter `{}` is not connected", self.id),
            });
        }
        match name {
            "cron_create" => self.tool_cron_create(params),
            "cron_list" => self.tool_cron_list(),
            "cron_delete" => self.tool_cron_delete(params),
            "cron_toggle" => self.tool_cron_toggle(params),
            _ => Err(AdapterError::ToolNotFound {
                adapter_id: self.id.clone(),
                tool_name: name.to_string(),
            }),
        }
    }

    fn required_auth(&self) -> Option<AuthRequirement> {
        None
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    async fn setup() -> CronAdapter {
        let mut adapter = CronAdapter::new("cron-test");
        adapter.connect().await.unwrap_or_else(|e| {
            panic!("failed to connect cron adapter: {e}");
        });
        adapter
    }

    #[tokio::test]
    async fn cron_adapter_has_four_tools() {
        let adapter = setup().await;
        assert_eq!(adapter.tools().len(), 4);
    }

    #[tokio::test]
    async fn cron_adapter_health_when_disconnected() {
        let adapter = CronAdapter::new("cron-test");
        let status = adapter.health_check().await.unwrap_or_else(|e| {
            panic!("health check failed: {e}");
        });
        assert_eq!(status, HealthStatus::Unhealthy);
    }

    #[tokio::test]
    async fn cron_adapter_rejects_when_not_connected() {
        let adapter = CronAdapter::new("cron-test");
        let result = adapter
            .execute_tool(
                "cron_create",
                json!({"name": "test", "schedule": "* * * * *", "command": "echo hi"}),
            )
            .await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn cron_create_and_list() {
        let adapter = setup().await;

        let create_result = adapter
            .execute_tool(
                "cron_create",
                json!({
                    "name": "hourly backup",
                    "schedule": "0 * * * *",
                    "command": "backup_all"
                }),
            )
            .await
            .unwrap_or_else(|e| panic!("create failed: {e}"));

        assert_eq!(create_result["created"], true);
        let job_id = create_result["id"]
            .as_str()
            .unwrap_or_else(|| panic!("create should return an id string"));
        assert!(!job_id.is_empty());

        let list_result = adapter
            .execute_tool("cron_list", json!({}))
            .await
            .unwrap_or_else(|e| panic!("list failed: {e}"));

        let jobs = list_result["jobs"]
            .as_array()
            .unwrap_or_else(|| panic!("jobs should be an array"));
        assert_eq!(jobs.len(), 1);
        assert_eq!(jobs[0]["name"], "hourly backup");
        assert_eq!(jobs[0]["schedule"], "0 * * * *");
        assert_eq!(jobs[0]["enabled"], true);
    }

    #[tokio::test]
    async fn cron_create_and_delete() {
        let adapter = setup().await;

        let create_result = adapter
            .execute_tool(
                "cron_create",
                json!({"name": "temp job", "schedule": "* * * * *", "command": "noop"}),
            )
            .await
            .unwrap_or_else(|e| panic!("create failed: {e}"));

        let job_id = create_result["id"]
            .as_str()
            .unwrap_or_else(|| panic!("create should return an id"));

        let delete_result = adapter
            .execute_tool("cron_delete", json!({"id": job_id}))
            .await
            .unwrap_or_else(|e| panic!("delete failed: {e}"));

        assert_eq!(delete_result["deleted"], true);

        // Verify it is gone.
        let list_result = adapter
            .execute_tool("cron_list", json!({}))
            .await
            .unwrap_or_else(|e| panic!("list failed: {e}"));

        let jobs = list_result["jobs"]
            .as_array()
            .unwrap_or_else(|| panic!("jobs should be an array"));
        assert!(jobs.is_empty());
    }

    #[tokio::test]
    async fn cron_delete_nonexistent_fails() {
        let adapter = setup().await;
        let result = adapter
            .execute_tool("cron_delete", json!({"id": "nonexistent-id"}))
            .await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn cron_toggle_enable_disable() {
        let adapter = setup().await;

        let create_result = adapter
            .execute_tool(
                "cron_create",
                json!({"name": "toggle test", "schedule": "0 0 * * *", "command": "daily_report"}),
            )
            .await
            .unwrap_or_else(|e| panic!("create failed: {e}"));

        let job_id = create_result["id"]
            .as_str()
            .unwrap_or_else(|| panic!("create should return an id"));

        // Disable.
        let toggle_result = adapter
            .execute_tool("cron_toggle", json!({"id": job_id, "enabled": false}))
            .await
            .unwrap_or_else(|e| panic!("toggle failed: {e}"));

        assert_eq!(toggle_result["enabled"], false);

        // Verify via list.
        let list_result = adapter
            .execute_tool("cron_list", json!({}))
            .await
            .unwrap_or_else(|e| panic!("list failed: {e}"));

        let jobs = list_result["jobs"]
            .as_array()
            .unwrap_or_else(|| panic!("jobs should be an array"));
        assert_eq!(jobs[0]["enabled"], false);

        // Re-enable.
        let toggle_result = adapter
            .execute_tool("cron_toggle", json!({"id": job_id, "enabled": true}))
            .await
            .unwrap_or_else(|e| panic!("re-enable failed: {e}"));

        assert_eq!(toggle_result["enabled"], true);
    }

    #[tokio::test]
    async fn cron_toggle_nonexistent_fails() {
        let adapter = setup().await;
        let result = adapter
            .execute_tool("cron_toggle", json!({"id": "nope", "enabled": true}))
            .await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn cron_create_missing_name_fails() {
        let adapter = setup().await;
        let result = adapter
            .execute_tool(
                "cron_create",
                json!({"schedule": "* * * * *", "command": "echo hi"}),
            )
            .await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn unknown_tool_returns_error() {
        let adapter = setup().await;
        let result = adapter.execute_tool("cron_nonexistent", json!({})).await;
        assert!(result.is_err());
    }
}
