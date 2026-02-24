//! Memory tools adapter -- save, search, list, and delete semantic memories.
//!
//! This adapter exposes the agent's semantic memory layer as tools that the
//! LLM can invoke during a ReAct loop.  It wraps
//! [`openintent_store::SemanticMemory`] and provides four tools:
//!
//! - `memory_save` -- persist a new memory entry.
//! - `memory_search` -- keyword search across stored memories.
//! - `memory_list` -- list memories by category.
//! - `memory_delete` -- remove a memory by ID.

use std::sync::Arc;

use async_trait::async_trait;
use serde_json::{Value, json};
use tracing::{debug, info};

use openintent_store::{MemoryCategory, NewMemory, SemanticMemory};

use crate::error::{AdapterError, Result};
use crate::traits::{Adapter, AdapterType, AuthRequirement, HealthStatus, ToolDefinition};

/// Adapter that exposes semantic memory operations as agent tools.
pub struct MemoryToolsAdapter {
    /// Unique adapter instance identifier.
    id: String,
    /// Whether the adapter has been connected (initialised).
    connected: bool,
    /// Handle to the semantic memory layer.
    memory: Arc<SemanticMemory>,
}

impl MemoryToolsAdapter {
    /// Create a new memory tools adapter backed by the given semantic memory.
    pub fn new(id: impl Into<String>, memory: Arc<SemanticMemory>) -> Self {
        Self {
            id: id.into(),
            connected: false,
            memory,
        }
    }

    /// Parse a category string into a [`MemoryCategory`], returning an
    /// adapter error if the value is invalid.
    fn parse_category(value: &str, tool_name: &str) -> Result<MemoryCategory> {
        match value {
            "preference" => Ok(MemoryCategory::Preference),
            "knowledge" => Ok(MemoryCategory::Knowledge),
            "pattern" => Ok(MemoryCategory::Pattern),
            "skill" => Ok(MemoryCategory::Skill),
            other => Err(AdapterError::InvalidParams {
                tool_name: tool_name.to_string(),
                reason: format!(
                    "invalid category `{other}`: expected one of preference, knowledge, pattern, skill"
                ),
            }),
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

    /// Save a new memory entry.
    async fn tool_memory_save(&self, params: Value) -> Result<Value> {
        let content = Self::require_str(&params, "content", "memory_save")?;
        let category_str = Self::require_str(&params, "category", "memory_save")?;
        let category = Self::parse_category(category_str, "memory_save")?;

        let importance = params
            .get("importance")
            .and_then(|v| v.as_f64())
            .unwrap_or(0.7);

        debug!(category = category_str, importance, "saving memory");

        let id = self
            .memory
            .insert(NewMemory {
                category,
                content: content.to_string(),
                embedding: None,
                importance,
            })
            .await
            .map_err(|e| AdapterError::ExecutionFailed {
                tool_name: "memory_save".to_string(),
                reason: format!("failed to save memory: {e}"),
            })?;

        Ok(json!({ "id": id, "saved": true }))
    }

    /// Search memories by keyword.
    async fn tool_memory_search(&self, params: Value) -> Result<Value> {
        let query = Self::require_str(&params, "query", "memory_search")?;

        let category = match params.get("category").and_then(|v| v.as_str()) {
            Some(cat_str) => Some(Self::parse_category(cat_str, "memory_search")?),
            None => None,
        };

        let limit = params.get("limit").and_then(|v| v.as_u64()).unwrap_or(5) as u32;

        debug!(query, ?category, limit, "searching memories");

        let memories = self
            .memory
            .search_by_keyword(query, category, limit)
            .await
            .map_err(|e| AdapterError::ExecutionFailed {
                tool_name: "memory_search".to_string(),
                reason: format!("search failed: {e}"),
            })?;

        let results: Vec<Value> = memories
            .iter()
            .map(|m| {
                json!({
                    "id": m.id,
                    "content": m.content,
                    "category": format!("{:?}", m.category).to_lowercase(),
                    "importance": m.importance,
                })
            })
            .collect();

        Ok(json!({ "results": results }))
    }

    /// List memories, optionally filtered by category.
    async fn tool_memory_list(&self, params: Value) -> Result<Value> {
        let category = match params.get("category").and_then(|v| v.as_str()) {
            Some(cat_str) => Some(Self::parse_category(cat_str, "memory_list")?),
            None => None,
        };

        let limit = params.get("limit").and_then(|v| v.as_u64()).unwrap_or(10) as u32;

        debug!(?category, limit, "listing memories");

        let memories = self.memory.list_all(category, limit).await.map_err(|e| {
            AdapterError::ExecutionFailed {
                tool_name: "memory_list".to_string(),
                reason: format!("list failed: {e}"),
            }
        })?;

        let results: Vec<Value> = memories
            .iter()
            .map(|m| {
                json!({
                    "id": m.id,
                    "content": m.content,
                    "category": format!("{:?}", m.category).to_lowercase(),
                    "importance": m.importance,
                })
            })
            .collect();

        Ok(json!({ "results": results }))
    }

    /// Delete a memory by ID.
    async fn tool_memory_delete(&self, params: Value) -> Result<Value> {
        let id = params.get("id").and_then(|v| v.as_i64()).ok_or_else(|| {
            AdapterError::InvalidParams {
                tool_name: "memory_delete".to_string(),
                reason: "missing required integer field `id`".to_string(),
            }
        })?;

        debug!(id, "deleting memory");

        self.memory
            .delete(id)
            .await
            .map_err(|e| AdapterError::ExecutionFailed {
                tool_name: "memory_delete".to_string(),
                reason: format!("delete failed: {e}"),
            })?;

        Ok(json!({ "deleted": true }))
    }
}

#[async_trait]
impl Adapter for MemoryToolsAdapter {
    fn id(&self) -> &str {
        &self.id
    }

    fn adapter_type(&self) -> AdapterType {
        AdapterType::System
    }

    async fn connect(&mut self) -> Result<()> {
        info!(id = %self.id, "memory tools adapter connected");
        self.connected = true;
        Ok(())
    }

    async fn disconnect(&mut self) -> Result<()> {
        info!(id = %self.id, "memory tools adapter disconnected");
        self.connected = false;
        Ok(())
    }

    async fn health_check(&self) -> Result<HealthStatus> {
        if !self.connected {
            return Ok(HealthStatus::Unhealthy);
        }
        // Verify we can count memories (quick DB round-trip).
        match self.memory.count(None).await {
            Ok(_) => Ok(HealthStatus::Healthy),
            Err(_) => Ok(HealthStatus::Degraded),
        }
    }

    fn tools(&self) -> Vec<ToolDefinition> {
        vec![
            ToolDefinition {
                name: "memory_save".into(),
                description: "Save a new memory entry to semantic memory".into(),
                parameters: json!({
                    "type": "object",
                    "properties": {
                        "content": {
                            "type": "string",
                            "description": "The content to remember"
                        },
                        "category": {
                            "type": "string",
                            "enum": ["preference", "knowledge", "pattern", "skill"],
                            "description": "The category of the memory"
                        },
                        "importance": {
                            "type": "number",
                            "description": "Importance score from 0.0 to 1.0 (default: 0.7)",
                            "minimum": 0.0,
                            "maximum": 1.0
                        }
                    },
                    "required": ["content", "category"]
                }),
            },
            ToolDefinition {
                name: "memory_search".into(),
                description: "Search memories by keyword".into(),
                parameters: json!({
                    "type": "object",
                    "properties": {
                        "query": {
                            "type": "string",
                            "description": "Keyword query to search for"
                        },
                        "category": {
                            "type": "string",
                            "enum": ["preference", "knowledge", "pattern", "skill"],
                            "description": "Optional category filter"
                        },
                        "limit": {
                            "type": "integer",
                            "description": "Maximum number of results (default: 5)",
                            "minimum": 1,
                            "maximum": 100
                        }
                    },
                    "required": ["query"]
                }),
            },
            ToolDefinition {
                name: "memory_list".into(),
                description: "List memories, optionally filtered by category".into(),
                parameters: json!({
                    "type": "object",
                    "properties": {
                        "category": {
                            "type": "string",
                            "enum": ["preference", "knowledge", "pattern", "skill"],
                            "description": "Optional category filter"
                        },
                        "limit": {
                            "type": "integer",
                            "description": "Maximum number of results (default: 10)",
                            "minimum": 1,
                            "maximum": 100
                        }
                    }
                }),
            },
            ToolDefinition {
                name: "memory_delete".into(),
                description: "Delete a memory by its ID".into(),
                parameters: json!({
                    "type": "object",
                    "properties": {
                        "id": {
                            "type": "integer",
                            "description": "The ID of the memory to delete"
                        }
                    },
                    "required": ["id"]
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
            "memory_save" => self.tool_memory_save(params).await,
            "memory_search" => self.tool_memory_search(params).await,
            "memory_list" => self.tool_memory_list(params).await,
            "memory_delete" => self.tool_memory_delete(params).await,
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
    use openintent_store::Database;

    async fn setup() -> MemoryToolsAdapter {
        let db = Database::open_in_memory().unwrap_or_else(|e| {
            panic!("failed to open in-memory database: {e}");
        });
        db.run_migrations().await.unwrap_or_else(|e| {
            panic!("failed to run migrations: {e}");
        });
        let memory = Arc::new(SemanticMemory::new(db));
        let mut adapter = MemoryToolsAdapter::new("memory-test", memory);
        adapter.connect().await.unwrap_or_else(|e| {
            panic!("failed to connect adapter: {e}");
        });
        adapter
    }

    #[tokio::test]
    async fn memory_tools_adapter_has_four_tools() {
        let adapter = setup().await;
        assert_eq!(adapter.tools().len(), 4);
    }

    #[tokio::test]
    async fn memory_tools_health_when_disconnected() {
        let db = Database::open_in_memory().unwrap_or_else(|e| {
            panic!("failed to open in-memory database: {e}");
        });
        db.run_migrations().await.unwrap_or_else(|e| {
            panic!("failed to run migrations: {e}");
        });
        let memory = Arc::new(SemanticMemory::new(db));
        let adapter = MemoryToolsAdapter::new("memory-test", memory);
        let status = adapter.health_check().await.unwrap_or_else(|e| {
            panic!("health check failed: {e}");
        });
        assert_eq!(status, HealthStatus::Unhealthy);
    }

    #[tokio::test]
    async fn memory_tools_rejects_when_not_connected() {
        let db = Database::open_in_memory().unwrap_or_else(|e| {
            panic!("failed to open in-memory database: {e}");
        });
        db.run_migrations().await.unwrap_or_else(|e| {
            panic!("failed to run migrations: {e}");
        });
        let memory = Arc::new(SemanticMemory::new(db));
        let adapter = MemoryToolsAdapter::new("memory-test", memory);
        let result = adapter
            .execute_tool(
                "memory_save",
                json!({"content": "test", "category": "knowledge"}),
            )
            .await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn memory_save_and_search() {
        let adapter = setup().await;

        // Save a memory.
        let save_result = adapter
            .execute_tool(
                "memory_save",
                json!({
                    "content": "Rust is a systems programming language",
                    "category": "knowledge",
                    "importance": 0.9
                }),
            )
            .await
            .unwrap_or_else(|e| panic!("save failed: {e}"));

        assert_eq!(save_result["saved"], true);
        assert!(save_result["id"].as_i64().is_some());

        // Search for it.
        let search_result = adapter
            .execute_tool("memory_search", json!({ "query": "Rust", "limit": 5 }))
            .await
            .unwrap_or_else(|e| panic!("search failed: {e}"));

        let results = search_result["results"]
            .as_array()
            .unwrap_or_else(|| panic!("results should be an array"));
        assert_eq!(results.len(), 1);
        assert_eq!(results[0]["category"], "knowledge");
    }

    #[tokio::test]
    async fn memory_save_and_list() {
        let adapter = setup().await;

        // Save two memories.
        adapter
            .execute_tool(
                "memory_save",
                json!({"content": "likes dark mode", "category": "preference"}),
            )
            .await
            .unwrap_or_else(|e| panic!("save 1 failed: {e}"));

        adapter
            .execute_tool(
                "memory_save",
                json!({"content": "Rust knowledge", "category": "knowledge"}),
            )
            .await
            .unwrap_or_else(|e| panic!("save 2 failed: {e}"));

        // List all.
        let list_result = adapter
            .execute_tool("memory_list", json!({}))
            .await
            .unwrap_or_else(|e| panic!("list failed: {e}"));

        let results = list_result["results"]
            .as_array()
            .unwrap_or_else(|| panic!("results should be an array"));
        assert_eq!(results.len(), 2);

        // List by category.
        let list_pref = adapter
            .execute_tool("memory_list", json!({"category": "preference"}))
            .await
            .unwrap_or_else(|e| panic!("list prefs failed: {e}"));

        let pref_results = list_pref["results"]
            .as_array()
            .unwrap_or_else(|| panic!("results should be an array"));
        assert_eq!(pref_results.len(), 1);
    }

    #[tokio::test]
    async fn memory_save_and_delete() {
        let adapter = setup().await;

        let save_result = adapter
            .execute_tool(
                "memory_save",
                json!({"content": "to be deleted", "category": "skill"}),
            )
            .await
            .unwrap_or_else(|e| panic!("save failed: {e}"));

        let id = save_result["id"]
            .as_i64()
            .unwrap_or_else(|| panic!("save should return an id"));

        let delete_result = adapter
            .execute_tool("memory_delete", json!({"id": id}))
            .await
            .unwrap_or_else(|e| panic!("delete failed: {e}"));

        assert_eq!(delete_result["deleted"], true);

        // Verify it is gone.
        let list_result = adapter
            .execute_tool("memory_list", json!({}))
            .await
            .unwrap_or_else(|e| panic!("list failed: {e}"));

        let results = list_result["results"]
            .as_array()
            .unwrap_or_else(|| panic!("results should be an array"));
        assert!(results.is_empty());
    }

    #[tokio::test]
    async fn memory_save_invalid_category_fails() {
        let adapter = setup().await;
        let result = adapter
            .execute_tool(
                "memory_save",
                json!({"content": "test", "category": "invalid"}),
            )
            .await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn memory_save_missing_content_fails() {
        let adapter = setup().await;
        let result = adapter
            .execute_tool("memory_save", json!({"category": "knowledge"}))
            .await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn unknown_tool_returns_error() {
        let adapter = setup().await;
        let result = adapter.execute_tool("memory_nonexistent", json!({})).await;
        assert!(result.is_err());
    }
}
