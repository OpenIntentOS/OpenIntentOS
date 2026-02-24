//! Integration tests for the openintent-adapters crate.
//!
//! These tests exercise the adapter trait implementations end-to-end,
//! verifying tool discovery, execution, and lifecycle management.

use openintent_adapters::{
    Adapter, CronAdapter, FilesystemAdapter, HealthStatus, HttpRequestAdapter, ShellAdapter,
    WebFetchAdapter, WebSearchAdapter,
};
use serde_json::json;

/// On macOS, tempfile returns `/var/folders/...` but the filesystem
/// canonicalizes to `/private/var/folders/...`. We canonicalize the temp
/// dir path to match the adapter's path traversal checks.
fn canon_tempdir() -> (tempfile::TempDir, std::path::PathBuf) {
    let dir = tempfile::tempdir().unwrap();
    let canon = dir.path().canonicalize().unwrap();
    (dir, canon)
}

// ═══════════════════════════════════════════════════════════════════════
//  Filesystem adapter
// ═══════════════════════════════════════════════════════════════════════

#[tokio::test]
async fn filesystem_adapter_lifecycle() {
    let (_dir, canon) = canon_tempdir();
    let mut adapter = FilesystemAdapter::new("test-fs", &canon);

    // Health check before connect should be unhealthy.
    let health = adapter.health_check().await.unwrap();
    assert_eq!(health, HealthStatus::Unhealthy);

    adapter.connect().await.unwrap();

    // Health check after connect should be healthy.
    let health = adapter.health_check().await.unwrap();
    assert_eq!(health, HealthStatus::Healthy);

    // Write a file.
    let result = adapter
        .execute_tool(
            "fs_write_file",
            json!({
                "path": "test.txt",
                "content": "hello world"
            }),
        )
        .await
        .unwrap();
    assert_eq!(result["success"], true);

    // Read it back.
    let result = adapter
        .execute_tool("fs_read_file", json!({"path": "test.txt"}))
        .await
        .unwrap();
    let content = result.get("content").and_then(|v| v.as_str()).unwrap();
    assert_eq!(content, "hello world");

    // List directory.
    let result = adapter
        .execute_tool("fs_list_directory", json!({"path": "."}))
        .await
        .unwrap();
    let entries = result["entries"].as_array().unwrap();
    assert_eq!(entries.len(), 1);
    assert_eq!(entries[0]["name"], "test.txt");

    // File info.
    let result = adapter
        .execute_tool("fs_file_info", json!({"path": "test.txt"}))
        .await
        .unwrap();
    assert_eq!(result["is_file"], true);
    assert_eq!(result["is_dir"], false);

    // Create subdirectory.
    adapter
        .execute_tool("fs_create_directory", json!({"path": "subdir"}))
        .await
        .unwrap();

    let result = adapter
        .execute_tool("fs_file_info", json!({"path": "subdir"}))
        .await
        .unwrap();
    assert_eq!(result["is_dir"], true);

    // Delete file.
    adapter
        .execute_tool("fs_delete", json!({"path": "test.txt"}))
        .await
        .unwrap();

    // Verify file is gone.
    let result = adapter
        .execute_tool("fs_read_file", json!({"path": "test.txt"}))
        .await;
    assert!(result.is_err());

    // Delete directory.
    adapter
        .execute_tool("fs_delete", json!({"path": "subdir"}))
        .await
        .unwrap();

    // Disconnect.
    adapter.disconnect().await.unwrap();

    // After disconnect, tools should fail.
    let result = adapter
        .execute_tool("fs_read_file", json!({"path": "anything"}))
        .await;
    assert!(result.is_err());
}

#[tokio::test]
async fn filesystem_adapter_path_traversal_blocked() {
    let (_dir, canon) = canon_tempdir();
    let mut adapter = FilesystemAdapter::new("test-fs", &canon);
    adapter.connect().await.unwrap();

    let result = adapter
        .execute_tool("fs_read_file", json!({"path": "../../etc/passwd"}))
        .await;
    assert!(result.is_err());
}

#[tokio::test]
async fn filesystem_adapter_write_nested_creates_parents() {
    let (_dir, canon) = canon_tempdir();
    let mut adapter = FilesystemAdapter::new("test-fs", &canon);
    adapter.connect().await.unwrap();

    // Write to a nested path -- parent directories should be created.
    adapter
        .execute_tool(
            "fs_write_file",
            json!({
                "path": "a/b/c/deep.txt",
                "content": "nested"
            }),
        )
        .await
        .unwrap();

    let result = adapter
        .execute_tool("fs_read_file", json!({"path": "a/b/c/deep.txt"}))
        .await
        .unwrap();
    assert_eq!(result["content"], "nested");
}

// ═══════════════════════════════════════════════════════════════════════
//  Shell adapter
// ═══════════════════════════════════════════════════════════════════════

#[tokio::test]
async fn shell_adapter_executes_command() {
    let (_dir, canon) = canon_tempdir();
    let mut adapter = ShellAdapter::new("test-shell", &canon);
    adapter.connect().await.unwrap();

    let result = adapter
        .execute_tool("shell_execute", json!({"command": "echo hello"}))
        .await
        .unwrap();

    let stdout = result.get("stdout").and_then(|v| v.as_str()).unwrap();
    assert!(stdout.contains("hello"));
    assert_eq!(result["exit_code"], 0);
    assert_eq!(result["success"], true);
}

#[tokio::test]
async fn shell_adapter_captures_stderr() {
    let (_dir, canon) = canon_tempdir();
    let mut adapter = ShellAdapter::new("test-shell", &canon);
    adapter.connect().await.unwrap();

    let result = adapter
        .execute_tool("shell_execute", json!({"command": "echo error >&2"}))
        .await
        .unwrap();

    let stderr = result.get("stderr").and_then(|v| v.as_str()).unwrap();
    assert!(stderr.contains("error"));
}

#[tokio::test]
async fn shell_adapter_reports_exit_code() {
    let (_dir, canon) = canon_tempdir();
    let mut adapter = ShellAdapter::new("test-shell", &canon);
    adapter.connect().await.unwrap();

    let result = adapter
        .execute_tool("shell_execute", json!({"command": "exit 42"}))
        .await
        .unwrap();

    assert_eq!(result["exit_code"], 42);
    assert_eq!(result["success"], false);
}

#[tokio::test]
async fn shell_adapter_health_check() {
    let (_dir, canon) = canon_tempdir();
    let mut adapter = ShellAdapter::new("test-shell", &canon);
    adapter.connect().await.unwrap();

    let health = adapter.health_check().await.unwrap();
    assert_eq!(health, HealthStatus::Healthy);
}

// ═══════════════════════════════════════════════════════════════════════
//  Cron adapter
// ═══════════════════════════════════════════════════════════════════════

#[tokio::test]
async fn cron_adapter_crud() {
    let mut adapter = CronAdapter::new("test-cron");
    adapter.connect().await.unwrap();

    // Create job.
    let result = adapter
        .execute_tool(
            "cron_create",
            json!({
                "name": "test-job",
                "schedule": "0 9 * * *",
                "command": "echo hello"
            }),
        )
        .await
        .unwrap();
    assert!(result.get("id").is_some());
    let id = result["id"].as_str().unwrap().to_string();

    // List jobs.
    let result = adapter.execute_tool("cron_list", json!({})).await.unwrap();
    let jobs = result["jobs"].as_array().unwrap();
    assert_eq!(jobs.len(), 1);
    assert_eq!(jobs[0]["name"], "test-job");
    assert_eq!(jobs[0]["schedule"], "0 9 * * *");
    assert_eq!(jobs[0]["enabled"], true);

    // Toggle off.
    let result = adapter
        .execute_tool("cron_toggle", json!({"id": id, "enabled": false}))
        .await
        .unwrap();
    assert_eq!(result["enabled"], false);

    // Verify toggle via list.
    let result = adapter.execute_tool("cron_list", json!({})).await.unwrap();
    assert_eq!(result["jobs"][0]["enabled"], false);

    // Toggle back on.
    adapter
        .execute_tool("cron_toggle", json!({"id": id, "enabled": true}))
        .await
        .unwrap();

    // Delete.
    adapter
        .execute_tool("cron_delete", json!({"id": id}))
        .await
        .unwrap();

    let result = adapter.execute_tool("cron_list", json!({})).await.unwrap();
    let jobs = result["jobs"].as_array().unwrap();
    assert!(jobs.is_empty());
}

#[tokio::test]
async fn cron_adapter_multiple_jobs() {
    let mut adapter = CronAdapter::new("test-cron");
    adapter.connect().await.unwrap();

    // Create three jobs.
    for i in 0..3 {
        adapter
            .execute_tool(
                "cron_create",
                json!({
                    "name": format!("job-{i}"),
                    "schedule": "* * * * *",
                    "command": format!("echo {i}")
                }),
            )
            .await
            .unwrap();
    }

    let result = adapter.execute_tool("cron_list", json!({})).await.unwrap();
    let jobs = result["jobs"].as_array().unwrap();
    assert_eq!(jobs.len(), 3);
}

// ═══════════════════════════════════════════════════════════════════════
//  HTTP request adapter
// ═══════════════════════════════════════════════════════════════════════

#[tokio::test]
async fn http_request_adapter_lifecycle() {
    let mut adapter = HttpRequestAdapter::new("test-http");

    // Before connect, should be unhealthy.
    let health = adapter.health_check().await.unwrap();
    assert_eq!(health, HealthStatus::Unhealthy);

    adapter.connect().await.unwrap();

    // After connect, should be healthy.
    let health = adapter.health_check().await.unwrap();
    assert_eq!(health, HealthStatus::Healthy);

    // Making a real HTTP request might fail in CI without network.
    // We test that the adapter does not panic and handles both cases.
    let result = adapter
        .execute_tool(
            "http_request",
            json!({
                "method": "GET",
                "url": "https://httpbin.org/get",
                "timeout_seconds": 10
            }),
        )
        .await;

    match result {
        Ok(val) => {
            assert!(val.get("status").is_some());
        }
        Err(_) => {
            // Network may not be available -- acceptable.
        }
    }

    adapter.disconnect().await.unwrap();
}

// ═══════════════════════════════════════════════════════════════════════
//  Tool definitions validation
// ═══════════════════════════════════════════════════════════════════════

#[tokio::test]
async fn adapter_tools_are_well_defined() {
    let (_dir, canon) = canon_tempdir();

    // Filesystem adapter.
    let mut fs = FilesystemAdapter::new("fs", &canon);
    fs.connect().await.unwrap();
    let tools = fs.tools();
    assert!(!tools.is_empty());
    for tool in &tools {
        assert!(!tool.name.is_empty(), "tool name must not be empty");
        assert!(
            !tool.description.is_empty(),
            "tool description must not be empty"
        );
    }

    // Shell adapter.
    let mut shell = ShellAdapter::new("sh", &canon);
    shell.connect().await.unwrap();
    let tools = shell.tools();
    assert!(!tools.is_empty());
    assert_eq!(tools[0].name, "shell_execute");

    // Web search adapter.
    let mut ws = WebSearchAdapter::new("ws");
    ws.connect().await.unwrap();
    let tools = ws.tools();
    assert!(!tools.is_empty());

    // Web fetch adapter.
    let mut wf = WebFetchAdapter::new("wf");
    wf.connect().await.unwrap();
    let tools = wf.tools();
    assert!(!tools.is_empty());

    // HTTP request adapter.
    let mut http = HttpRequestAdapter::new("http");
    http.connect().await.unwrap();
    let tools = http.tools();
    assert!(!tools.is_empty());

    // Cron adapter.
    let mut cron = CronAdapter::new("cron");
    cron.connect().await.unwrap();
    let tools = cron.tools();
    assert_eq!(tools.len(), 4);
}

#[tokio::test]
async fn unknown_tool_returns_error() {
    let (_dir, canon) = canon_tempdir();
    let mut fs = FilesystemAdapter::new("fs", &canon);
    fs.connect().await.unwrap();

    let result = fs.execute_tool("nonexistent_tool", json!({})).await;
    assert!(result.is_err());
}
