//! Filesystem adapter -- read, write, list, create, delete, and inspect files.
//!
//! This adapter provides safe, async filesystem operations using `tokio::fs`.
//! All paths are resolved relative to an optional `root_dir` and validated
//! against path traversal attacks (e.g. `../../etc/passwd`).

use async_trait::async_trait;
use serde_json::{Value, json};
use tracing::{debug, info};

use crate::error::{AdapterError, Result};
use crate::traits::{Adapter, AdapterType, AuthRequirement, HealthStatus, ToolDefinition};

/// Filesystem service adapter.
pub struct FilesystemAdapter {
    /// Unique identifier for this adapter instance.
    id: String,
    /// Root directory for all file operations.  Paths supplied by tools are
    /// resolved relative to this directory and must not escape it.
    root_dir: std::path::PathBuf,
    /// Whether the adapter has been connected (initialised).
    connected: bool,
}

impl FilesystemAdapter {
    /// Create a new filesystem adapter rooted at `root_dir`.
    pub fn new(id: impl Into<String>, root_dir: impl Into<std::path::PathBuf>) -> Self {
        Self {
            id: id.into(),
            root_dir: root_dir.into(),
            connected: false,
        }
    }

    /// Resolve a user-supplied path against the root directory and validate
    /// that the result does not escape the root (path traversal protection).
    ///
    /// Returns the canonicalized absolute path on success or an error if the
    /// resolved path would leave the root directory.
    fn safe_resolve(&self, raw_path: &str, tool_name: &str) -> Result<std::path::PathBuf> {
        let candidate = if std::path::Path::new(raw_path).is_absolute() {
            std::path::PathBuf::from(raw_path)
        } else {
            self.root_dir.join(raw_path)
        };

        // Build a normalized path without touching the filesystem (the target
        // may not exist yet, so canonicalize() would fail).
        let normalized = normalize_path(&candidate);

        // Canonicalize the root so the prefix check is reliable.
        let canon_root = self
            .root_dir
            .canonicalize()
            .unwrap_or_else(|_| self.root_dir.clone());

        if !normalized.starts_with(&canon_root) {
            return Err(AdapterError::InvalidParams {
                tool_name: tool_name.to_string(),
                reason: format!(
                    "path `{raw_path}` resolves to `{}` which is outside the root directory `{}`",
                    normalized.display(),
                    canon_root.display(),
                ),
            });
        }

        Ok(normalized)
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

    async fn tool_fs_read_file(&self, params: Value) -> Result<Value> {
        let path_str = Self::require_str(&params, "path", "fs_read_file")?;
        let full_path = self.safe_resolve(path_str, "fs_read_file")?;
        debug!(path = %full_path.display(), "reading file");

        let content = tokio::fs::read_to_string(&full_path).await?;
        Ok(json!({
            "path": full_path.display().to_string(),
            "content": content,
            "size_bytes": content.len(),
        }))
    }

    async fn tool_fs_write_file(&self, params: Value) -> Result<Value> {
        let path_str = Self::require_str(&params, "path", "fs_write_file")?;
        let content = Self::require_str(&params, "content", "fs_write_file")?;
        let full_path = self.safe_resolve(path_str, "fs_write_file")?;
        debug!(path = %full_path.display(), "writing file");

        // Ensure parent directory exists.
        if let Some(parent) = full_path.parent() {
            tokio::fs::create_dir_all(parent).await?;
        }

        tokio::fs::write(&full_path, content).await?;
        let meta = tokio::fs::metadata(&full_path).await?;
        Ok(json!({
            "path": full_path.display().to_string(),
            "size_bytes": meta.len(),
            "success": true,
        }))
    }

    async fn tool_fs_list_directory(&self, params: Value) -> Result<Value> {
        let path_str = params.get("path").and_then(|v| v.as_str()).unwrap_or(".");
        let full_path = self.safe_resolve(path_str, "fs_list_directory")?;
        debug!(path = %full_path.display(), "listing directory");

        let mut entries = Vec::new();
        let mut read_dir = tokio::fs::read_dir(&full_path).await?;

        while let Some(entry) = read_dir.next_entry().await? {
            let file_type = entry.file_type().await?;
            let meta = entry.metadata().await?;
            entries.push(json!({
                "name": entry.file_name().to_string_lossy(),
                "is_dir": file_type.is_dir(),
                "is_file": file_type.is_file(),
                "size_bytes": meta.len(),
            }));
        }

        let count = entries.len();
        Ok(json!({
            "path": full_path.display().to_string(),
            "entries": entries,
            "count": count,
        }))
    }

    async fn tool_fs_create_directory(&self, params: Value) -> Result<Value> {
        let path_str = Self::require_str(&params, "path", "fs_create_directory")?;
        let full_path = self.safe_resolve(path_str, "fs_create_directory")?;
        debug!(path = %full_path.display(), "creating directory");

        tokio::fs::create_dir_all(&full_path).await?;
        Ok(json!({
            "path": full_path.display().to_string(),
            "success": true,
        }))
    }

    async fn tool_fs_delete(&self, params: Value) -> Result<Value> {
        let path_str = Self::require_str(&params, "path", "fs_delete")?;
        let full_path = self.safe_resolve(path_str, "fs_delete")?;
        debug!(path = %full_path.display(), "deleting");

        let meta = tokio::fs::metadata(&full_path).await?;
        if meta.is_dir() {
            tokio::fs::remove_dir_all(&full_path).await?;
        } else {
            tokio::fs::remove_file(&full_path).await?;
        }

        Ok(json!({
            "path": full_path.display().to_string(),
            "success": true,
        }))
    }

    async fn tool_fs_file_info(&self, params: Value) -> Result<Value> {
        let path_str = Self::require_str(&params, "path", "fs_file_info")?;
        let full_path = self.safe_resolve(path_str, "fs_file_info")?;
        debug!(path = %full_path.display(), "getting file info");

        let meta = tokio::fs::metadata(&full_path).await?;
        let modified = meta
            .modified()
            .ok()
            .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
            .map(|d| d.as_secs());
        let created = meta
            .created()
            .ok()
            .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
            .map(|d| d.as_secs());

        Ok(json!({
            "path": full_path.display().to_string(),
            "is_file": meta.is_file(),
            "is_dir": meta.is_dir(),
            "is_symlink": meta.is_symlink(),
            "size_bytes": meta.len(),
            "readonly": meta.permissions().readonly(),
            "modified_epoch": modified,
            "created_epoch": created,
        }))
    }
}

/// Normalize a path by resolving `.` and `..` components without touching the
/// filesystem.  This is necessary because the target path may not exist yet
/// (e.g. when writing a new file).
fn normalize_path(path: &std::path::Path) -> std::path::PathBuf {
    let mut components = Vec::new();
    for component in path.components() {
        match component {
            std::path::Component::ParentDir => {
                // Pop only if there is a normal component to pop.
                if matches!(components.last(), Some(std::path::Component::Normal(_))) {
                    components.pop();
                } else {
                    components.push(component);
                }
            }
            std::path::Component::CurDir => { /* skip */ }
            _ => components.push(component),
        }
    }
    components.iter().collect()
}

#[async_trait]
impl Adapter for FilesystemAdapter {
    fn id(&self) -> &str {
        &self.id
    }

    fn adapter_type(&self) -> AdapterType {
        AdapterType::System
    }

    async fn connect(&mut self) -> Result<()> {
        info!(id = %self.id, root = %self.root_dir.display(), "filesystem adapter connected");
        self.connected = true;
        Ok(())
    }

    async fn disconnect(&mut self) -> Result<()> {
        info!(id = %self.id, "filesystem adapter disconnected");
        self.connected = false;
        Ok(())
    }

    async fn health_check(&self) -> Result<HealthStatus> {
        if !self.connected {
            return Ok(HealthStatus::Unhealthy);
        }
        // Verify the root directory is accessible.
        match tokio::fs::metadata(&self.root_dir).await {
            Ok(meta) if meta.is_dir() => Ok(HealthStatus::Healthy),
            Ok(_) => Ok(HealthStatus::Degraded),
            Err(_) => Ok(HealthStatus::Unhealthy),
        }
    }

    fn tools(&self) -> Vec<ToolDefinition> {
        vec![
            ToolDefinition {
                name: "fs_read_file".into(),
                description: "Read the contents of a file".into(),
                parameters: json!({
                    "type": "object",
                    "properties": {
                        "path": { "type": "string", "description": "Path to the file to read" }
                    },
                    "required": ["path"]
                }),
            },
            ToolDefinition {
                name: "fs_write_file".into(),
                description: "Write content to a file, creating it if necessary".into(),
                parameters: json!({
                    "type": "object",
                    "properties": {
                        "path": { "type": "string", "description": "Path to the file to write" },
                        "content": { "type": "string", "description": "Content to write" }
                    },
                    "required": ["path", "content"]
                }),
            },
            ToolDefinition {
                name: "fs_list_directory".into(),
                description: "List entries in a directory".into(),
                parameters: json!({
                    "type": "object",
                    "properties": {
                        "path": { "type": "string", "description": "Directory path (default: root dir)" }
                    }
                }),
            },
            ToolDefinition {
                name: "fs_create_directory".into(),
                description: "Create a directory (including parents)".into(),
                parameters: json!({
                    "type": "object",
                    "properties": {
                        "path": { "type": "string", "description": "Directory path to create" }
                    },
                    "required": ["path"]
                }),
            },
            ToolDefinition {
                name: "fs_delete".into(),
                description: "Delete a file or directory".into(),
                parameters: json!({
                    "type": "object",
                    "properties": {
                        "path": { "type": "string", "description": "Path to delete" }
                    },
                    "required": ["path"]
                }),
            },
            ToolDefinition {
                name: "fs_file_info".into(),
                description: "Get metadata about a file or directory".into(),
                parameters: json!({
                    "type": "object",
                    "properties": {
                        "path": { "type": "string", "description": "Path to inspect" }
                    },
                    "required": ["path"]
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
            "fs_read_file" => self.tool_fs_read_file(params).await,
            "fs_write_file" => self.tool_fs_write_file(params).await,
            "fs_list_directory" => self.tool_fs_list_directory(params).await,
            "fs_create_directory" => self.tool_fs_create_directory(params).await,
            "fs_delete" => self.tool_fs_delete(params).await,
            "fs_file_info" => self.tool_fs_file_info(params).await,
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

    #[tokio::test]
    async fn filesystem_adapter_tools_not_empty() {
        let adapter = FilesystemAdapter::new("fs-test", "/tmp");
        assert_eq!(adapter.tools().len(), 6);
    }

    #[tokio::test]
    async fn filesystem_adapter_health_when_disconnected() {
        let adapter = FilesystemAdapter::new("fs-test", "/tmp");
        let status = adapter.health_check().await.unwrap();
        assert_eq!(status, HealthStatus::Unhealthy);
    }

    #[tokio::test]
    async fn filesystem_adapter_rejects_when_not_connected() {
        let adapter = FilesystemAdapter::new("fs-test", "/tmp");
        let result = adapter
            .execute_tool("fs_read_file", json!({"path": "/tmp/test.txt"}))
            .await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn path_traversal_is_blocked() {
        let adapter = FilesystemAdapter::new("fs-test", "/tmp/sandbox");
        let result = adapter.safe_resolve("../../etc/passwd", "fs_read_file");
        assert!(result.is_err());
        let err_msg = format!("{}", result.unwrap_err());
        assert!(err_msg.contains("outside the root directory"));
    }

    #[test]
    fn normalize_path_resolves_parent_components() {
        let p = std::path::Path::new("/tmp/sandbox/sub/../other");
        let norm = normalize_path(p);
        assert_eq!(norm, std::path::PathBuf::from("/tmp/sandbox/other"));
    }

    #[test]
    fn normalize_path_resolves_current_dir_components() {
        let p = std::path::Path::new("/tmp/./sandbox/./file.txt");
        let norm = normalize_path(p);
        assert_eq!(norm, std::path::PathBuf::from("/tmp/sandbox/file.txt"));
    }
}
