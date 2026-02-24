//! Integration tests for the openintent-store crate.
//!
//! These tests exercise the full database lifecycle including migrations,
//! session CRUD, and the 3-layer memory system against a real SQLite
//! database on disk (via tempfile).

use openintent_store::{
    Database, EpisodeKind, EpisodicMemory, MemoryCategory, NewMemory, SemanticMemory, SessionStore,
    WorkingMemory,
};

// ═══════════════════════════════════════════════════════════════════════
//  Database lifecycle
// ═══════════════════════════════════════════════════════════════════════

#[tokio::test]
async fn database_open_and_migrate_on_disk() {
    let dir = tempfile::tempdir().unwrap();
    let db_path = dir.path().join("test.db");

    let db = Database::open_and_migrate(db_path.clone()).await.unwrap();

    // Verify core tables exist by querying them.
    let workflow_count: i64 = db
        .execute(|conn| {
            let c: i64 = conn.query_row("SELECT count(*) FROM workflows", [], |row| row.get(0))?;
            Ok(c)
        })
        .await
        .unwrap();
    assert_eq!(workflow_count, 0);

    let episode_count: i64 = db
        .execute(|conn| {
            let c: i64 = conn.query_row("SELECT count(*) FROM episodes", [], |row| row.get(0))?;
            Ok(c)
        })
        .await
        .unwrap();
    assert_eq!(episode_count, 0);

    let memory_count: i64 = db
        .execute(|conn| {
            let c: i64 = conn.query_row("SELECT count(*) FROM memories", [], |row| row.get(0))?;
            Ok(c)
        })
        .await
        .unwrap();
    assert_eq!(memory_count, 0);

    // Verify the database file was created.
    assert!(db_path.exists());
}

#[tokio::test]
async fn database_open_and_migrate_idempotent() {
    let dir = tempfile::tempdir().unwrap();
    let db_path = dir.path().join("test_idempotent.db");

    // Open and migrate twice -- should not fail.
    let db1 = Database::open_and_migrate(db_path.clone()).await.unwrap();
    drop(db1);

    let db2 = Database::open_and_migrate(db_path).await.unwrap();
    let count: i64 = db2
        .execute(|conn| {
            let c: i64 = conn.query_row("SELECT count(*) FROM workflows", [], |row| row.get(0))?;
            Ok(c)
        })
        .await
        .unwrap();
    assert_eq!(count, 0);
}

// ═══════════════════════════════════════════════════════════════════════
//  Session full lifecycle (on-disk database)
// ═══════════════════════════════════════════════════════════════════════

#[tokio::test]
async fn session_full_lifecycle() {
    let dir = tempfile::tempdir().unwrap();
    let db = Database::open_and_migrate(dir.path().join("test.db"))
        .await
        .unwrap();
    let store = SessionStore::new(db);

    // Create session.
    let session = store.create("test-session", "claude-sonnet").await.unwrap();
    assert_eq!(session.name, "test-session");
    assert_eq!(session.model, "claude-sonnet");
    assert_eq!(session.message_count, 0);

    // Append messages.
    let msg1_id = store
        .append_message(&session.id, "user", "hello", None, None)
        .await
        .unwrap();
    assert!(msg1_id > 0);

    let msg2_id = store
        .append_message(&session.id, "assistant", "hi there", None, None)
        .await
        .unwrap();
    assert!(msg2_id > msg1_id);

    // Get messages.
    let messages = store.get_messages(&session.id, Some(10)).await.unwrap();
    assert_eq!(messages.len(), 2);
    assert_eq!(messages[0].role, "user");
    assert_eq!(messages[0].content, "hello");
    assert_eq!(messages[1].role, "assistant");
    assert_eq!(messages[1].content, "hi there");

    // List sessions.
    let sessions = store.list(10, 0).await.unwrap();
    assert_eq!(sessions.len(), 1);
    assert_eq!(sessions[0].name, "test-session");

    // Verify message count was updated.
    let updated = store.get(&session.id).await.unwrap();
    assert_eq!(updated.message_count, 2);

    // Delete.
    store.delete(&session.id).await.unwrap();
    let sessions = store.list(10, 0).await.unwrap();
    assert!(sessions.is_empty());
}

#[tokio::test]
async fn session_compact_messages_on_disk() {
    let dir = tempfile::tempdir().unwrap();
    let db = Database::open_and_migrate(dir.path().join("test.db"))
        .await
        .unwrap();
    let store = SessionStore::new(db);

    let session = store.create("compact-test", "model").await.unwrap();

    // Add 10 messages.
    for i in 0..10 {
        store
            .append_message(&session.id, "user", &format!("msg {i}"), None, None)
            .await
            .unwrap();
    }

    // Compact: keep 3 recent messages, summarize the rest.
    store
        .compact_messages(&session.id, "Summary of older messages", 3)
        .await
        .unwrap();

    let messages = store.get_messages(&session.id, None).await.unwrap();
    // 1 summary + 3 kept = 4
    assert_eq!(messages.len(), 4);
    assert_eq!(messages[0].role, "system");
    assert_eq!(messages[0].content, "Summary of older messages");
    assert_eq!(messages[1].content, "msg 7");
    assert_eq!(messages[2].content, "msg 8");
    assert_eq!(messages[3].content, "msg 9");
}

// ═══════════════════════════════════════════════════════════════════════
//  Semantic memory full lifecycle (on-disk database)
// ═══════════════════════════════════════════════════════════════════════

#[tokio::test]
async fn memory_full_lifecycle() {
    let dir = tempfile::tempdir().unwrap();
    let db = Database::open_and_migrate(dir.path().join("test.db"))
        .await
        .unwrap();
    let memory = SemanticMemory::new(db);

    // Store a memory.
    let id = memory
        .insert(NewMemory {
            category: MemoryCategory::Knowledge,
            content: "Rust is a systems programming language".to_string(),
            embedding: None,
            importance: 0.8,
        })
        .await
        .unwrap();
    assert!(id > 0);

    // Search by keyword.
    let results = memory.search_by_keyword("Rust", None, 5).await.unwrap();
    assert_eq!(results.len(), 1);
    assert!(results[0].content.contains("Rust"));
    assert_eq!(results[0].category, MemoryCategory::Knowledge);

    // List all.
    let all = memory.list_all(None, 10).await.unwrap();
    assert_eq!(all.len(), 1);

    // List by category.
    let knowledge = memory
        .list_by_category(MemoryCategory::Knowledge, 10)
        .await
        .unwrap();
    assert_eq!(knowledge.len(), 1);

    let prefs = memory
        .list_by_category(MemoryCategory::Preference, 10)
        .await
        .unwrap();
    assert!(prefs.is_empty());

    // Get by ID (also increments access_count).
    let fetched = memory.get(id).await.unwrap();
    assert_eq!(fetched.content, "Rust is a systems programming language");
    assert_eq!(fetched.access_count, 1);

    // Update importance.
    memory.update_importance(id, 0.95).await.unwrap();
    let updated = memory.get(id).await.unwrap();
    assert!((updated.importance - 0.95).abs() < f64::EPSILON);

    // Delete.
    memory.delete(id).await.unwrap();
    let all = memory.list_all(None, 10).await.unwrap();
    assert!(all.is_empty());
}

#[tokio::test]
async fn memory_search_with_category_filter() {
    let dir = tempfile::tempdir().unwrap();
    let db = Database::open_and_migrate(dir.path().join("test.db"))
        .await
        .unwrap();
    let memory = SemanticMemory::new(db);

    memory
        .insert(NewMemory {
            category: MemoryCategory::Knowledge,
            content: "Rust has zero-cost abstractions".to_string(),
            embedding: None,
            importance: 0.7,
        })
        .await
        .unwrap();

    memory
        .insert(NewMemory {
            category: MemoryCategory::Preference,
            content: "User prefers Rust over C++".to_string(),
            embedding: None,
            importance: 0.6,
        })
        .await
        .unwrap();

    // Search without filter -- finds both.
    let all = memory.search_by_keyword("Rust", None, 10).await.unwrap();
    assert_eq!(all.len(), 2);

    // Search with category filter -- finds only knowledge.
    let knowledge_only = memory
        .search_by_keyword("Rust", Some(MemoryCategory::Knowledge), 10)
        .await
        .unwrap();
    assert_eq!(knowledge_only.len(), 1);
    assert_eq!(knowledge_only[0].category, MemoryCategory::Knowledge);
}

// ═══════════════════════════════════════════════════════════════════════
//  Episodic memory lifecycle (on-disk database)
// ═══════════════════════════════════════════════════════════════════════

#[tokio::test]
async fn episodic_memory_lifecycle() {
    let dir = tempfile::tempdir().unwrap();
    let db = Database::open_and_migrate(dir.path().join("test.db"))
        .await
        .unwrap();

    // The episodes table has a foreign key to tasks(id), so we need to
    // create parent task records first.
    db.execute(|conn| {
        let now = chrono::Utc::now().timestamp();
        conn.execute(
            "INSERT INTO tasks (id, status, created_at) VALUES ('task-001', 'pending', ?1)",
            [now],
        )?;
        conn.execute(
            "INSERT INTO tasks (id, status, created_at) VALUES ('task-002', 'pending', ?1)",
            [now],
        )?;
        Ok(())
    })
    .await
    .unwrap();

    let episodic = EpisodicMemory::new(db);

    // Insert episodes for a task.
    let id1 = episodic
        .insert(
            "task-001",
            EpisodeKind::Observation,
            serde_json::json!({"input": "user asked about weather"}),
        )
        .await
        .unwrap();
    assert!(id1 > 0);

    let id2 = episodic
        .insert(
            "task-001",
            EpisodeKind::Action,
            serde_json::json!({"tool": "web_search", "query": "weather today"}),
        )
        .await
        .unwrap();
    assert!(id2 > id1);

    let _id3 = episodic
        .insert(
            "task-001",
            EpisodeKind::Result,
            serde_json::json!({"result": "sunny, 72F"}),
        )
        .await
        .unwrap();

    // Get by ID.
    let ep = episodic.get(id1).await.unwrap();
    assert_eq!(ep.task_id, "task-001");
    assert_eq!(ep.kind, EpisodeKind::Observation);

    // List by task.
    let episodes = episodic.list_by_task("task-001").await.unwrap();
    assert_eq!(episodes.len(), 3);
    assert_eq!(episodes[0].kind, EpisodeKind::Observation);
    assert_eq!(episodes[1].kind, EpisodeKind::Action);
    assert_eq!(episodes[2].kind, EpisodeKind::Result);

    // Delete by task.
    let deleted = episodic.delete_by_task("task-001").await.unwrap();
    assert_eq!(deleted, 3);

    let remaining = episodic.list_by_task("task-001").await.unwrap();
    assert!(remaining.is_empty());

    // Insert episode for the second task and delete by timestamp.
    episodic
        .insert(
            "task-002",
            EpisodeKind::Reflection,
            serde_json::json!({"insight": "user likes concise answers"}),
        )
        .await
        .unwrap();

    // Delete episodes older than far in the future -- should delete everything.
    let far_future = chrono::Utc::now().timestamp() + 86400;
    let deleted = episodic.delete_before(far_future).await.unwrap();
    assert_eq!(deleted, 1);
}

// ═══════════════════════════════════════════════════════════════════════
//  Working memory (in-process, no database needed)
// ═══════════════════════════════════════════════════════════════════════

#[test]
fn working_memory_crud() {
    let mut wm = WorkingMemory::new();
    assert!(wm.is_empty());

    wm.set("key1", serde_json::json!("value1"));
    wm.set("key2", serde_json::json!(42));

    assert_eq!(wm.len(), 2);
    assert!(wm.contains("key1"));
    assert_eq!(wm.get("key1").unwrap(), &serde_json::json!("value1"));

    let removed = wm.remove("key1");
    assert!(removed.is_some());
    assert_eq!(wm.len(), 1);
    assert!(!wm.contains("key1"));

    wm.clear();
    assert!(wm.is_empty());
}

// ═══════════════════════════════════════════════════════════════════════
//  Cache layer
// ═══════════════════════════════════════════════════════════════════════

#[tokio::test]
async fn cache_layer_basic_operations() {
    use openintent_store::CacheLayer;

    let cache: CacheLayer<String> = CacheLayer::builder("test-cache")
        .max_capacity(100)
        .ttl_seconds(60)
        .build();

    // Insert and retrieve.
    cache.insert("key1", &"value1".to_string()).await.unwrap();
    let val = cache.get("key1").await;
    assert_eq!(val.as_deref(), Some("value1"));

    // Stats.
    let stats = cache.stats();
    assert_eq!(stats.hits(), 1);
    assert_eq!(stats.misses(), 0);

    // Miss.
    let missing = cache.get("nonexistent").await;
    assert!(missing.is_none());

    let stats = cache.stats();
    assert_eq!(stats.misses(), 1);

    // Invalidate.
    cache.invalidate("key1").await;
    let val = cache.get("key1").await;
    assert!(val.is_none());
}
