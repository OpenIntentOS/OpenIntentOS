//! Dev task persistence for self-development operations.
//!
//! Provides SQLite-backed CRUD operations for development tasks that
//! the AI agent creates to modify its own codebase. Each task tracks
//! its lifecycle from intent through branching, coding, testing, and
//! PR creation. Messages associated with a task capture the conversation
//! history between the user and the agent during task execution.

use chrono::Utc;
use serde::{Deserialize, Serialize};
use tracing::{debug, instrument};
use uuid::Uuid;

use crate::db::Database;
use crate::error::{StoreError, StoreResult};

// ═══════════════════════════════════════════════════════════════════════
//  Types
// ═══════════════════════════════════════════════════════════════════════

/// A persisted development task.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DevTask {
    /// Unique identifier (UUID v7).
    pub id: String,
    /// Where the task originated: telegram, cli, evolution, or api.
    pub source: String,
    /// Telegram chat ID (if originated from Telegram).
    pub chat_id: Option<i64>,
    /// The natural-language intent describing what to build or fix.
    pub intent: String,
    /// Current lifecycle status of the task.
    pub status: String,
    /// Git branch name created for this task.
    pub branch: Option<String>,
    /// URL of the pull request, once created.
    pub pr_url: Option<String>,
    /// Description of the current step being executed.
    pub current_step: Option<String>,
    /// JSON array of progress log entries.
    pub progress_log: serde_json::Value,
    /// Error message if the task failed.
    pub error: Option<String>,
    /// Number of times this task has been retried.
    pub retry_count: i32,
    /// Maximum number of retries allowed.
    pub max_retries: i32,
    /// Unix timestamp when the task was created.
    pub created_at: i64,
    /// Unix timestamp when the task was last updated.
    pub updated_at: i64,
}

/// A message associated with a development task.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DevTaskMessage {
    /// Auto-incrementing row ID.
    pub id: i64,
    /// The task this message belongs to.
    pub task_id: String,
    /// Message role: user, system, agent, or progress.
    pub role: String,
    /// Message content.
    pub content: String,
    /// Unix timestamp when the message was created.
    pub created_at: i64,
}

// ═══════════════════════════════════════════════════════════════════════
//  DevTaskStore
// ═══════════════════════════════════════════════════════════════════════

/// CRUD operations on development tasks and their messages.
#[derive(Clone)]
pub struct DevTaskStore {
    db: Database,
}

impl DevTaskStore {
    /// Create a new dev task store backed by `db`.
    pub fn new(db: Database) -> Self {
        Self { db }
    }

    /// Create a new development task and return the stored record.
    ///
    /// The task starts in `pending` status with an empty progress log.
    #[instrument(skip(self))]
    pub async fn create(
        &self,
        source: &str,
        chat_id: Option<i64>,
        intent: &str,
    ) -> StoreResult<DevTask> {
        let id = Uuid::now_v7().to_string();
        let source = source.to_string();
        let intent = intent.to_string();
        let now = Utc::now().timestamp();

        let task = DevTask {
            id: id.clone(),
            source: source.clone(),
            chat_id,
            intent: intent.clone(),
            status: "pending".to_string(),
            branch: None,
            pr_url: None,
            current_step: None,
            progress_log: serde_json::json!([]),
            error: None,
            retry_count: 0,
            max_retries: 3,
            created_at: now,
            updated_at: now,
        };

        self.db
            .execute(move |conn| {
                conn.execute(
                    "INSERT INTO dev_tasks (id, source, chat_id, intent, status, progress_log, retry_count, max_retries, created_at, updated_at) \
                     VALUES (?1, ?2, ?3, ?4, 'pending', '[]', 0, 3, ?5, ?5)",
                    rusqlite::params![id, source, chat_id, intent, now],
                )?;
                Ok(())
            })
            .await?;

        debug!(task_id = %task.id, source = %task.source, "dev task created");
        Ok(task)
    }

    /// Fetch a single dev task by ID, returning `None` if not found.
    #[instrument(skip(self))]
    pub async fn get(&self, id: &str) -> StoreResult<Option<DevTask>> {
        let id = id.to_string();
        self.db
            .execute(move |conn| {
                let result = conn.query_row(
                    "SELECT id, source, chat_id, intent, status, branch, pr_url, current_step, \
                     progress_log, error, retry_count, max_retries, created_at, updated_at \
                     FROM dev_tasks WHERE id = ?1",
                    rusqlite::params![id],
                    |row| {
                        Ok(DevTaskRow {
                            id: row.get(0)?,
                            source: row.get(1)?,
                            chat_id: row.get(2)?,
                            intent: row.get(3)?,
                            status: row.get(4)?,
                            branch: row.get(5)?,
                            pr_url: row.get(6)?,
                            current_step: row.get(7)?,
                            progress_log: row.get(8)?,
                            error: row.get(9)?,
                            retry_count: row.get(10)?,
                            max_retries: row.get(11)?,
                            created_at: row.get(12)?,
                            updated_at: row.get(13)?,
                        })
                    },
                );
                match result {
                    Ok(row) => row.into_dev_task().map(Some),
                    Err(rusqlite::Error::QueryReturnedNoRows) => Ok(None),
                    Err(e) => Err(StoreError::Sqlite(e)),
                }
            })
            .await
    }

    /// List dev tasks filtered by status, ordered by most recently updated.
    #[instrument(skip(self))]
    pub async fn list_by_status(
        &self,
        status: &str,
        limit: i64,
        offset: i64,
    ) -> StoreResult<Vec<DevTask>> {
        let status = status.to_string();
        self.db
            .execute(move |conn| {
                let mut stmt = conn.prepare(
                    "SELECT id, source, chat_id, intent, status, branch, pr_url, current_step, \
                     progress_log, error, retry_count, max_retries, created_at, updated_at \
                     FROM dev_tasks WHERE status = ?1 ORDER BY updated_at DESC LIMIT ?2 OFFSET ?3",
                )?;
                let rows = stmt
                    .query_map(rusqlite::params![status, limit, offset], |row| {
                        Ok(DevTaskRow {
                            id: row.get(0)?,
                            source: row.get(1)?,
                            chat_id: row.get(2)?,
                            intent: row.get(3)?,
                            status: row.get(4)?,
                            branch: row.get(5)?,
                            pr_url: row.get(6)?,
                            current_step: row.get(7)?,
                            progress_log: row.get(8)?,
                            error: row.get(9)?,
                            retry_count: row.get(10)?,
                            max_retries: row.get(11)?,
                            created_at: row.get(12)?,
                            updated_at: row.get(13)?,
                        })
                    })?
                    .collect::<Result<Vec<_>, _>>()?;

                rows.into_iter().map(|r| r.into_dev_task()).collect()
            })
            .await
    }

    /// List dev tasks for a specific Telegram chat, ordered by most recently updated.
    #[instrument(skip(self))]
    pub async fn list_by_chat(
        &self,
        chat_id: i64,
        limit: i64,
        offset: i64,
    ) -> StoreResult<Vec<DevTask>> {
        self.db
            .execute(move |conn| {
                let mut stmt = conn.prepare(
                    "SELECT id, source, chat_id, intent, status, branch, pr_url, current_step, \
                     progress_log, error, retry_count, max_retries, created_at, updated_at \
                     FROM dev_tasks WHERE chat_id = ?1 ORDER BY updated_at DESC LIMIT ?2 OFFSET ?3",
                )?;
                let rows = stmt
                    .query_map(rusqlite::params![chat_id, limit, offset], |row| {
                        Ok(DevTaskRow {
                            id: row.get(0)?,
                            source: row.get(1)?,
                            chat_id: row.get(2)?,
                            intent: row.get(3)?,
                            status: row.get(4)?,
                            branch: row.get(5)?,
                            pr_url: row.get(6)?,
                            current_step: row.get(7)?,
                            progress_log: row.get(8)?,
                            error: row.get(9)?,
                            retry_count: row.get(10)?,
                            max_retries: row.get(11)?,
                            created_at: row.get(12)?,
                            updated_at: row.get(13)?,
                        })
                    })?
                    .collect::<Result<Vec<_>, _>>()?;

                rows.into_iter().map(|r| r.into_dev_task()).collect()
            })
            .await
    }

    /// Update the status and current step of a dev task.
    ///
    /// Also updates the `updated_at` timestamp.
    #[instrument(skip(self))]
    pub async fn update_status(
        &self,
        id: &str,
        status: &str,
        current_step: Option<&str>,
    ) -> StoreResult<()> {
        let id = id.to_string();
        let status = status.to_string();
        let current_step = current_step.map(|s| s.to_string());
        let now = Utc::now().timestamp();

        self.db
            .execute(move |conn| {
                let updated = conn.execute(
                    "UPDATE dev_tasks SET status = ?2, current_step = ?3, updated_at = ?4 WHERE id = ?1",
                    rusqlite::params![id, status, current_step, now],
                )?;
                if updated == 0 {
                    return Err(StoreError::NotFound {
                        entity: "dev_task",
                        id,
                    });
                }
                Ok(())
            })
            .await
    }

    /// Set the git branch name for a dev task.
    #[instrument(skip(self))]
    pub async fn set_branch(&self, id: &str, branch: &str) -> StoreResult<()> {
        let id = id.to_string();
        let branch = branch.to_string();
        let now = Utc::now().timestamp();

        self.db
            .execute(move |conn| {
                let updated = conn.execute(
                    "UPDATE dev_tasks SET branch = ?2, updated_at = ?3 WHERE id = ?1",
                    rusqlite::params![id, branch, now],
                )?;
                if updated == 0 {
                    return Err(StoreError::NotFound {
                        entity: "dev_task",
                        id,
                    });
                }
                Ok(())
            })
            .await
    }

    /// Set the pull request URL for a dev task.
    #[instrument(skip(self))]
    pub async fn set_pr_url(&self, id: &str, pr_url: &str) -> StoreResult<()> {
        let id = id.to_string();
        let pr_url = pr_url.to_string();
        let now = Utc::now().timestamp();

        self.db
            .execute(move |conn| {
                let updated = conn.execute(
                    "UPDATE dev_tasks SET pr_url = ?2, updated_at = ?3 WHERE id = ?1",
                    rusqlite::params![id, pr_url, now],
                )?;
                if updated == 0 {
                    return Err(StoreError::NotFound {
                        entity: "dev_task",
                        id,
                    });
                }
                Ok(())
            })
            .await
    }

    /// Set the error message for a dev task.
    #[instrument(skip(self, error))]
    pub async fn set_error(&self, id: &str, error: &str) -> StoreResult<()> {
        let id = id.to_string();
        let error = error.to_string();
        let now = Utc::now().timestamp();

        self.db
            .execute(move |conn| {
                let updated = conn.execute(
                    "UPDATE dev_tasks SET error = ?2, updated_at = ?3 WHERE id = ?1",
                    rusqlite::params![id, error, now],
                )?;
                if updated == 0 {
                    return Err(StoreError::NotFound {
                        entity: "dev_task",
                        id,
                    });
                }
                Ok(())
            })
            .await
    }

    /// Increment the retry count for a dev task and return the new value.
    #[instrument(skip(self))]
    pub async fn increment_retry(&self, id: &str) -> StoreResult<i32> {
        let id = id.to_string();
        let now = Utc::now().timestamp();

        self.db
            .execute(move |conn| {
                let updated = conn.execute(
                    "UPDATE dev_tasks SET retry_count = retry_count + 1, updated_at = ?2 WHERE id = ?1",
                    rusqlite::params![id, now],
                )?;
                if updated == 0 {
                    return Err(StoreError::NotFound {
                        entity: "dev_task",
                        id,
                    });
                }

                let new_count: i32 = conn.query_row(
                    "SELECT retry_count FROM dev_tasks WHERE id = ?1",
                    rusqlite::params![id],
                    |row| row.get(0),
                )?;
                Ok(new_count)
            })
            .await
    }

    /// Append an entry to the task's progress log JSON array.
    #[instrument(skip(self, entry))]
    pub async fn append_progress(&self, id: &str, entry: &str) -> StoreResult<()> {
        let id = id.to_string();
        let entry = entry.to_string();
        let now = Utc::now().timestamp();

        self.db
            .execute(move |conn| {
                // Read the current progress_log JSON.
                let current_log: String = conn
                    .query_row(
                        "SELECT progress_log FROM dev_tasks WHERE id = ?1",
                        rusqlite::params![id],
                        |row| row.get(0),
                    )
                    .map_err(|e| match e {
                        rusqlite::Error::QueryReturnedNoRows => StoreError::NotFound {
                            entity: "dev_task",
                            id: id.clone(),
                        },
                        other => StoreError::Sqlite(other),
                    })?;

                // Parse, append, and serialize back.
                let mut log: Vec<serde_json::Value> = serde_json::from_str(&current_log)?;
                log.push(serde_json::Value::String(entry));
                let updated_log = serde_json::to_string(&log)?;

                conn.execute(
                    "UPDATE dev_tasks SET progress_log = ?2, updated_at = ?3 WHERE id = ?1",
                    rusqlite::params![id, updated_log, now],
                )?;

                Ok(())
            })
            .await
    }

    /// Append a message to a dev task's message history.
    ///
    /// Returns the new message's row ID.
    #[instrument(skip(self, content))]
    pub async fn append_message(
        &self,
        task_id: &str,
        role: &str,
        content: &str,
    ) -> StoreResult<i64> {
        let task_id = task_id.to_string();
        let role = role.to_string();
        let content = content.to_string();
        let now = Utc::now().timestamp();

        self.db
            .execute(move |conn| {
                conn.execute(
                    "INSERT INTO dev_task_messages (task_id, role, content, created_at) \
                     VALUES (?1, ?2, ?3, ?4)",
                    rusqlite::params![task_id, role, content, now],
                )?;
                let msg_id = conn.last_insert_rowid();
                Ok(msg_id)
            })
            .await
    }

    /// Get messages for a dev task, ordered by creation time ascending.
    ///
    /// If `limit` is `Some(n)`, returns the most recent `n` messages.
    /// If `limit` is `None`, returns all messages.
    #[instrument(skip(self))]
    pub async fn get_messages(
        &self,
        task_id: &str,
        limit: Option<i64>,
    ) -> StoreResult<Vec<DevTaskMessage>> {
        let task_id = task_id.to_string();
        self.db
            .execute(move |conn| {
                let messages = match limit {
                    Some(n) => {
                        let mut stmt = conn.prepare(
                            "SELECT id, task_id, role, content, created_at \
                             FROM (SELECT * FROM dev_task_messages WHERE task_id = ?1 ORDER BY created_at DESC, id DESC LIMIT ?2) \
                             ORDER BY created_at ASC, id ASC",
                        )?;
                        stmt.query_map(rusqlite::params![task_id, n], |row| {
                            Ok(DevTaskMessage {
                                id: row.get(0)?,
                                task_id: row.get(1)?,
                                role: row.get(2)?,
                                content: row.get(3)?,
                                created_at: row.get(4)?,
                            })
                        })?
                        .collect::<Result<Vec<_>, _>>()?
                    }
                    None => {
                        let mut stmt = conn.prepare(
                            "SELECT id, task_id, role, content, created_at \
                             FROM dev_task_messages WHERE task_id = ?1 ORDER BY created_at ASC, id ASC",
                        )?;
                        stmt.query_map(rusqlite::params![task_id], |row| {
                            Ok(DevTaskMessage {
                                id: row.get(0)?,
                                task_id: row.get(1)?,
                                role: row.get(2)?,
                                content: row.get(3)?,
                                created_at: row.get(4)?,
                            })
                        })?
                        .collect::<Result<Vec<_>, _>>()?
                    }
                };
                Ok(messages)
            })
            .await
    }

    /// List tasks that are in a recoverable in-progress state.
    ///
    /// Returns tasks with status in ('branching', 'coding', 'testing'),
    /// ordered by most recently updated.
    #[instrument(skip(self))]
    pub async fn list_recoverable(&self) -> StoreResult<Vec<DevTask>> {
        self.db
            .execute(move |conn| {
                let mut stmt = conn.prepare(
                    "SELECT id, source, chat_id, intent, status, branch, pr_url, current_step, \
                     progress_log, error, retry_count, max_retries, created_at, updated_at \
                     FROM dev_tasks WHERE status IN ('branching', 'coding', 'testing') \
                     ORDER BY updated_at DESC",
                )?;
                let rows = stmt
                    .query_map([], |row| {
                        Ok(DevTaskRow {
                            id: row.get(0)?,
                            source: row.get(1)?,
                            chat_id: row.get(2)?,
                            intent: row.get(3)?,
                            status: row.get(4)?,
                            branch: row.get(5)?,
                            pr_url: row.get(6)?,
                            current_step: row.get(7)?,
                            progress_log: row.get(8)?,
                            error: row.get(9)?,
                            retry_count: row.get(10)?,
                            max_retries: row.get(11)?,
                            created_at: row.get(12)?,
                            updated_at: row.get(13)?,
                        })
                    })?
                    .collect::<Result<Vec<_>, _>>()?;

                rows.into_iter().map(|r| r.into_dev_task()).collect()
            })
            .await
    }

    /// Cancel a dev task by setting its status to 'cancelled'.
    #[instrument(skip(self))]
    pub async fn cancel(&self, id: &str) -> StoreResult<()> {
        let id = id.to_string();
        let now = Utc::now().timestamp();

        self.db
            .execute(move |conn| {
                let updated = conn.execute(
                    "UPDATE dev_tasks SET status = 'cancelled', updated_at = ?2 WHERE id = ?1",
                    rusqlite::params![id, now],
                )?;
                if updated == 0 {
                    return Err(StoreError::NotFound {
                        entity: "dev_task",
                        id,
                    });
                }
                Ok(())
            })
            .await
    }

    /// Delete a dev task and all its messages (cascade).
    #[instrument(skip(self))]
    pub async fn delete(&self, id: &str) -> StoreResult<()> {
        let id = id.to_string();
        self.db
            .execute(move |conn| {
                let deleted =
                    conn.execute("DELETE FROM dev_tasks WHERE id = ?1", rusqlite::params![id])?;
                if deleted == 0 {
                    return Err(StoreError::NotFound {
                        entity: "dev_task",
                        id,
                    });
                }
                Ok(())
            })
            .await
    }

    /// Find an active (non-terminal) task for a given chat with the same intent.
    ///
    /// Returns the first matching task if one exists. Used to prevent duplicate
    /// tasks when Telegram redelivers messages after a bot restart.
    #[instrument(skip(self))]
    pub async fn find_active_by_intent(
        &self,
        chat_id: i64,
        intent: &str,
    ) -> StoreResult<Option<DevTask>> {
        let intent = intent.to_string();
        self.db
            .execute(move |conn| {
                let result = conn.query_row(
                    "SELECT id, source, chat_id, intent, status, branch, pr_url, current_step, \
                     progress_log, error, retry_count, max_retries, created_at, updated_at \
                     FROM dev_tasks WHERE chat_id = ?1 AND intent = ?2 \
                     AND status NOT IN ('completed', 'failed', 'cancelled') \
                     ORDER BY created_at DESC LIMIT 1",
                    rusqlite::params![chat_id, intent],
                    |row| {
                        Ok(DevTaskRow {
                            id: row.get(0)?,
                            source: row.get(1)?,
                            chat_id: row.get(2)?,
                            intent: row.get(3)?,
                            status: row.get(4)?,
                            branch: row.get(5)?,
                            pr_url: row.get(6)?,
                            current_step: row.get(7)?,
                            progress_log: row.get(8)?,
                            error: row.get(9)?,
                            retry_count: row.get(10)?,
                            max_retries: row.get(11)?,
                            created_at: row.get(12)?,
                            updated_at: row.get(13)?,
                        })
                    },
                );
                match result {
                    Ok(row) => row.into_dev_task().map(Some),
                    Err(rusqlite::Error::QueryReturnedNoRows) => Ok(None),
                    Err(e) => Err(StoreError::Sqlite(e)),
                }
            })
            .await
    }

    /// Count dev tasks with a given status.
    #[instrument(skip(self))]
    pub async fn count_by_status(&self, status: &str) -> StoreResult<i64> {
        let status = status.to_string();
        self.db
            .execute(move |conn| {
                let count: i64 = conn.query_row(
                    "SELECT COUNT(*) FROM dev_tasks WHERE status = ?1",
                    rusqlite::params![status],
                    |row| row.get(0),
                )?;
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
/// parsing inside `|row| { ... }`), then converts to `DevTask`
/// in a second step where we can return `StoreError::Json`.
struct DevTaskRow {
    id: String,
    source: String,
    chat_id: Option<i64>,
    intent: String,
    status: String,
    branch: Option<String>,
    pr_url: Option<String>,
    current_step: Option<String>,
    progress_log: String,
    error: Option<String>,
    retry_count: i32,
    max_retries: i32,
    created_at: i64,
    updated_at: i64,
}

impl DevTaskRow {
    /// Convert raw row strings into a fully deserialized `DevTask`.
    fn into_dev_task(self) -> StoreResult<DevTask> {
        let progress_log: serde_json::Value = serde_json::from_str(&self.progress_log)?;

        Ok(DevTask {
            id: self.id,
            source: self.source,
            chat_id: self.chat_id,
            intent: self.intent,
            status: self.status,
            branch: self.branch,
            pr_url: self.pr_url,
            current_step: self.current_step,
            progress_log,
            error: self.error,
            retry_count: self.retry_count,
            max_retries: self.max_retries,
            created_at: self.created_at,
            updated_at: self.updated_at,
        })
    }
}

// ── tests ────────────────────────────────────────────────────────────

#[cfg(test)]
#[path = "dev_task_store_tests.rs"]
mod tests;
