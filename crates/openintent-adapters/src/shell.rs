//! Shell adapter -- execute shell commands with configurable working directory
//! and timeout.
//!
//! This adapter wraps `tokio::process::Command` to provide async command
//! execution.  It returns stdout, stderr, and exit code as structured JSON.
//! Output is truncated to [`MAX_OUTPUT_BYTES`] (100 KB) to prevent memory
//! exhaustion from runaway commands.

use async_trait::async_trait;
use serde_json::{Value, json};
use tracing::{debug, info, warn};

use crate::error::{AdapterError, Result};
use crate::traits::{Adapter, AdapterType, AuthRequirement, HealthStatus, ToolDefinition};

/// Default command timeout in seconds.
const DEFAULT_TIMEOUT_SECS: u64 = 30;

/// Maximum output size in bytes (100 KB).  Stdout and stderr are each
/// independently truncated to this limit.
const MAX_OUTPUT_BYTES: usize = 100 * 1024;

/// Shell service adapter.
pub struct ShellAdapter {
    /// Unique identifier for this adapter instance.
    id: String,
    /// Default working directory for commands.
    working_dir: std::path::PathBuf,
    /// Default timeout for command execution in seconds.
    default_timeout_secs: u64,
    /// Whether the adapter has been connected.
    connected: bool,
}

impl ShellAdapter {
    /// Create a new shell adapter with a default working directory.
    pub fn new(id: impl Into<String>, working_dir: impl Into<std::path::PathBuf>) -> Self {
        Self {
            id: id.into(),
            working_dir: working_dir.into(),
            default_timeout_secs: DEFAULT_TIMEOUT_SECS,
            connected: false,
        }
    }

    /// Set the default timeout for command execution.
    pub fn with_timeout(mut self, timeout_secs: u64) -> Self {
        self.default_timeout_secs = timeout_secs;
        self
    }

    /// Execute a shell command and return structured output.
    async fn tool_shell_execute(&self, params: Value) -> Result<Value> {
        let command = params
            .get("command")
            .and_then(|v| v.as_str())
            .ok_or_else(|| AdapterError::InvalidParams {
                tool_name: "shell_execute".into(),
                reason: "missing required string field `command`".into(),
            })?;

        let working_dir = params
            .get("working_dir")
            .and_then(|v| v.as_str())
            .map(std::path::PathBuf::from)
            .unwrap_or_else(|| self.working_dir.clone());

        let timeout_secs = params
            .get("timeout_secs")
            .and_then(|v| v.as_u64())
            .unwrap_or(self.default_timeout_secs);

        debug!(
            command = command,
            working_dir = %working_dir.display(),
            timeout_secs = timeout_secs,
            "executing shell command"
        );

        // Spawn the command via the system shell.
        let child = tokio::process::Command::new("sh")
            .arg("-c")
            .arg(command)
            .current_dir(&working_dir)
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped())
            .kill_on_drop(true)
            .spawn()
            .map_err(|e| AdapterError::ExecutionFailed {
                tool_name: "shell_execute".into(),
                reason: format!("failed to spawn process: {e}"),
            })?;

        // Wait with timeout.  `wait_with_output` takes ownership, so on
        // timeout the child is dropped and killed via `kill_on_drop(true)`.
        let result = tokio::time::timeout(
            std::time::Duration::from_secs(timeout_secs),
            child.wait_with_output(),
        )
        .await;

        match result {
            Ok(Ok(output)) => {
                let exit_code = output.status.code().unwrap_or(-1);
                let (stdout, stdout_truncated) = truncate_output(&output.stdout);
                let (stderr, stderr_truncated) = truncate_output(&output.stderr);

                if stdout_truncated || stderr_truncated {
                    debug!(
                        exit_code = exit_code,
                        stdout_truncated = stdout_truncated,
                        stderr_truncated = stderr_truncated,
                        "command completed (output truncated)"
                    );
                } else {
                    debug!(exit_code = exit_code, "command completed");
                }

                Ok(json!({
                    "command": command,
                    "exit_code": exit_code,
                    "stdout": stdout,
                    "stderr": stderr,
                    "stdout_truncated": stdout_truncated,
                    "stderr_truncated": stderr_truncated,
                    "success": exit_code == 0,
                }))
            }
            Ok(Err(e)) => Err(AdapterError::ExecutionFailed {
                tool_name: "shell_execute".into(),
                reason: format!("process error: {e}"),
            }),
            Err(_) => {
                // Timeout -- child is killed on drop via kill_on_drop(true).
                warn!(
                    command = command,
                    timeout_secs = timeout_secs,
                    "command timed out"
                );
                Err(AdapterError::Timeout {
                    seconds: timeout_secs,
                    reason: format!("shell command `{command}` exceeded time limit"),
                })
            }
        }
    }
}

/// Truncate raw command output to [`MAX_OUTPUT_BYTES`], converting to a
/// lossy UTF-8 string.  Returns `(output_string, was_truncated)`.
fn truncate_output(raw: &[u8]) -> (String, bool) {
    if raw.len() <= MAX_OUTPUT_BYTES {
        (String::from_utf8_lossy(raw).into_owned(), false)
    } else {
        let truncated = &raw[..MAX_OUTPUT_BYTES];
        let mut s = String::from_utf8_lossy(truncated).into_owned();
        s.push_str("\n... [output truncated at 100 KB]");
        (s, true)
    }
}

#[async_trait]
impl Adapter for ShellAdapter {
    fn id(&self) -> &str {
        &self.id
    }

    fn adapter_type(&self) -> AdapterType {
        AdapterType::System
    }

    async fn connect(&mut self) -> Result<()> {
        info!(id = %self.id, cwd = %self.working_dir.display(), "shell adapter connected");
        self.connected = true;
        Ok(())
    }

    async fn disconnect(&mut self) -> Result<()> {
        info!(id = %self.id, "shell adapter disconnected");
        self.connected = false;
        Ok(())
    }

    async fn health_check(&self) -> Result<HealthStatus> {
        if !self.connected {
            return Ok(HealthStatus::Unhealthy);
        }
        // Quick sanity check -- can we run `true`?
        match tokio::process::Command::new("true").output().await {
            Ok(output) if output.status.success() => Ok(HealthStatus::Healthy),
            _ => Ok(HealthStatus::Degraded),
        }
    }

    fn tools(&self) -> Vec<ToolDefinition> {
        vec![ToolDefinition {
            name: "shell_execute".into(),
            description: "Execute a shell command and return stdout, stderr, and exit code".into(),
            parameters: json!({
                "type": "object",
                "properties": {
                    "command": {
                        "type": "string",
                        "description": "The shell command to execute"
                    },
                    "working_dir": {
                        "type": "string",
                        "description": "Working directory for the command (optional)"
                    },
                    "timeout_secs": {
                        "type": "integer",
                        "description": "Timeout in seconds (default: 30)"
                    }
                },
                "required": ["command"]
            }),
        }]
    }

    async fn execute_tool(&self, name: &str, params: Value) -> Result<Value> {
        if !self.connected {
            return Err(AdapterError::ExecutionFailed {
                tool_name: name.to_string(),
                reason: format!("adapter `{}` is not connected", self.id),
            });
        }
        match name {
            "shell_execute" => self.tool_shell_execute(params).await,
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
    async fn shell_adapter_tools_not_empty() {
        let adapter = ShellAdapter::new("shell-test", "/tmp");
        assert_eq!(adapter.tools().len(), 1);
        assert_eq!(adapter.tools()[0].name, "shell_execute");
    }

    #[tokio::test]
    async fn shell_adapter_rejects_when_not_connected() {
        let adapter = ShellAdapter::new("shell-test", "/tmp");
        let result = adapter
            .execute_tool("shell_execute", json!({"command": "echo hello"}))
            .await;
        assert!(result.is_err());
    }

    #[test]
    fn truncate_output_short_input_not_truncated() {
        let data = b"hello world";
        let (s, truncated) = truncate_output(data);
        assert_eq!(s, "hello world");
        assert!(!truncated);
    }

    #[test]
    fn truncate_output_large_input_is_truncated() {
        let data = vec![b'x'; MAX_OUTPUT_BYTES + 1000];
        let (s, truncated) = truncate_output(&data);
        assert!(truncated);
        assert!(s.contains("[output truncated at 100 KB]"));
        // The string before the truncation message should be at most MAX_OUTPUT_BYTES
        // worth of characters (plus the suffix).
        assert!(s.len() <= MAX_OUTPUT_BYTES + 50);
    }
}
