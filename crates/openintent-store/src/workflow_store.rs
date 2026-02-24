//! Workflow persistence for intent-driven automation.
//!
//! Provides SQLite-backed CRUD operations for workflows, including
//! pagination, enable/disable toggles, and name-based lookups.
//! Each workflow stores its step definitions and trigger configuration
//! as JSON, supporting cron, event, and manual triggers.

use chrono::Utc;
use serde::{Deserialize, Serialize};
use tracing::{debug, instrument};
use uuid::Uuid;

use crate::db::Database;
use crate::error::{StoreError, StoreResult};

// ═══════════════════════════════════════════════════════════════════════
//  Types
// ═══════════════════════════════════════════════════════════════════════

/// A persisted workflow definition.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StoredWorkflow {
    /// Unique identifier (UUID v7).
    pub id: String,
    /// Human-readable workflow name.
    pub name: String,
    /// Optional description of what the workflow does.
    pub description: Option<String>,
    /// The raw intent text that generated this workflow.
    pub intent_raw: String,
    /// JSON array of step definitions.
    pub steps: serde_json::Value,
    /// Optional JSON trigger configuration (cron/event/manual).
    pub trigger: Option<serde_json::Value>,
    /// Whether the workflow is enabled for scheduling.
    pub enabled: bool,
    /// Unix timestamp when the workflow was created.
    pub created_at: i64,
    /// Unix timestamp when the workflow was last updated.
    pub updated_at: i64,
}

// ═══════════════════════════════════════════════════════════════════════
//  WorkflowStore
// ═══════════════════════════════════════════════════════════════════════

/// CRUD operations on workflow definitions.
#[derive(Clone)]
pub struct WorkflowStore {
    db: Database,
}

impl WorkflowStore {
    /// Create a new workflow store backed by `db`.
    pub fn new(db: Database) -> Self {
        Self { db }
    }

    /// Create a new workflow and return the stored record.
    ///
    /// Generates a UUID v7 identifier and sets both timestamps to now.
    #[instrument(skip(self, steps, trigger))]
    pub async fn create(
        &self,
        name: &str,
        description: Option<&str>,
        intent_raw: &str,
        steps: serde_json::Value,
        trigger: Option<serde_json::Value>,
    ) -> StoreResult<StoredWorkflow> {
        let id = Uuid::now_v7().to_string();
        let name = name.to_string();
        let description = description.map(|s| s.to_string());
        let intent_raw = intent_raw.to_string();
        let now = Utc::now().timestamp();

        let steps_json = serde_json::to_string(&steps)?;
        let trigger_json = trigger.as_ref().map(serde_json::to_string).transpose()?;

        let workflow = StoredWorkflow {
            id: id.clone(),
            name: name.clone(),
            description: description.clone(),
            intent_raw: intent_raw.clone(),
            steps,
            trigger,
            enabled: true,
            created_at: now,
            updated_at: now,
        };

        self.db
            .execute(move |conn| {
                conn.execute(
                    "INSERT INTO workflows (id, name, description, intent_raw, steps, trigger, enabled, created_at, updated_at) \
                     VALUES (?1, ?2, ?3, ?4, ?5, ?6, 1, ?7, ?7)",
                    rusqlite::params![id, name, description, intent_raw, steps_json, trigger_json, now],
                )?;
                Ok(())
            })
            .await?;

        debug!(workflow_id = %workflow.id, workflow_name = %workflow.name, "workflow created");
        Ok(workflow)
    }

    /// Fetch a single workflow by ID, returning `None` if not found.
    #[instrument(skip(self))]
    pub async fn get(&self, id: &str) -> StoreResult<Option<StoredWorkflow>> {
        let id = id.to_string();
        self.db
            .execute(move |conn| {
                let result = conn.query_row(
                    "SELECT id, name, description, intent_raw, steps, trigger, enabled, created_at, updated_at \
                     FROM workflows WHERE id = ?1",
                    rusqlite::params![id],
                    |row| {
                        Ok(WorkflowRow {
                            id: row.get(0)?,
                            name: row.get(1)?,
                            description: row.get(2)?,
                            intent_raw: row.get(3)?,
                            steps: row.get(4)?,
                            trigger: row.get(5)?,
                            enabled: row.get(6)?,
                            created_at: row.get(7)?,
                            updated_at: row.get(8)?,
                        })
                    },
                );
                match result {
                    Ok(row) => row.into_stored_workflow().map(Some),
                    Err(rusqlite::Error::QueryReturnedNoRows) => Ok(None),
                    Err(e) => Err(StoreError::Sqlite(e)),
                }
            })
            .await
    }

    /// Fetch a single workflow by name, returning `None` if not found.
    #[instrument(skip(self))]
    pub async fn get_by_name(&self, name: &str) -> StoreResult<Option<StoredWorkflow>> {
        let name = name.to_string();
        self.db
            .execute(move |conn| {
                let result = conn.query_row(
                    "SELECT id, name, description, intent_raw, steps, trigger, enabled, created_at, updated_at \
                     FROM workflows WHERE name = ?1",
                    rusqlite::params![name],
                    |row| {
                        Ok(WorkflowRow {
                            id: row.get(0)?,
                            name: row.get(1)?,
                            description: row.get(2)?,
                            intent_raw: row.get(3)?,
                            steps: row.get(4)?,
                            trigger: row.get(5)?,
                            enabled: row.get(6)?,
                            created_at: row.get(7)?,
                            updated_at: row.get(8)?,
                        })
                    },
                );
                match result {
                    Ok(row) => row.into_stored_workflow().map(Some),
                    Err(rusqlite::Error::QueryReturnedNoRows) => Ok(None),
                    Err(e) => Err(StoreError::Sqlite(e)),
                }
            })
            .await
    }

    /// List workflows ordered by most recently updated, with pagination.
    #[instrument(skip(self))]
    pub async fn list(&self, limit: i64, offset: i64) -> StoreResult<Vec<StoredWorkflow>> {
        self.db
            .execute(move |conn| {
                let mut stmt = conn.prepare(
                    "SELECT id, name, description, intent_raw, steps, trigger, enabled, created_at, updated_at \
                     FROM workflows ORDER BY updated_at DESC LIMIT ?1 OFFSET ?2",
                )?;
                let rows = stmt
                    .query_map(rusqlite::params![limit, offset], |row| {
                        Ok(WorkflowRow {
                            id: row.get(0)?,
                            name: row.get(1)?,
                            description: row.get(2)?,
                            intent_raw: row.get(3)?,
                            steps: row.get(4)?,
                            trigger: row.get(5)?,
                            enabled: row.get(6)?,
                            created_at: row.get(7)?,
                            updated_at: row.get(8)?,
                        })
                    })?
                    .collect::<Result<Vec<_>, _>>()?;

                rows.into_iter()
                    .map(|r| r.into_stored_workflow())
                    .collect()
            })
            .await
    }

    /// List all enabled workflows (for the scheduler).
    #[instrument(skip(self))]
    pub async fn list_enabled(&self) -> StoreResult<Vec<StoredWorkflow>> {
        self.db
            .execute(move |conn| {
                let mut stmt = conn.prepare(
                    "SELECT id, name, description, intent_raw, steps, trigger, enabled, created_at, updated_at \
                     FROM workflows WHERE enabled = 1 ORDER BY updated_at DESC",
                )?;
                let rows = stmt
                    .query_map([], |row| {
                        Ok(WorkflowRow {
                            id: row.get(0)?,
                            name: row.get(1)?,
                            description: row.get(2)?,
                            intent_raw: row.get(3)?,
                            steps: row.get(4)?,
                            trigger: row.get(5)?,
                            enabled: row.get(6)?,
                            created_at: row.get(7)?,
                            updated_at: row.get(8)?,
                        })
                    })?
                    .collect::<Result<Vec<_>, _>>()?;

                rows.into_iter()
                    .map(|r| r.into_stored_workflow())
                    .collect()
            })
            .await
    }

    /// Update a workflow's name, description, steps, and trigger.
    ///
    /// Updates the `updated_at` timestamp automatically.
    #[instrument(skip(self, steps, trigger))]
    pub async fn update(
        &self,
        id: &str,
        name: &str,
        description: Option<&str>,
        steps: serde_json::Value,
        trigger: Option<serde_json::Value>,
    ) -> StoreResult<()> {
        let id = id.to_string();
        let name = name.to_string();
        let description = description.map(|s| s.to_string());
        let now = Utc::now().timestamp();

        let steps_json = serde_json::to_string(&steps)?;
        let trigger_json = trigger.as_ref().map(serde_json::to_string).transpose()?;

        self.db
            .execute(move |conn| {
                let updated = conn.execute(
                    "UPDATE workflows SET name = ?2, description = ?3, steps = ?4, trigger = ?5, updated_at = ?6 \
                     WHERE id = ?1",
                    rusqlite::params![id, name, description, steps_json, trigger_json, now],
                )?;
                if updated == 0 {
                    return Err(StoreError::NotFound {
                        entity: "workflow",
                        id,
                    });
                }
                Ok(())
            })
            .await
    }

    /// Toggle a workflow's enabled state.
    #[instrument(skip(self))]
    pub async fn set_enabled(&self, id: &str, enabled: bool) -> StoreResult<()> {
        let id = id.to_string();
        let now = Utc::now().timestamp();

        self.db
            .execute(move |conn| {
                let updated = conn.execute(
                    "UPDATE workflows SET enabled = ?2, updated_at = ?3 WHERE id = ?1",
                    rusqlite::params![id, enabled, now],
                )?;
                if updated == 0 {
                    return Err(StoreError::NotFound {
                        entity: "workflow",
                        id,
                    });
                }
                Ok(())
            })
            .await
    }

    /// Delete a workflow by ID.
    #[instrument(skip(self))]
    pub async fn delete(&self, id: &str) -> StoreResult<()> {
        let id = id.to_string();
        self.db
            .execute(move |conn| {
                let deleted =
                    conn.execute("DELETE FROM workflows WHERE id = ?1", rusqlite::params![id])?;
                if deleted == 0 {
                    return Err(StoreError::NotFound {
                        entity: "workflow",
                        id,
                    });
                }
                Ok(())
            })
            .await
    }

    /// Return the total number of workflows.
    #[instrument(skip(self))]
    pub async fn count(&self) -> StoreResult<i64> {
        self.db
            .execute(|conn| {
                let count: i64 =
                    conn.query_row("SELECT COUNT(*) FROM workflows", [], |row| row.get(0))?;
                Ok(count)
            })
            .await
    }
}

// ═══════════════════════════════════════════════════════════════════════
//  Internal row mapping
// ═══════════════════════════════════════════════════════════════════════

/// Raw row data from SQLite before JSON deserialization.
///
/// Keeps the `rusqlite` row-mapping closure simple (no fallible JSON
/// parsing inside `|row| { ... }`), then converts to `StoredWorkflow`
/// in a second step where we can return `StoreError::Json`.
struct WorkflowRow {
    id: String,
    name: String,
    description: Option<String>,
    intent_raw: String,
    steps: String,
    trigger: Option<String>,
    enabled: bool,
    created_at: i64,
    updated_at: i64,
}

impl WorkflowRow {
    /// Convert raw row strings into a fully deserialized `StoredWorkflow`.
    fn into_stored_workflow(self) -> StoreResult<StoredWorkflow> {
        let steps: serde_json::Value = serde_json::from_str(&self.steps)?;
        let trigger: Option<serde_json::Value> =
            self.trigger.map(|t| serde_json::from_str(&t)).transpose()?;

        Ok(StoredWorkflow {
            id: self.id,
            name: self.name,
            description: self.description,
            intent_raw: self.intent_raw,
            steps,
            trigger,
            enabled: self.enabled,
            created_at: self.created_at,
            updated_at: self.updated_at,
        })
    }
}

// ── tests ────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    /// Create an in-memory database with the workflows table for testing.
    async fn setup_db() -> Database {
        let db = Database::open_in_memory().unwrap();
        db.run_migrations().await.unwrap();
        db
    }

    #[tokio::test]
    async fn create_and_get_roundtrip() {
        let db = setup_db().await;
        let store = WorkflowStore::new(db);

        let steps = json!([{"action": "fetch", "url": "https://example.com"}]);
        let trigger = json!({"type": "cron", "schedule": "0 * * * *"});

        let workflow = store
            .create(
                "my workflow",
                Some("fetches a page every hour"),
                "fetch example.com hourly",
                steps.clone(),
                Some(trigger.clone()),
            )
            .await
            .unwrap();

        assert_eq!(workflow.name, "my workflow");
        assert_eq!(
            workflow.description.as_deref(),
            Some("fetches a page every hour")
        );
        assert_eq!(workflow.intent_raw, "fetch example.com hourly");
        assert_eq!(workflow.steps, steps);
        assert_eq!(workflow.trigger, Some(trigger.clone()));
        assert!(workflow.enabled);
        assert!(workflow.created_at > 0);
        assert_eq!(workflow.created_at, workflow.updated_at);

        let fetched = store.get(&workflow.id).await.unwrap();
        assert!(fetched.is_some());
        let fetched = fetched.unwrap();
        assert_eq!(fetched.id, workflow.id);
        assert_eq!(fetched.name, "my workflow");
        assert_eq!(fetched.steps, steps);
        assert_eq!(fetched.trigger, Some(trigger));
    }

    #[tokio::test]
    async fn get_by_name() {
        let db = setup_db().await;
        let store = WorkflowStore::new(db);

        let steps = json!([{"action": "notify"}]);
        store
            .create("unique-name", None, "notify me", steps, None)
            .await
            .unwrap();

        let found = store.get_by_name("unique-name").await.unwrap();
        assert!(found.is_some());
        assert_eq!(found.unwrap().name, "unique-name");

        let not_found = store.get_by_name("nonexistent").await.unwrap();
        assert!(not_found.is_none());
    }

    #[tokio::test]
    async fn list_with_pagination() {
        let db = setup_db().await;
        let store = WorkflowStore::new(db);

        let steps = json!([]);
        for i in 0..5 {
            store
                .create(
                    &format!("workflow-{i}"),
                    None,
                    "intent",
                    steps.clone(),
                    None,
                )
                .await
                .unwrap();
        }

        let all = store.list(10, 0).await.unwrap();
        assert_eq!(all.len(), 5);

        let page1 = store.list(2, 0).await.unwrap();
        assert_eq!(page1.len(), 2);

        let page2 = store.list(2, 2).await.unwrap();
        assert_eq!(page2.len(), 2);

        let page3 = store.list(2, 4).await.unwrap();
        assert_eq!(page3.len(), 1);

        let empty = store.list(10, 10).await.unwrap();
        assert!(empty.is_empty());
    }

    #[tokio::test]
    async fn list_enabled() {
        let db = setup_db().await;
        let store = WorkflowStore::new(db);

        let steps = json!([]);
        let w1 = store
            .create("enabled-1", None, "intent", steps.clone(), None)
            .await
            .unwrap();
        let w2 = store
            .create("enabled-2", None, "intent", steps.clone(), None)
            .await
            .unwrap();
        let w3 = store
            .create("disabled-1", None, "intent", steps.clone(), None)
            .await
            .unwrap();

        // Disable one workflow.
        store.set_enabled(&w3.id, false).await.unwrap();

        let enabled = store.list_enabled().await.unwrap();
        assert_eq!(enabled.len(), 2);

        let enabled_ids: Vec<&str> = enabled.iter().map(|w| w.id.as_str()).collect();
        assert!(enabled_ids.contains(&w1.id.as_str()));
        assert!(enabled_ids.contains(&w2.id.as_str()));
        assert!(!enabled_ids.contains(&w3.id.as_str()));
    }

    #[tokio::test]
    async fn update_workflow() {
        let db = setup_db().await;
        let store = WorkflowStore::new(db);

        let steps = json!([{"action": "old"}]);
        let workflow = store
            .create("original", Some("old desc"), "intent", steps, None)
            .await
            .unwrap();

        let new_steps = json!([{"action": "new"}, {"action": "extra"}]);
        let new_trigger = json!({"type": "event", "source": "webhook"});

        store
            .update(
                &workflow.id,
                "updated-name",
                Some("new desc"),
                new_steps.clone(),
                Some(new_trigger.clone()),
            )
            .await
            .unwrap();

        let fetched = store.get(&workflow.id).await.unwrap().unwrap();
        assert_eq!(fetched.name, "updated-name");
        assert_eq!(fetched.description.as_deref(), Some("new desc"));
        assert_eq!(fetched.steps, new_steps);
        assert_eq!(fetched.trigger, Some(new_trigger));
        assert!(fetched.updated_at >= workflow.updated_at);
    }

    #[tokio::test]
    async fn update_nonexistent_returns_not_found() {
        let db = setup_db().await;
        let store = WorkflowStore::new(db);

        let result = store
            .update("nonexistent-id", "name", None, json!([]), None)
            .await;

        assert!(result.is_err());
        match result.unwrap_err() {
            StoreError::NotFound { entity, .. } => assert_eq!(entity, "workflow"),
            other => panic!("expected NotFound, got: {other}"),
        }
    }

    #[tokio::test]
    async fn set_enabled_toggle() {
        let db = setup_db().await;
        let store = WorkflowStore::new(db);

        let workflow = store
            .create("togglable", None, "intent", json!([]), None)
            .await
            .unwrap();

        assert!(workflow.enabled);

        store.set_enabled(&workflow.id, false).await.unwrap();
        let fetched = store.get(&workflow.id).await.unwrap().unwrap();
        assert!(!fetched.enabled);

        store.set_enabled(&workflow.id, true).await.unwrap();
        let fetched = store.get(&workflow.id).await.unwrap().unwrap();
        assert!(fetched.enabled);
    }

    #[tokio::test]
    async fn set_enabled_nonexistent_returns_not_found() {
        let db = setup_db().await;
        let store = WorkflowStore::new(db);

        let result = store.set_enabled("nonexistent-id", true).await;
        assert!(result.is_err());
        match result.unwrap_err() {
            StoreError::NotFound { entity, .. } => assert_eq!(entity, "workflow"),
            other => panic!("expected NotFound, got: {other}"),
        }
    }

    #[tokio::test]
    async fn delete_workflow() {
        let db = setup_db().await;
        let store = WorkflowStore::new(db);

        let workflow = store
            .create("to-delete", None, "intent", json!([]), None)
            .await
            .unwrap();

        store.delete(&workflow.id).await.unwrap();

        let fetched = store.get(&workflow.id).await.unwrap();
        assert!(fetched.is_none());
    }

    #[tokio::test]
    async fn delete_nonexistent_returns_not_found() {
        let db = setup_db().await;
        let store = WorkflowStore::new(db);

        let result = store.delete("nonexistent-id").await;
        assert!(result.is_err());
        match result.unwrap_err() {
            StoreError::NotFound { entity, .. } => assert_eq!(entity, "workflow"),
            other => panic!("expected NotFound, got: {other}"),
        }
    }

    #[tokio::test]
    async fn count_workflows() {
        let db = setup_db().await;
        let store = WorkflowStore::new(db);

        assert_eq!(store.count().await.unwrap(), 0);

        store
            .create("wf-1", None, "intent", json!([]), None)
            .await
            .unwrap();
        assert_eq!(store.count().await.unwrap(), 1);

        store
            .create("wf-2", None, "intent", json!([]), None)
            .await
            .unwrap();
        assert_eq!(store.count().await.unwrap(), 2);
    }

    #[tokio::test]
    async fn get_nonexistent_returns_none() {
        let db = setup_db().await;
        let store = WorkflowStore::new(db);

        let result = store.get("nonexistent-id").await.unwrap();
        assert!(result.is_none());
    }

    #[tokio::test]
    async fn create_with_no_description_and_no_trigger() {
        let db = setup_db().await;
        let store = WorkflowStore::new(db);

        let steps = json!(["step1", "step2"]);
        let workflow = store
            .create("minimal", None, "do something", steps.clone(), None)
            .await
            .unwrap();

        assert!(workflow.description.is_none());
        assert!(workflow.trigger.is_none());

        let fetched = store.get(&workflow.id).await.unwrap().unwrap();
        assert!(fetched.description.is_none());
        assert!(fetched.trigger.is_none());
        assert_eq!(fetched.steps, steps);
    }

    #[tokio::test]
    async fn create_duplicate_name_allowed() {
        let db = setup_db().await;
        let store = WorkflowStore::new(db);

        let steps = json!([]);
        let w1 = store
            .create("same-name", None, "intent 1", steps.clone(), None)
            .await
            .unwrap();
        let w2 = store
            .create("same-name", None, "intent 2", steps, None)
            .await
            .unwrap();

        // Both should exist with different IDs.
        assert_ne!(w1.id, w2.id);
        assert_eq!(w1.name, w2.name);

        // get_by_name returns one of them (implementation-dependent which one).
        let found = store.get_by_name("same-name").await.unwrap();
        assert!(found.is_some());

        // Both are countable.
        assert_eq!(store.count().await.unwrap(), 2);
    }

    #[tokio::test]
    async fn list_ordered_by_updated_at_desc() {
        let db = setup_db().await;
        let store = WorkflowStore::new(db);

        let w1 = store
            .create("first", None, "intent", json!([]), None)
            .await
            .unwrap();
        let _w2 = store
            .create("second", None, "intent", json!([]), None)
            .await
            .unwrap();

        // Update the first workflow so it becomes the most recently updated.
        store
            .update(&w1.id, "first-updated", None, json!(["new"]), None)
            .await
            .unwrap();

        let all = store.list(10, 0).await.unwrap();
        assert_eq!(all.len(), 2);
        // The updated workflow should be first.
        assert_eq!(all[0].id, w1.id);
    }
}
