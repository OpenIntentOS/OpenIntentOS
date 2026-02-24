//! Key-value store for persistent bot state.
//!
//! Stores simple string key-value pairs in SQLite. Used to persist
//! Telegram polling offsets, feature flags, and other bot-level state
//! that must survive restarts.

use tracing::{debug, instrument};

use crate::db::Database;
use crate::error::StoreResult;

/// Persistent key-value store for bot state.
#[derive(Clone)]
pub struct BotStateStore {
    db: Database,
}

impl BotStateStore {
    /// Create a new bot state store backed by `db`.
    pub fn new(db: Database) -> Self {
        Self { db }
    }

    /// Get a value by key, returning `None` if not found.
    #[instrument(skip(self))]
    pub async fn get(&self, key: &str) -> StoreResult<Option<String>> {
        let key = key.to_string();
        self.db
            .execute(move |conn| {
                let result = conn.query_row(
                    "SELECT value FROM bot_state WHERE key = ?1",
                    rusqlite::params![key],
                    |row| row.get(0),
                );
                match result {
                    Ok(value) => Ok(Some(value)),
                    Err(rusqlite::Error::QueryReturnedNoRows) => Ok(None),
                    Err(e) => Err(e.into()),
                }
            })
            .await
    }

    /// Set a value for a key (insert or update).
    #[instrument(skip(self, value))]
    pub async fn set(&self, key: &str, value: &str) -> StoreResult<()> {
        let key = key.to_string();
        let value = value.to_string();
        self.db
            .execute(move |conn| {
                conn.execute(
                    "INSERT INTO bot_state (key, value) VALUES (?1, ?2) \
                     ON CONFLICT(key) DO UPDATE SET value = excluded.value",
                    rusqlite::params![key, value],
                )?;
                debug!(key = %key, "bot state updated");
                Ok(())
            })
            .await
    }

    /// Delete a key, returning `true` if it existed.
    #[instrument(skip(self))]
    pub async fn delete(&self, key: &str) -> StoreResult<bool> {
        let key = key.to_string();
        self.db
            .execute(move |conn| {
                let deleted = conn.execute(
                    "DELETE FROM bot_state WHERE key = ?1",
                    rusqlite::params![key],
                )?;
                Ok(deleted > 0)
            })
            .await
    }

    /// Get a value parsed as i64, returning `None` if not found or unparseable.
    pub async fn get_i64(&self, key: &str) -> StoreResult<Option<i64>> {
        let val = self.get(key).await?;
        Ok(val.and_then(|v| v.parse().ok()))
    }

    /// Set an i64 value.
    pub async fn set_i64(&self, key: &str, value: i64) -> StoreResult<()> {
        self.set(key, &value.to_string()).await
    }
}

// ── tests ────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    async fn setup_db() -> Database {
        let db = Database::open_in_memory().unwrap();
        db.run_migrations().await.unwrap();
        db
    }

    #[tokio::test]
    async fn get_nonexistent_returns_none() {
        let db = setup_db().await;
        let store = BotStateStore::new(db);

        assert!(store.get("missing").await.unwrap().is_none());
    }

    #[tokio::test]
    async fn set_and_get() {
        let db = setup_db().await;
        let store = BotStateStore::new(db);

        store.set("key1", "value1").await.unwrap();
        assert_eq!(store.get("key1").await.unwrap(), Some("value1".to_string()));
    }

    #[tokio::test]
    async fn set_overwrites() {
        let db = setup_db().await;
        let store = BotStateStore::new(db);

        store.set("key1", "old").await.unwrap();
        store.set("key1", "new").await.unwrap();
        assert_eq!(store.get("key1").await.unwrap(), Some("new".to_string()));
    }

    #[tokio::test]
    async fn delete_existing() {
        let db = setup_db().await;
        let store = BotStateStore::new(db);

        store.set("key1", "val").await.unwrap();
        assert!(store.delete("key1").await.unwrap());
        assert!(store.get("key1").await.unwrap().is_none());
    }

    #[tokio::test]
    async fn delete_nonexistent() {
        let db = setup_db().await;
        let store = BotStateStore::new(db);

        assert!(!store.delete("missing").await.unwrap());
    }

    #[tokio::test]
    async fn get_set_i64() {
        let db = setup_db().await;
        let store = BotStateStore::new(db);

        store.set_i64("offset", 42).await.unwrap();
        assert_eq!(store.get_i64("offset").await.unwrap(), Some(42));
    }

    #[tokio::test]
    async fn get_i64_unparseable_returns_none() {
        let db = setup_db().await;
        let store = BotStateStore::new(db);

        store.set("offset", "not_a_number").await.unwrap();
        assert_eq!(store.get_i64("offset").await.unwrap(), None);
    }
}
