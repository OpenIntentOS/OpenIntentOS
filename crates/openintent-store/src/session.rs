//! Session persistence for conversation history.
//!
//! Provides SQLite-backed storage for conversation sessions and their
//! messages. Each session tracks the model used, message count, and
//! approximate token usage. Messages within a session are ordered by
//! creation time and can be compacted via summarization.

use chrono::Utc;
use serde::{Deserialize, Serialize};
use tracing::{debug, instrument};
use uuid::Uuid;

use crate::db::Database;
use crate::error::{StoreError, StoreResult};

// ═══════════════════════════════════════════════════════════════════════
//  Types
// ═══════════════════════════════════════════════════════════════════════

/// A conversation session.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Session {
    /// Unique identifier (UUID v7).
    pub id: String,
    /// User-friendly session name.
    pub name: String,
    /// Model used for this session.
    pub model: String,
    /// Number of messages in this session.
    pub message_count: i64,
    /// Approximate token usage across all messages.
    pub token_count: i64,
    /// Unix timestamp when the session was created.
    pub created_at: i64,
    /// Unix timestamp when the session was last updated.
    pub updated_at: i64,
}

/// A single message within a session.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SessionMessage {
    /// Auto-incrementing row ID.
    pub id: i64,
    /// The session this message belongs to.
    pub session_id: String,
    /// Message role: "system", "user", "assistant", or "tool_result".
    pub role: String,
    /// JSON-serialized message content.
    pub content: String,
    /// JSON-serialized tool calls (for assistant messages).
    pub tool_calls: Option<String>,
    /// Tool call ID (for tool_result messages).
    pub tool_call_id: Option<String>,
    /// Unix timestamp when the message was created.
    pub created_at: i64,
}

// ═══════════════════════════════════════════════════════════════════════
//  SessionStore
// ═══════════════════════════════════════════════════════════════════════

/// CRUD operations on conversation sessions and their messages.
#[derive(Clone)]
pub struct SessionStore {
    db: Database,
}

impl SessionStore {
    /// Create a new session store backed by `db`.
    pub fn new(db: Database) -> Self {
        Self { db }
    }

    /// Create a new session with the given name and model.
    #[instrument(skip(self))]
    pub async fn create(&self, name: &str, model: &str) -> StoreResult<Session> {
        let id = Uuid::now_v7().to_string();
        let name = name.to_string();
        let model = model.to_string();
        let now = Utc::now().timestamp();

        let session = Session {
            id: id.clone(),
            name: name.clone(),
            model: model.clone(),
            message_count: 0,
            token_count: 0,
            created_at: now,
            updated_at: now,
        };

        self.db
            .execute(move |conn| {
                conn.execute(
                    "INSERT INTO sessions (id, name, model, message_count, token_count, created_at, updated_at) \
                     VALUES (?1, ?2, ?3, 0, 0, ?4, ?4)",
                    rusqlite::params![id, name, model, now],
                )?;
                Ok(())
            })
            .await?;

        debug!(session_id = %session.id, "session created");
        Ok(session)
    }

    /// Fetch a single session by ID.
    #[instrument(skip(self))]
    pub async fn get(&self, id: &str) -> StoreResult<Session> {
        let id = id.to_string();
        self.db
            .execute(move |conn| {
                conn.query_row(
                    "SELECT id, name, model, message_count, token_count, created_at, updated_at \
                     FROM sessions WHERE id = ?1",
                    rusqlite::params![id],
                    |row| {
                        Ok(Session {
                            id: row.get(0)?,
                            name: row.get(1)?,
                            model: row.get(2)?,
                            message_count: row.get(3)?,
                            token_count: row.get(4)?,
                            created_at: row.get(5)?,
                            updated_at: row.get(6)?,
                        })
                    },
                )
                .map_err(|e| match e {
                    rusqlite::Error::QueryReturnedNoRows => StoreError::NotFound {
                        entity: "session",
                        id: id.clone(),
                    },
                    other => StoreError::Sqlite(other),
                })
            })
            .await
    }

    /// List sessions ordered by most recently updated, with pagination.
    #[instrument(skip(self))]
    pub async fn list(&self, limit: u32, offset: u32) -> StoreResult<Vec<Session>> {
        self.db
            .execute(move |conn| {
                let mut stmt = conn.prepare(
                    "SELECT id, name, model, message_count, token_count, created_at, updated_at \
                     FROM sessions ORDER BY updated_at DESC LIMIT ?1 OFFSET ?2",
                )?;
                let rows = stmt
                    .query_map(rusqlite::params![limit, offset], |row| {
                        Ok(Session {
                            id: row.get(0)?,
                            name: row.get(1)?,
                            model: row.get(2)?,
                            message_count: row.get(3)?,
                            token_count: row.get(4)?,
                            created_at: row.get(5)?,
                            updated_at: row.get(6)?,
                        })
                    })?
                    .collect::<Result<Vec<_>, _>>()?;
                Ok(rows)
            })
            .await
    }

    /// Get the most recently updated session, if any.
    #[instrument(skip(self))]
    pub async fn get_latest(&self) -> StoreResult<Option<Session>> {
        self.db
            .execute(move |conn| {
                let result = conn.query_row(
                    "SELECT id, name, model, message_count, token_count, created_at, updated_at \
                     FROM sessions ORDER BY updated_at DESC LIMIT 1",
                    [],
                    |row| {
                        Ok(Session {
                            id: row.get(0)?,
                            name: row.get(1)?,
                            model: row.get(2)?,
                            message_count: row.get(3)?,
                            token_count: row.get(4)?,
                            created_at: row.get(5)?,
                            updated_at: row.get(6)?,
                        })
                    },
                );
                match result {
                    Ok(session) => Ok(Some(session)),
                    Err(rusqlite::Error::QueryReturnedNoRows) => Ok(None),
                    Err(e) => Err(StoreError::Sqlite(e)),
                }
            })
            .await
    }

    /// Delete a session and all its messages (cascade).
    #[instrument(skip(self))]
    pub async fn delete(&self, id: &str) -> StoreResult<()> {
        let id = id.to_string();
        self.db
            .execute(move |conn| {
                let deleted =
                    conn.execute("DELETE FROM sessions WHERE id = ?1", rusqlite::params![id])?;
                if deleted == 0 {
                    return Err(StoreError::NotFound {
                        entity: "session",
                        id,
                    });
                }
                Ok(())
            })
            .await
    }

    /// Append a message to a session.
    ///
    /// Also increments the session's `message_count` and updates `updated_at`.
    /// Returns the new message's row ID.
    #[instrument(skip(self, content, tool_calls, tool_call_id))]
    pub async fn append_message(
        &self,
        session_id: &str,
        role: &str,
        content: &str,
        tool_calls: Option<&str>,
        tool_call_id: Option<&str>,
    ) -> StoreResult<i64> {
        let session_id = session_id.to_string();
        let role = role.to_string();
        let content = content.to_string();
        let tool_calls = tool_calls.map(|s| s.to_string());
        let tool_call_id = tool_call_id.map(|s| s.to_string());
        let now = Utc::now().timestamp();

        self.db
            .execute(move |conn| {
                conn.execute(
                    "INSERT INTO session_messages (session_id, role, content, tool_calls, tool_call_id, created_at) \
                     VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
                    rusqlite::params![session_id, role, content, tool_calls, tool_call_id, now],
                )?;
                let msg_id = conn.last_insert_rowid();

                conn.execute(
                    "UPDATE sessions SET message_count = message_count + 1, updated_at = ?2 WHERE id = ?1",
                    rusqlite::params![session_id, now],
                )?;

                Ok(msg_id)
            })
            .await
    }

    /// Get messages for a session, ordered by creation time ascending.
    ///
    /// If `limit` is `Some(n)`, returns the most recent `n` messages.
    /// If `limit` is `None`, returns all messages.
    #[instrument(skip(self))]
    pub async fn get_messages(
        &self,
        session_id: &str,
        limit: Option<u32>,
    ) -> StoreResult<Vec<SessionMessage>> {
        let session_id = session_id.to_string();
        self.db
            .execute(move |conn| {
                let messages = match limit {
                    Some(n) => {
                        // Subquery to get the most recent N, then re-order ascending.
                        let mut stmt = conn.prepare(
                            "SELECT id, session_id, role, content, tool_calls, tool_call_id, created_at \
                             FROM (SELECT * FROM session_messages WHERE session_id = ?1 ORDER BY created_at DESC, id DESC LIMIT ?2) \
                             ORDER BY created_at ASC, id ASC",
                        )?;
                        stmt.query_map(rusqlite::params![session_id, n], |row| {
                            Ok(SessionMessage {
                                id: row.get(0)?,
                                session_id: row.get(1)?,
                                role: row.get(2)?,
                                content: row.get(3)?,
                                tool_calls: row.get(4)?,
                                tool_call_id: row.get(5)?,
                                created_at: row.get(6)?,
                            })
                        })?
                        .collect::<Result<Vec<_>, _>>()?
                    }
                    None => {
                        let mut stmt = conn.prepare(
                            "SELECT id, session_id, role, content, tool_calls, tool_call_id, created_at \
                             FROM session_messages WHERE session_id = ?1 ORDER BY created_at ASC, id ASC",
                        )?;
                        stmt.query_map(rusqlite::params![session_id], |row| {
                            Ok(SessionMessage {
                                id: row.get(0)?,
                                session_id: row.get(1)?,
                                role: row.get(2)?,
                                content: row.get(3)?,
                                tool_calls: row.get(4)?,
                                tool_call_id: row.get(5)?,
                                created_at: row.get(6)?,
                            })
                        })?
                        .collect::<Result<Vec<_>, _>>()?
                    }
                };
                Ok(messages)
            })
            .await
    }

    /// Get the message count for a session.
    #[instrument(skip(self))]
    pub async fn get_message_count(&self, session_id: &str) -> StoreResult<i64> {
        let session_id = session_id.to_string();
        self.db
            .execute(move |conn| {
                let count: i64 = conn.query_row(
                    "SELECT COUNT(*) FROM session_messages WHERE session_id = ?1",
                    rusqlite::params![session_id],
                    |row| row.get(0),
                )?;
                Ok(count)
            })
            .await
    }

    /// Update the approximate token count for a session.
    #[instrument(skip(self))]
    pub async fn update_token_count(&self, session_id: &str, tokens: i64) -> StoreResult<()> {
        let session_id = session_id.to_string();
        let now = Utc::now().timestamp();
        self.db
            .execute(move |conn| {
                let updated = conn.execute(
                    "UPDATE sessions SET token_count = ?2, updated_at = ?3 WHERE id = ?1",
                    rusqlite::params![session_id, tokens, now],
                )?;
                if updated == 0 {
                    return Err(StoreError::NotFound {
                        entity: "session",
                        id: session_id,
                    });
                }
                Ok(())
            })
            .await
    }

    /// Compact a session's message history by replacing old messages with a summary.
    ///
    /// Keeps the most recent `keep_recent` messages and deletes all older ones,
    /// then inserts the provided `summary` as a system message at the beginning
    /// of the remaining history. Also updates the session's `message_count`.
    #[instrument(skip(self, summary))]
    pub async fn compact_messages(
        &self,
        session_id: &str,
        summary: &str,
        keep_recent: usize,
    ) -> StoreResult<()> {
        let session_id = session_id.to_string();
        let summary = summary.to_string();
        let now = Utc::now().timestamp();

        self.db
            .execute(move |conn| {
                // Find the cutoff: get the ID of the message at position `keep_recent`
                // from the end. Everything before that gets deleted.
                let keep_recent_i64 = keep_recent as i64;

                // Get the minimum ID to keep (the oldest of the recent N).
                let cutoff_id: Option<i64> = conn
                    .query_row(
                        "SELECT MIN(id) FROM (\
                            SELECT id FROM session_messages WHERE session_id = ?1 \
                            ORDER BY created_at DESC, id DESC LIMIT ?2\
                        )",
                        rusqlite::params![session_id, keep_recent_i64],
                        |row| row.get(0),
                    )
                    .map_err(StoreError::from)?;

                let cutoff_id = match cutoff_id {
                    Some(id) => id,
                    None => return Ok(()), // No messages, nothing to compact.
                };

                // Delete all messages older than the cutoff.
                conn.execute(
                    "DELETE FROM session_messages WHERE session_id = ?1 AND id < ?2",
                    rusqlite::params![session_id, cutoff_id],
                )?;

                // Insert the summary as a system message with a timestamp just before
                // the earliest remaining message so it sorts first.
                let earliest_ts: i64 = conn.query_row(
                    "SELECT MIN(created_at) FROM session_messages WHERE session_id = ?1",
                    rusqlite::params![session_id],
                    |row| row.get(0),
                )?;
                let summary_ts = earliest_ts - 1;

                conn.execute(
                    "INSERT INTO session_messages (session_id, role, content, tool_calls, tool_call_id, created_at) \
                     VALUES (?1, 'system', ?2, NULL, NULL, ?3)",
                    rusqlite::params![session_id, summary, summary_ts],
                )?;

                // Update the session's message_count to reflect reality.
                let new_count: i64 = conn.query_row(
                    "SELECT COUNT(*) FROM session_messages WHERE session_id = ?1",
                    rusqlite::params![session_id],
                    |row| row.get(0),
                )?;
                conn.execute(
                    "UPDATE sessions SET message_count = ?2, updated_at = ?3 WHERE id = ?1",
                    rusqlite::params![session_id, new_count, now],
                )?;

                debug!(
                    session_id = %session_id,
                    kept = keep_recent,
                    new_count = new_count,
                    "session messages compacted"
                );
                Ok(())
            })
            .await
    }
}

// ── tests ────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    /// Create an in-memory database with session tables for testing.
    async fn setup_db() -> Database {
        let db = Database::open_in_memory()
            .map_err(|e| panic!("failed to open db: {e}"))
            .unwrap();
        db.run_migrations().await.unwrap();

        // Create session tables manually (migration v2 may not be applied
        // in the test migration set yet, so we create them explicitly).
        db.execute(|conn| {
            conn.execute_batch(
                "CREATE TABLE IF NOT EXISTS sessions (
                    id TEXT PRIMARY KEY,
                    name TEXT NOT NULL,
                    model TEXT NOT NULL DEFAULT '',
                    message_count INTEGER DEFAULT 0,
                    token_count INTEGER DEFAULT 0,
                    created_at INTEGER NOT NULL,
                    updated_at INTEGER NOT NULL
                );
                CREATE TABLE IF NOT EXISTS session_messages (
                    id INTEGER PRIMARY KEY AUTOINCREMENT,
                    session_id TEXT NOT NULL REFERENCES sessions(id) ON DELETE CASCADE,
                    role TEXT NOT NULL,
                    content TEXT NOT NULL,
                    tool_calls TEXT,
                    tool_call_id TEXT,
                    created_at INTEGER NOT NULL
                );
                CREATE INDEX IF NOT EXISTS idx_session_messages_session ON session_messages(session_id);",
            )?;
            Ok(())
        })
        .await
        .unwrap();

        db
    }

    #[tokio::test]
    async fn create_and_get_session() {
        let db = setup_db().await;
        let store = SessionStore::new(db);

        let session = store.create("test session", "gpt-4").await.unwrap();
        assert_eq!(session.name, "test session");
        assert_eq!(session.model, "gpt-4");
        assert_eq!(session.message_count, 0);
        assert_eq!(session.token_count, 0);

        let fetched = store.get(&session.id).await.unwrap();
        assert_eq!(fetched.id, session.id);
        assert_eq!(fetched.name, "test session");
        assert_eq!(fetched.model, "gpt-4");
    }

    #[tokio::test]
    async fn get_nonexistent_session_returns_not_found() {
        let db = setup_db().await;
        let store = SessionStore::new(db);

        let result = store.get("nonexistent-id").await;
        assert!(result.is_err());
        match result.unwrap_err() {
            StoreError::NotFound { entity, .. } => assert_eq!(entity, "session"),
            other => panic!("expected NotFound, got: {other}"),
        }
    }

    #[tokio::test]
    async fn list_sessions_with_pagination() {
        let db = setup_db().await;
        let store = SessionStore::new(db);

        store.create("session 1", "model-a").await.unwrap();
        store.create("session 2", "model-b").await.unwrap();
        store.create("session 3", "model-c").await.unwrap();

        let all = store.list(10, 0).await.unwrap();
        assert_eq!(all.len(), 3);

        let page = store.list(2, 0).await.unwrap();
        assert_eq!(page.len(), 2);

        let page2 = store.list(2, 2).await.unwrap();
        assert_eq!(page2.len(), 1);
    }

    #[tokio::test]
    async fn get_latest_session() {
        let db = setup_db().await;
        let store = SessionStore::new(db);

        let none = store.get_latest().await.unwrap();
        assert!(none.is_none());

        let first = store.create("first", "model").await.unwrap();
        let second = store.create("second", "model").await.unwrap();

        let latest = store.get_latest().await.unwrap();
        assert!(latest.is_some());
        let latest = latest.unwrap();
        // Both sessions may share the same updated_at (second-precision),
        // so either could be returned as "latest". Just verify it is one
        // of the sessions we created.
        assert!(
            latest.id == first.id || latest.id == second.id,
            "expected latest to be one of the created sessions"
        );
    }

    #[tokio::test]
    async fn delete_session() {
        let db = setup_db().await;
        let store = SessionStore::new(db);

        let session = store.create("to delete", "model").await.unwrap();
        store.delete(&session.id).await.unwrap();

        let result = store.get(&session.id).await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn delete_nonexistent_session_returns_not_found() {
        let db = setup_db().await;
        let store = SessionStore::new(db);

        let result = store.delete("nonexistent").await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn append_and_get_messages() {
        let db = setup_db().await;
        let store = SessionStore::new(db);

        let session = store.create("chat", "model").await.unwrap();

        let msg1_id = store
            .append_message(&session.id, "user", "Hello!", None, None)
            .await
            .unwrap();
        assert!(msg1_id > 0);

        let msg2_id = store
            .append_message(
                &session.id,
                "assistant",
                "Hi there!",
                Some(r#"[{"name":"greet"}]"#),
                None,
            )
            .await
            .unwrap();
        assert!(msg2_id > msg1_id);

        let messages = store.get_messages(&session.id, None).await.unwrap();
        assert_eq!(messages.len(), 2);
        assert_eq!(messages[0].role, "user");
        assert_eq!(messages[0].content, "Hello!");
        assert_eq!(messages[1].role, "assistant");
        assert_eq!(
            messages[1].tool_calls.as_deref(),
            Some(r#"[{"name":"greet"}]"#)
        );

        // Session message_count should be updated.
        let updated = store.get(&session.id).await.unwrap();
        assert_eq!(updated.message_count, 2);
    }

    #[tokio::test]
    async fn get_messages_with_limit() {
        let db = setup_db().await;
        let store = SessionStore::new(db);

        let session = store.create("chat", "model").await.unwrap();

        for i in 0..5 {
            store
                .append_message(&session.id, "user", &format!("msg {i}"), None, None)
                .await
                .unwrap();
        }

        let recent = store.get_messages(&session.id, Some(3)).await.unwrap();
        assert_eq!(recent.len(), 3);
        // Should be the 3 most recent messages, in ascending order.
        assert_eq!(recent[0].content, "msg 2");
        assert_eq!(recent[1].content, "msg 3");
        assert_eq!(recent[2].content, "msg 4");
    }

    #[tokio::test]
    async fn get_message_count() {
        let db = setup_db().await;
        let store = SessionStore::new(db);

        let session = store.create("chat", "model").await.unwrap();
        assert_eq!(store.get_message_count(&session.id).await.unwrap(), 0);

        store
            .append_message(&session.id, "user", "hi", None, None)
            .await
            .unwrap();
        assert_eq!(store.get_message_count(&session.id).await.unwrap(), 1);
    }

    #[tokio::test]
    async fn update_token_count() {
        let db = setup_db().await;
        let store = SessionStore::new(db);

        let session = store.create("chat", "model").await.unwrap();
        assert_eq!(session.token_count, 0);

        store.update_token_count(&session.id, 1500).await.unwrap();

        let updated = store.get(&session.id).await.unwrap();
        assert_eq!(updated.token_count, 1500);
    }

    #[tokio::test]
    async fn update_token_count_nonexistent_session() {
        let db = setup_db().await;
        let store = SessionStore::new(db);

        let result = store.update_token_count("nonexistent", 100).await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn compact_messages() {
        let db = setup_db().await;
        let store = SessionStore::new(db);

        let session = store.create("chat", "model").await.unwrap();

        // Add 10 messages.
        for i in 0..10 {
            store
                .append_message(&session.id, "user", &format!("message {i}"), None, None)
                .await
                .unwrap();
        }

        // Compact: keep 3 recent messages, summarize the rest.
        store
            .compact_messages(&session.id, "Summary of first 7 messages", 3)
            .await
            .unwrap();

        let messages = store.get_messages(&session.id, None).await.unwrap();
        // Should be: 1 summary + 3 kept = 4 messages.
        assert_eq!(messages.len(), 4);

        // First message should be the summary.
        assert_eq!(messages[0].role, "system");
        assert_eq!(messages[0].content, "Summary of first 7 messages");

        // Last 3 should be the most recent original messages.
        assert_eq!(messages[1].content, "message 7");
        assert_eq!(messages[2].content, "message 8");
        assert_eq!(messages[3].content, "message 9");

        // Session message_count should reflect the compacted state.
        let updated = store.get(&session.id).await.unwrap();
        assert_eq!(updated.message_count, 4);
    }

    #[tokio::test]
    async fn compact_empty_session_is_noop() {
        let db = setup_db().await;
        let store = SessionStore::new(db);

        let session = store.create("chat", "model").await.unwrap();

        // Compacting an empty session should not fail.
        store
            .compact_messages(&session.id, "no-op summary", 5)
            .await
            .unwrap();

        let messages = store.get_messages(&session.id, None).await.unwrap();
        assert!(messages.is_empty());
    }

    #[tokio::test]
    async fn tool_result_message_fields() {
        let db = setup_db().await;
        let store = SessionStore::new(db);

        let session = store.create("chat", "model").await.unwrap();

        store
            .append_message(
                &session.id,
                "tool_result",
                r#"{"output":"42"}"#,
                None,
                Some("call_abc123"),
            )
            .await
            .unwrap();

        let messages = store.get_messages(&session.id, None).await.unwrap();
        assert_eq!(messages.len(), 1);
        assert_eq!(messages[0].role, "tool_result");
        assert_eq!(messages[0].tool_call_id.as_deref(), Some("call_abc123"));
    }

    #[tokio::test]
    async fn delete_session_cascades_messages() {
        let db = setup_db().await;
        let store = SessionStore::new(db);

        let session = store.create("chat", "model").await.unwrap();
        store
            .append_message(&session.id, "user", "hello", None, None)
            .await
            .unwrap();
        store
            .append_message(&session.id, "assistant", "hi", None, None)
            .await
            .unwrap();

        let count_before = store.get_message_count(&session.id).await.unwrap();
        assert_eq!(count_before, 2);

        store.delete(&session.id).await.unwrap();

        // Messages should be gone too (CASCADE).
        let session_id = session.id.clone();
        let orphan_count: i64 = store
            .db
            .execute(move |conn| {
                let count: i64 = conn.query_row(
                    "SELECT COUNT(*) FROM session_messages WHERE session_id = ?1",
                    rusqlite::params![session_id],
                    |row| row.get(0),
                )?;
                Ok(count)
            })
            .await
            .unwrap();
        assert_eq!(orphan_count, 0);
    }
}
