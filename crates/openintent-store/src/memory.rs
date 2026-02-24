//! 3-layer memory manager.
//!
//! | Layer    | Backing        | Latency     | Lifetime     |
//! |----------|----------------|-------------|--------------|
//! | Working  | `HashMap` RAM  | < 0.001 ms  | Single task  |
//! | Episodic | SQLite `episodes` table | < 5 us | 30 days |
//! | Semantic | SQLite `memories` table | < 1 ms | Permanent |
//!
//! Each layer has a clear, independent interface. Working memory is purely
//! in-process; episodic and semantic memory are backed by the [`Database`].

use std::collections::HashMap;

use chrono::Utc;
use serde::{Deserialize, Serialize};
use tracing::{debug, instrument};

use crate::db::Database;
use crate::error::{StoreError, StoreResult};

// ═══════════════════════════════════════════════════════════════════════
//  Layer 1 — Working Memory (RAM, scoped to a single task)
// ═══════════════════════════════════════════════════════════════════════

/// Fast, ephemeral key-value store that lives in RAM for a single task.
///
/// Not shared across tasks — each task gets its own `WorkingMemory`.
/// Values are arbitrary JSON (`serde_json::Value`).
#[derive(Debug, Clone, Default)]
pub struct WorkingMemory {
    store: HashMap<String, serde_json::Value>,
}

impl WorkingMemory {
    /// Create an empty working memory.
    pub fn new() -> Self {
        Self::default()
    }

    /// Insert or replace a value.
    pub fn set(&mut self, key: impl Into<String>, value: serde_json::Value) {
        let key = key.into();
        debug!(key = %key, "working_memory.set");
        self.store.insert(key, value);
    }

    /// Retrieve a value by key.
    pub fn get(&self, key: &str) -> Option<&serde_json::Value> {
        self.store.get(key)
    }

    /// Remove a key and return its former value.
    pub fn remove(&mut self, key: &str) -> Option<serde_json::Value> {
        debug!(key = %key, "working_memory.remove");
        self.store.remove(key)
    }

    /// Check whether a key exists.
    pub fn contains(&self, key: &str) -> bool {
        self.store.contains_key(key)
    }

    /// Return the number of entries.
    pub fn len(&self) -> usize {
        self.store.len()
    }

    /// Check if the store is empty.
    pub fn is_empty(&self) -> bool {
        self.store.is_empty()
    }

    /// Clear all entries (e.g. when a task completes).
    pub fn clear(&mut self) {
        debug!(entries = self.store.len(), "working_memory.clear");
        self.store.clear();
    }

    /// Iterate over all entries.
    pub fn iter(&self) -> impl Iterator<Item = (&String, &serde_json::Value)> {
        self.store.iter()
    }
}

// ═══════════════════════════════════════════════════════════════════════
//  Layer 2 — Episodic Memory (SQLite `episodes` table)
// ═══════════════════════════════════════════════════════════════════════

/// The type/role of an episode within a ReAct loop.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum EpisodeKind {
    Observation,
    Action,
    Result,
    Reflection,
}

impl EpisodeKind {
    fn as_str(self) -> &'static str {
        match self {
            Self::Observation => "observation",
            Self::Action => "action",
            Self::Result => "result",
            Self::Reflection => "reflection",
        }
    }

    fn from_str(s: &str) -> StoreResult<Self> {
        match s {
            "observation" => Ok(Self::Observation),
            "action" => Ok(Self::Action),
            "result" => Ok(Self::Result),
            "reflection" => Ok(Self::Reflection),
            other => Err(StoreError::InvalidArgument(format!(
                "unknown episode kind: {other}"
            ))),
        }
    }
}

/// A single episode record.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Episode {
    pub id: i64,
    pub task_id: String,
    pub kind: EpisodeKind,
    pub content: serde_json::Value,
    pub timestamp: i64,
}

/// CRUD operations on the `episodes` table.
#[derive(Clone)]
pub struct EpisodicMemory {
    db: Database,
}

impl EpisodicMemory {
    /// Create a new episodic memory backed by `db`.
    pub fn new(db: Database) -> Self {
        Self { db }
    }

    /// Insert a new episode.
    #[instrument(skip(self, content), fields(task_id = %task_id, kind = ?kind))]
    pub async fn insert(
        &self,
        task_id: &str,
        kind: EpisodeKind,
        content: serde_json::Value,
    ) -> StoreResult<i64> {
        let task_id = task_id.to_string();
        let kind_str = kind.as_str().to_string();
        let content_str = serde_json::to_string(&content)?;
        let now = Utc::now().timestamp();

        self.db
            .execute(move |conn| {
                conn.execute(
                    "INSERT INTO episodes (task_id, type, content, timestamp) VALUES (?1, ?2, ?3, ?4)",
                    rusqlite::params![task_id, kind_str, content_str, now],
                )?;
                Ok(conn.last_insert_rowid())
            })
            .await
    }

    /// Fetch a single episode by ID.
    #[instrument(skip(self))]
    pub async fn get(&self, id: i64) -> StoreResult<Episode> {
        self.db
            .execute(move |conn| {
                conn.query_row(
                    "SELECT id, task_id, type, content, timestamp FROM episodes WHERE id = ?1",
                    [id],
                    |row| {
                        Ok((
                            row.get::<_, i64>(0)?,
                            row.get::<_, String>(1)?,
                            row.get::<_, String>(2)?,
                            row.get::<_, String>(3)?,
                            row.get::<_, i64>(4)?,
                        ))
                    },
                )
                .map_err(StoreError::from)
                .and_then(|(id, task_id, kind_str, content_str, ts)| {
                    Ok(Episode {
                        id,
                        task_id,
                        kind: EpisodeKind::from_str(&kind_str)?,
                        content: serde_json::from_str(&content_str)?,
                        timestamp: ts,
                    })
                })
            })
            .await
    }

    /// List all episodes for a given task, ordered by timestamp ascending.
    #[instrument(skip(self))]
    pub async fn list_by_task(&self, task_id: &str) -> StoreResult<Vec<Episode>> {
        let task_id = task_id.to_string();
        self.db
            .execute(move |conn| {
                let mut stmt = conn.prepare(
                    "SELECT id, task_id, type, content, timestamp \
                     FROM episodes WHERE task_id = ?1 ORDER BY timestamp ASC",
                )?;
                let rows = stmt
                    .query_map([&task_id], |row| {
                        Ok((
                            row.get::<_, i64>(0)?,
                            row.get::<_, String>(1)?,
                            row.get::<_, String>(2)?,
                            row.get::<_, String>(3)?,
                            row.get::<_, i64>(4)?,
                        ))
                    })?
                    .collect::<Result<Vec<_>, _>>()?;

                let mut episodes = Vec::with_capacity(rows.len());
                for (id, task_id, kind_str, content_str, ts) in rows {
                    episodes.push(Episode {
                        id,
                        task_id,
                        kind: EpisodeKind::from_str(&kind_str)?,
                        content: serde_json::from_str(&content_str)?,
                        timestamp: ts,
                    });
                }
                Ok(episodes)
            })
            .await
    }

    /// Delete all episodes for a task (e.g. on task cleanup).
    #[instrument(skip(self))]
    pub async fn delete_by_task(&self, task_id: &str) -> StoreResult<usize> {
        let task_id = task_id.to_string();
        self.db
            .execute(move |conn| {
                let deleted =
                    conn.execute("DELETE FROM episodes WHERE task_id = ?1", [&task_id])?;
                Ok(deleted)
            })
            .await
    }

    /// Delete episodes older than `before_timestamp` (epoch seconds).
    ///
    /// Useful for the 30-day retention policy.
    #[instrument(skip(self))]
    pub async fn delete_before(&self, before_timestamp: i64) -> StoreResult<usize> {
        self.db
            .execute(move |conn| {
                let deleted = conn.execute(
                    "DELETE FROM episodes WHERE timestamp < ?1",
                    [before_timestamp],
                )?;
                Ok(deleted)
            })
            .await
    }
}

// ═══════════════════════════════════════════════════════════════════════
//  Layer 3 — Semantic Memory (SQLite `memories` table + vector storage)
// ═══════════════════════════════════════════════════════════════════════

/// The category of a semantic memory entry.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum MemoryCategory {
    Preference,
    Knowledge,
    Pattern,
    Skill,
}

impl MemoryCategory {
    fn as_str(self) -> &'static str {
        match self {
            Self::Preference => "preference",
            Self::Knowledge => "knowledge",
            Self::Pattern => "pattern",
            Self::Skill => "skill",
        }
    }

    fn from_str(s: &str) -> StoreResult<Self> {
        match s {
            "preference" => Ok(Self::Preference),
            "knowledge" => Ok(Self::Knowledge),
            "pattern" => Ok(Self::Pattern),
            "skill" => Ok(Self::Skill),
            other => Err(StoreError::InvalidArgument(format!(
                "unknown memory category: {other}"
            ))),
        }
    }
}

/// A single semantic memory record.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Memory {
    pub id: i64,
    pub category: MemoryCategory,
    pub content: String,
    /// Optional embedding vector (f32 values serialized as a byte blob).
    #[serde(skip)]
    pub embedding: Option<Vec<f32>>,
    pub importance: f64,
    pub access_count: i64,
    pub created_at: i64,
    pub updated_at: i64,
}

/// Input for creating a new memory.
pub struct NewMemory {
    pub category: MemoryCategory,
    pub content: String,
    pub embedding: Option<Vec<f32>>,
    pub importance: f64,
}

/// CRUD operations on the `memories` table with vector storage support.
#[derive(Clone)]
pub struct SemanticMemory {
    db: Database,
}

impl SemanticMemory {
    /// Create a new semantic memory layer backed by `db`.
    pub fn new(db: Database) -> Self {
        Self { db }
    }

    /// Insert a new memory record.
    #[instrument(skip(self, input), fields(category = ?input.category))]
    pub async fn insert(&self, input: NewMemory) -> StoreResult<i64> {
        let now = Utc::now().timestamp();
        let category = input.category.as_str().to_string();
        let embedding_blob = input.embedding.map(embedding_to_blob);

        self.db
            .execute(move |conn| {
                conn.execute(
                    "INSERT INTO memories (category, content, embedding, importance, access_count, created_at, updated_at) \
                     VALUES (?1, ?2, ?3, ?4, 0, ?5, ?5)",
                    rusqlite::params![
                        category,
                        input.content,
                        embedding_blob,
                        input.importance,
                        now,
                    ],
                )?;
                Ok(conn.last_insert_rowid())
            })
            .await
    }

    /// Fetch a single memory by ID, incrementing its access count.
    #[instrument(skip(self))]
    pub async fn get(&self, id: i64) -> StoreResult<Memory> {
        let now = Utc::now().timestamp();
        self.db
            .execute(move |conn| {
                // Bump access count.
                conn.execute(
                    "UPDATE memories SET access_count = access_count + 1, updated_at = ?2 WHERE id = ?1",
                    rusqlite::params![id, now],
                )?;

                conn.query_row(
                    "SELECT id, category, content, embedding, importance, access_count, created_at, updated_at \
                     FROM memories WHERE id = ?1",
                    [id],
                    |row| {
                        Ok((
                            row.get::<_, i64>(0)?,
                            row.get::<_, String>(1)?,
                            row.get::<_, String>(2)?,
                            row.get::<_, Option<Vec<u8>>>(3)?,
                            row.get::<_, f64>(4)?,
                            row.get::<_, i64>(5)?,
                            row.get::<_, i64>(6)?,
                            row.get::<_, i64>(7)?,
                        ))
                    },
                )
                .map_err(StoreError::from)
                .and_then(|(id, cat, content, emb, imp, ac, ca, ua)| {
                    Ok(Memory {
                        id,
                        category: MemoryCategory::from_str(&cat)?,
                        content,
                        embedding: emb.map(blob_to_embedding),
                        importance: imp,
                        access_count: ac,
                        created_at: ca,
                        updated_at: ua,
                    })
                })
            })
            .await
    }

    /// List memories by category, ordered by importance descending.
    #[instrument(skip(self))]
    pub async fn list_by_category(
        &self,
        category: MemoryCategory,
        limit: u32,
    ) -> StoreResult<Vec<Memory>> {
        let cat = category.as_str().to_string();
        self.db
            .execute(move |conn| {
                let mut stmt = conn.prepare(
                    "SELECT id, category, content, embedding, importance, access_count, created_at, updated_at \
                     FROM memories WHERE category = ?1 ORDER BY importance DESC LIMIT ?2",
                )?;
                let rows = stmt
                    .query_map(rusqlite::params![cat, limit], |row| {
                        Ok((
                            row.get::<_, i64>(0)?,
                            row.get::<_, String>(1)?,
                            row.get::<_, String>(2)?,
                            row.get::<_, Option<Vec<u8>>>(3)?,
                            row.get::<_, f64>(4)?,
                            row.get::<_, i64>(5)?,
                            row.get::<_, i64>(6)?,
                            row.get::<_, i64>(7)?,
                        ))
                    })?
                    .collect::<Result<Vec<_>, _>>()?;

                let mut memories = Vec::with_capacity(rows.len());
                for (id, cat, content, emb, imp, ac, ca, ua) in rows {
                    memories.push(Memory {
                        id,
                        category: MemoryCategory::from_str(&cat)?,
                        content,
                        embedding: emb.map(blob_to_embedding),
                        importance: imp,
                        access_count: ac,
                        created_at: ca,
                        updated_at: ua,
                    });
                }
                Ok(memories)
            })
            .await
    }

    /// Update the embedding vector for a memory.
    #[instrument(skip(self, embedding))]
    pub async fn update_embedding(&self, id: i64, embedding: Vec<f32>) -> StoreResult<()> {
        let now = Utc::now().timestamp();
        let blob = embedding_to_blob(embedding);
        self.db
            .execute(move |conn| {
                let updated = conn.execute(
                    "UPDATE memories SET embedding = ?2, updated_at = ?3 WHERE id = ?1",
                    rusqlite::params![id, blob, now],
                )?;
                if updated == 0 {
                    return Err(StoreError::NotFound {
                        entity: "memory",
                        id: id.to_string(),
                    });
                }
                Ok(())
            })
            .await
    }

    /// Update the importance score of a memory.
    #[instrument(skip(self))]
    pub async fn update_importance(&self, id: i64, importance: f64) -> StoreResult<()> {
        let now = Utc::now().timestamp();
        self.db
            .execute(move |conn| {
                let updated = conn.execute(
                    "UPDATE memories SET importance = ?2, updated_at = ?3 WHERE id = ?1",
                    rusqlite::params![id, importance, now],
                )?;
                if updated == 0 {
                    return Err(StoreError::NotFound {
                        entity: "memory",
                        id: id.to_string(),
                    });
                }
                Ok(())
            })
            .await
    }

    /// Delete a memory by ID.
    #[instrument(skip(self))]
    pub async fn delete(&self, id: i64) -> StoreResult<()> {
        self.db
            .execute(move |conn| {
                let deleted = conn.execute("DELETE FROM memories WHERE id = ?1", [id])?;
                if deleted == 0 {
                    return Err(StoreError::NotFound {
                        entity: "memory",
                        id: id.to_string(),
                    });
                }
                Ok(())
            })
            .await
    }

    /// Search memories by keyword (case-insensitive LIKE match on content).
    ///
    /// Optionally filter by category. Results are ordered by importance
    /// descending and limited to `limit` rows.
    #[instrument(skip(self))]
    pub async fn search_by_keyword(
        &self,
        query: &str,
        category: Option<MemoryCategory>,
        limit: u32,
    ) -> StoreResult<Vec<Memory>> {
        let pattern = format!("%{query}%");
        let cat = category.map(|c| c.as_str().to_string());
        self.db
            .execute(move |conn| {
                let (sql, params_vec): (String, Vec<Box<dyn rusqlite::types::ToSql + Send>>) =
                    match &cat {
                        Some(cat_str) => (
                            "SELECT id, category, content, embedding, importance, access_count, \
                             created_at, updated_at FROM memories \
                             WHERE content LIKE ?1 AND category = ?2 \
                             ORDER BY importance DESC LIMIT ?3"
                                .to_string(),
                            vec![
                                Box::new(pattern) as Box<dyn rusqlite::types::ToSql + Send>,
                                Box::new(cat_str.clone()),
                                Box::new(limit),
                            ],
                        ),
                        None => (
                            "SELECT id, category, content, embedding, importance, access_count, \
                             created_at, updated_at FROM memories \
                             WHERE content LIKE ?1 \
                             ORDER BY importance DESC LIMIT ?2"
                                .to_string(),
                            vec![
                                Box::new(pattern) as Box<dyn rusqlite::types::ToSql + Send>,
                                Box::new(limit),
                            ],
                        ),
                    };

                let params_refs: Vec<&dyn rusqlite::types::ToSql> = params_vec
                    .iter()
                    .map(|p| p.as_ref() as &dyn rusqlite::types::ToSql)
                    .collect();

                let mut stmt = conn.prepare(&sql)?;
                let rows = stmt
                    .query_map(params_refs.as_slice(), |row| {
                        Ok((
                            row.get::<_, i64>(0)?,
                            row.get::<_, String>(1)?,
                            row.get::<_, String>(2)?,
                            row.get::<_, Option<Vec<u8>>>(3)?,
                            row.get::<_, f64>(4)?,
                            row.get::<_, i64>(5)?,
                            row.get::<_, i64>(6)?,
                            row.get::<_, i64>(7)?,
                        ))
                    })?
                    .collect::<Result<Vec<_>, _>>()?;

                let mut memories = Vec::with_capacity(rows.len());
                for (id, cat, content, emb, imp, ac, ca, ua) in rows {
                    memories.push(Memory {
                        id,
                        category: MemoryCategory::from_str(&cat)?,
                        content,
                        embedding: emb.map(blob_to_embedding),
                        importance: imp,
                        access_count: ac,
                        created_at: ca,
                        updated_at: ua,
                    });
                }
                Ok(memories)
            })
            .await
    }

    /// List all memories, optionally filtered by category, ordered by
    /// importance descending.
    #[instrument(skip(self))]
    pub async fn list_all(
        &self,
        category: Option<MemoryCategory>,
        limit: u32,
    ) -> StoreResult<Vec<Memory>> {
        let cat = category.map(|c| c.as_str().to_string());
        self.db
            .execute(move |conn| {
                let (sql, params_vec): (String, Vec<Box<dyn rusqlite::types::ToSql + Send>>) =
                    match &cat {
                        Some(cat_str) => (
                            "SELECT id, category, content, embedding, importance, access_count, \
                             created_at, updated_at FROM memories \
                             WHERE category = ?1 \
                             ORDER BY importance DESC LIMIT ?2"
                                .to_string(),
                            vec![
                                Box::new(cat_str.clone()) as Box<dyn rusqlite::types::ToSql + Send>,
                                Box::new(limit),
                            ],
                        ),
                        None => (
                            "SELECT id, category, content, embedding, importance, access_count, \
                             created_at, updated_at FROM memories \
                             ORDER BY importance DESC LIMIT ?1"
                                .to_string(),
                            vec![Box::new(limit) as Box<dyn rusqlite::types::ToSql + Send>],
                        ),
                    };

                let params_refs: Vec<&dyn rusqlite::types::ToSql> = params_vec
                    .iter()
                    .map(|p| p.as_ref() as &dyn rusqlite::types::ToSql)
                    .collect();

                let mut stmt = conn.prepare(&sql)?;
                let rows = stmt
                    .query_map(params_refs.as_slice(), |row| {
                        Ok((
                            row.get::<_, i64>(0)?,
                            row.get::<_, String>(1)?,
                            row.get::<_, String>(2)?,
                            row.get::<_, Option<Vec<u8>>>(3)?,
                            row.get::<_, f64>(4)?,
                            row.get::<_, i64>(5)?,
                            row.get::<_, i64>(6)?,
                            row.get::<_, i64>(7)?,
                        ))
                    })?
                    .collect::<Result<Vec<_>, _>>()?;

                let mut memories = Vec::with_capacity(rows.len());
                for (id, cat, content, emb, imp, ac, ca, ua) in rows {
                    memories.push(Memory {
                        id,
                        category: MemoryCategory::from_str(&cat)?,
                        content,
                        embedding: emb.map(blob_to_embedding),
                        importance: imp,
                        access_count: ac,
                        created_at: ca,
                        updated_at: ua,
                    });
                }
                Ok(memories)
            })
            .await
    }

    /// Count all memories, optionally filtered by category.
    pub async fn count(&self, category: Option<MemoryCategory>) -> StoreResult<i64> {
        self.db
            .execute(move |conn| {
                let count: i64 = match category {
                    Some(cat) => conn.query_row(
                        "SELECT count(*) FROM memories WHERE category = ?1",
                        [cat.as_str()],
                        |row| row.get(0),
                    )?,
                    None => {
                        conn.query_row("SELECT count(*) FROM memories", [], |row| row.get(0))?
                    }
                };
                Ok(count)
            })
            .await
    }
}

// ── vector helpers ───────────────────────────────────────────────────

/// Serialize a `Vec<f32>` into a byte blob (little-endian) for SQLite BLOB storage.
fn embedding_to_blob(embedding: Vec<f32>) -> Vec<u8> {
    let mut bytes = Vec::with_capacity(embedding.len() * 4);
    for val in &embedding {
        bytes.extend_from_slice(&val.to_le_bytes());
    }
    bytes
}

/// Deserialize a byte blob back into a `Vec<f32>`.
fn blob_to_embedding(blob: Vec<u8>) -> Vec<f32> {
    blob.chunks_exact(4)
        .map(|chunk| {
            let arr: [u8; 4] = chunk.try_into().expect("chunk is exactly 4 bytes");
            f32::from_le_bytes(arr)
        })
        .collect()
}

// ── tests ────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    // ── Working Memory ───────────────────────────────────────────────

    #[test]
    fn working_memory_basic_ops() {
        let mut wm = WorkingMemory::new();
        assert!(wm.is_empty());

        wm.set("key1", serde_json::json!("hello"));
        assert_eq!(wm.len(), 1);
        assert!(wm.contains("key1"));
        assert_eq!(wm.get("key1"), Some(&serde_json::json!("hello")));

        let removed = wm.remove("key1");
        assert_eq!(removed, Some(serde_json::json!("hello")));
        assert!(wm.is_empty());
    }

    #[test]
    fn working_memory_clear() {
        let mut wm = WorkingMemory::new();
        wm.set("a", serde_json::json!(1));
        wm.set("b", serde_json::json!(2));
        assert_eq!(wm.len(), 2);

        wm.clear();
        assert!(wm.is_empty());
    }

    // ── Vector helpers ───────────────────────────────────────────────

    #[test]
    fn embedding_roundtrip() {
        let original = vec![1.0_f32, -0.5, 0.0, 3.14, f32::MAX, f32::MIN];
        let blob = embedding_to_blob(original.clone());
        let restored = blob_to_embedding(blob);
        assert_eq!(original, restored);
    }

    // ── Episodic Memory ──────────────────────────────────────────────

    async fn setup_db() -> Database {
        let db = Database::open_in_memory().unwrap();
        db.run_migrations().await.unwrap();
        db
    }

    #[tokio::test]
    async fn episodic_insert_and_get() {
        let db = setup_db().await;
        let em = EpisodicMemory::new(db.clone());

        // We need a task row because of the FK constraint.
        db.execute(|conn| {
            conn.execute(
                "INSERT INTO tasks (id, status, created_at) VALUES ('t1', 'running', 0)",
                [],
            )?;
            Ok(())
        })
        .await
        .unwrap();

        let id = em
            .insert(
                "t1",
                EpisodeKind::Observation,
                serde_json::json!({"text": "saw a cat"}),
            )
            .await
            .unwrap();

        let episode = em.get(id).await.unwrap();
        assert_eq!(episode.task_id, "t1");
        assert_eq!(episode.kind, EpisodeKind::Observation);
        assert_eq!(episode.content["text"], "saw a cat");
    }

    #[tokio::test]
    async fn episodic_list_and_delete() {
        let db = setup_db().await;
        let em = EpisodicMemory::new(db.clone());

        db.execute(|conn| {
            conn.execute(
                "INSERT INTO tasks (id, status, created_at) VALUES ('t2', 'running', 0)",
                [],
            )?;
            Ok(())
        })
        .await
        .unwrap();

        em.insert("t2", EpisodeKind::Action, serde_json::json!("a1"))
            .await
            .unwrap();
        em.insert("t2", EpisodeKind::Result, serde_json::json!("r1"))
            .await
            .unwrap();

        let episodes = em.list_by_task("t2").await.unwrap();
        assert_eq!(episodes.len(), 2);

        let deleted = em.delete_by_task("t2").await.unwrap();
        assert_eq!(deleted, 2);

        let episodes = em.list_by_task("t2").await.unwrap();
        assert!(episodes.is_empty());
    }

    // ── Semantic Memory ──────────────────────────────────────────────

    #[tokio::test]
    async fn semantic_insert_and_get() {
        let db = setup_db().await;
        let sm = SemanticMemory::new(db);

        let id = sm
            .insert(NewMemory {
                category: MemoryCategory::Knowledge,
                content: "Rust is a systems programming language".to_string(),
                embedding: Some(vec![0.1, 0.2, 0.3]),
                importance: 0.8,
            })
            .await
            .unwrap();

        let mem = sm.get(id).await.unwrap();
        assert_eq!(mem.category, MemoryCategory::Knowledge);
        assert_eq!(mem.content, "Rust is a systems programming language");
        assert_eq!(mem.embedding.as_ref().unwrap().len(), 3);
        assert_eq!(mem.access_count, 1); // get() bumps access_count
    }

    #[tokio::test]
    async fn semantic_list_by_category() {
        let db = setup_db().await;
        let sm = SemanticMemory::new(db);

        sm.insert(NewMemory {
            category: MemoryCategory::Preference,
            content: "Likes dark mode".to_string(),
            embedding: None,
            importance: 0.9,
        })
        .await
        .unwrap();

        sm.insert(NewMemory {
            category: MemoryCategory::Preference,
            content: "Prefers verbose output".to_string(),
            embedding: None,
            importance: 0.5,
        })
        .await
        .unwrap();

        sm.insert(NewMemory {
            category: MemoryCategory::Knowledge,
            content: "Unrelated".to_string(),
            embedding: None,
            importance: 1.0,
        })
        .await
        .unwrap();

        let prefs = sm
            .list_by_category(MemoryCategory::Preference, 10)
            .await
            .unwrap();
        assert_eq!(prefs.len(), 2);
        // Should be ordered by importance DESC.
        assert!(prefs[0].importance >= prefs[1].importance);
    }

    #[tokio::test]
    async fn semantic_delete() {
        let db = setup_db().await;
        let sm = SemanticMemory::new(db);

        let id = sm
            .insert(NewMemory {
                category: MemoryCategory::Skill,
                content: "test".to_string(),
                embedding: None,
                importance: 0.5,
            })
            .await
            .unwrap();

        sm.delete(id).await.unwrap();

        let result = sm.get(id).await;
        assert!(result.is_err());
    }
}
