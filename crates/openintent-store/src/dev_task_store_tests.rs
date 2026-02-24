use super::*;
use serde_json::json;

async fn setup_db() -> Database {
    let db = Database::open_in_memory().unwrap();
    db.run_migrations().await.unwrap();
    db
}

#[tokio::test]
async fn create_and_get() {
    let db = setup_db().await;
    let store = DevTaskStore::new(db);

    let task = store
        .create("telegram", Some(12345), "add rate limiting")
        .await
        .unwrap();
    assert_eq!(task.source, "telegram");
    assert_eq!(task.chat_id, Some(12345));
    assert_eq!(task.intent, "add rate limiting");
    assert_eq!(task.status, "pending");
    assert_eq!(task.retry_count, 0);
    assert_eq!(task.max_retries, 3);
    assert_eq!(task.progress_log, json!([]));
    assert!(task.branch.is_none());
    assert!(task.pr_url.is_none());
    assert!(task.error.is_none());

    let fetched = store.get(&task.id).await.unwrap().unwrap();
    assert_eq!(fetched.id, task.id);
    assert_eq!(fetched.intent, "add rate limiting");
}

#[tokio::test]
async fn get_nonexistent_returns_none() {
    let db = setup_db().await;
    let store = DevTaskStore::new(db);

    let result = store.get("nonexistent-id").await.unwrap();
    assert!(result.is_none());
}

#[tokio::test]
async fn list_by_status() {
    let db = setup_db().await;
    let store = DevTaskStore::new(db);

    store.create("telegram", Some(1), "task a").await.unwrap();
    let task_b = store.create("cli", None, "task b").await.unwrap();
    store.create("telegram", Some(1), "task c").await.unwrap();

    // Move task_b to coding.
    store
        .update_status(&task_b.id, "coding", Some("writing code"))
        .await
        .unwrap();

    let pending = store.list_by_status("pending", 10, 0).await.unwrap();
    assert_eq!(pending.len(), 2);

    let coding = store.list_by_status("coding", 10, 0).await.unwrap();
    assert_eq!(coding.len(), 1);
    assert_eq!(coding[0].id, task_b.id);
}

#[tokio::test]
async fn list_by_chat() {
    let db = setup_db().await;
    let store = DevTaskStore::new(db);

    store
        .create("telegram", Some(100), "for chat 100 a")
        .await
        .unwrap();
    store
        .create("telegram", Some(200), "for chat 200")
        .await
        .unwrap();
    store
        .create("telegram", Some(100), "for chat 100 b")
        .await
        .unwrap();

    let chat_100 = store.list_by_chat(100, 10, 0).await.unwrap();
    assert_eq!(chat_100.len(), 2);

    let chat_200 = store.list_by_chat(200, 10, 0).await.unwrap();
    assert_eq!(chat_200.len(), 1);

    let chat_999 = store.list_by_chat(999, 10, 0).await.unwrap();
    assert!(chat_999.is_empty());
}

#[tokio::test]
async fn update_status() {
    let db = setup_db().await;
    let store = DevTaskStore::new(db);

    let task = store.create("cli", None, "test status").await.unwrap();
    store
        .update_status(&task.id, "branching", Some("creating branch"))
        .await
        .unwrap();

    let fetched = store.get(&task.id).await.unwrap().unwrap();
    assert_eq!(fetched.status, "branching");
    assert_eq!(fetched.current_step.as_deref(), Some("creating branch"));
    assert!(fetched.updated_at >= task.updated_at);
}

#[tokio::test]
async fn update_status_not_found() {
    let db = setup_db().await;
    let store = DevTaskStore::new(db);

    let result = store.update_status("bad-id", "coding", None).await;
    assert!(result.is_err());
}

#[tokio::test]
async fn set_branch() {
    let db = setup_db().await;
    let store = DevTaskStore::new(db);

    let task = store.create("cli", None, "test branch").await.unwrap();
    store.set_branch(&task.id, "feat/dev-abc123").await.unwrap();

    let fetched = store.get(&task.id).await.unwrap().unwrap();
    assert_eq!(fetched.branch.as_deref(), Some("feat/dev-abc123"));
}

#[tokio::test]
async fn set_pr_url() {
    let db = setup_db().await;
    let store = DevTaskStore::new(db);

    let task = store.create("cli", None, "test pr").await.unwrap();
    store
        .set_pr_url(&task.id, "https://github.com/org/repo/pull/42")
        .await
        .unwrap();

    let fetched = store.get(&task.id).await.unwrap().unwrap();
    assert_eq!(
        fetched.pr_url.as_deref(),
        Some("https://github.com/org/repo/pull/42")
    );
}

#[tokio::test]
async fn set_error() {
    let db = setup_db().await;
    let store = DevTaskStore::new(db);

    let task = store.create("cli", None, "test error").await.unwrap();
    store
        .set_error(&task.id, "compilation failed")
        .await
        .unwrap();

    let fetched = store.get(&task.id).await.unwrap().unwrap();
    assert_eq!(fetched.error.as_deref(), Some("compilation failed"));
}

#[tokio::test]
async fn increment_retry() {
    let db = setup_db().await;
    let store = DevTaskStore::new(db);

    let task = store.create("cli", None, "test retry").await.unwrap();
    assert_eq!(task.retry_count, 0);

    let new_count = store.increment_retry(&task.id).await.unwrap();
    assert_eq!(new_count, 1);

    let new_count = store.increment_retry(&task.id).await.unwrap();
    assert_eq!(new_count, 2);

    let fetched = store.get(&task.id).await.unwrap().unwrap();
    assert_eq!(fetched.retry_count, 2);
}

#[tokio::test]
async fn append_progress() {
    let db = setup_db().await;
    let store = DevTaskStore::new(db);

    let task = store.create("cli", None, "test progress").await.unwrap();
    store
        .append_progress(&task.id, "step 1 done")
        .await
        .unwrap();
    store
        .append_progress(&task.id, "step 2 done")
        .await
        .unwrap();

    let fetched = store.get(&task.id).await.unwrap().unwrap();
    let log = fetched.progress_log.as_array().unwrap();
    assert_eq!(log.len(), 2);
    assert_eq!(log[0].as_str().unwrap(), "step 1 done");
    assert_eq!(log[1].as_str().unwrap(), "step 2 done");
}

#[tokio::test]
async fn messages_crud() {
    let db = setup_db().await;
    let store = DevTaskStore::new(db);

    let task = store
        .create("telegram", Some(1), "test messages")
        .await
        .unwrap();

    let msg_id_1 = store
        .append_message(&task.id, "user", "please add logging")
        .await
        .unwrap();
    assert!(msg_id_1 > 0);

    let msg_id_2 = store
        .append_message(&task.id, "agent", "working on it")
        .await
        .unwrap();
    assert!(msg_id_2 > msg_id_1);

    store
        .append_message(&task.id, "progress", "branch created")
        .await
        .unwrap();

    // Get all messages.
    let all = store.get_messages(&task.id, None).await.unwrap();
    assert_eq!(all.len(), 3);
    assert_eq!(all[0].role, "user");
    assert_eq!(all[1].role, "agent");
    assert_eq!(all[2].role, "progress");

    // Get limited messages (most recent 2).
    let recent = store.get_messages(&task.id, Some(2)).await.unwrap();
    assert_eq!(recent.len(), 2);
    assert_eq!(recent[0].role, "agent");
    assert_eq!(recent[1].role, "progress");
}

#[tokio::test]
async fn list_recoverable() {
    let db = setup_db().await;
    let store = DevTaskStore::new(db);

    let t1 = store.create("cli", None, "pending task").await.unwrap();
    let t2 = store.create("cli", None, "coding task").await.unwrap();
    let t3 = store.create("cli", None, "testing task").await.unwrap();
    let t4 = store.create("cli", None, "completed task").await.unwrap();

    // Leave t1 as pending (not recoverable).
    store
        .update_status(&t2.id, "coding", Some("writing"))
        .await
        .unwrap();
    store
        .update_status(&t3.id, "testing", Some("running tests"))
        .await
        .unwrap();
    store
        .update_status(&t4.id, "completed", None)
        .await
        .unwrap();

    let recoverable = store.list_recoverable().await.unwrap();
    assert_eq!(recoverable.len(), 2);
    // Both coding and testing are recoverable.
    let statuses: Vec<&str> = recoverable.iter().map(|t| t.status.as_str()).collect();
    assert!(statuses.contains(&"coding"));
    assert!(statuses.contains(&"testing"));
    // Not pending or completed.
    assert!(!statuses.contains(&"pending"));
    assert!(!statuses.contains(&"completed"));

    // Verify t1 is still pending.
    let pending = store.get(&t1.id).await.unwrap().unwrap();
    assert_eq!(pending.status, "pending");
}

#[tokio::test]
async fn cancel_and_delete() {
    let db = setup_db().await;
    let store = DevTaskStore::new(db);

    let task = store.create("cli", None, "to cancel").await.unwrap();
    store.cancel(&task.id).await.unwrap();

    let fetched = store.get(&task.id).await.unwrap().unwrap();
    assert_eq!(fetched.status, "cancelled");

    // Delete the task.
    store.delete(&task.id).await.unwrap();
    let gone = store.get(&task.id).await.unwrap();
    assert!(gone.is_none());
}

#[tokio::test]
async fn count_by_status() {
    let db = setup_db().await;
    let store = DevTaskStore::new(db);

    store.create("cli", None, "a").await.unwrap();
    store.create("cli", None, "b").await.unwrap();
    let c = store.create("cli", None, "c").await.unwrap();
    store.update_status(&c.id, "coding", None).await.unwrap();

    assert_eq!(store.count_by_status("pending").await.unwrap(), 2);
    assert_eq!(store.count_by_status("coding").await.unwrap(), 1);
    assert_eq!(store.count_by_status("completed").await.unwrap(), 0);
}

#[tokio::test]
async fn find_active_by_intent_deduplication() {
    let db = setup_db().await;
    let store = DevTaskStore::new(db);

    let chat_id = 42;
    let intent = "add rate limiting";

    // No task yet — should return None.
    let found = store.find_active_by_intent(chat_id, intent).await.unwrap();
    assert!(found.is_none());

    // Create a task — should now be found.
    let task = store
        .create("telegram", Some(chat_id), intent)
        .await
        .unwrap();
    let found = store.find_active_by_intent(chat_id, intent).await.unwrap();
    assert!(found.is_some());
    assert_eq!(found.unwrap().id, task.id);

    // Different chat — should NOT find it.
    let found = store.find_active_by_intent(999, intent).await.unwrap();
    assert!(found.is_none());

    // Different intent — should NOT find it.
    let found = store
        .find_active_by_intent(chat_id, "something else")
        .await
        .unwrap();
    assert!(found.is_none());

    // Cancel the task — should no longer be found.
    store.cancel(&task.id).await.unwrap();
    let found = store.find_active_by_intent(chat_id, intent).await.unwrap();
    assert!(found.is_none());
}
