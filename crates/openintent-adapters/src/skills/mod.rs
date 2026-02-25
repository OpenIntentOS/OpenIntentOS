//! Skills adapter for executing OpenIntentOS skills.
//!
//! This adapter provides tools for discovering and executing skills from the
//! skills crate. Skills are specialized functions that can be called by the
//! agent to perform specific tasks like OAuth setup, data processing, etc.

use async_trait::async_trait;
use serde_json::{json, Value};
use std::sync::Arc;
use tracing::{debug, error, info, warn};

use crate::error::{AdapterError, Result};
use openintent_agent::{ToolAdapter, ToolDefinition};

/// Adapter for executing OpenIntentOS skills.
pub struct SkillsAdapter {
    /// Base path to the skills directory
    skills_path: String,
}

impl SkillsAdapter {
    /// Create a new skills adapter.
    pub fn new(skills_path: String) -> Self {
        Self { skills_path }
    }

    /// Execute a skill by name with the given arguments.
    async fn execute_skill(&self, skill_name: &str, args: &Value) -> Result<String> {
        debug!(skill = %skill_name, "executing skill");

        match skill_name {
            "skill_email_oauth_setup" => {
                let email = args
                    .get("email")
                    .and_then(|v| v.as_str())
                    .ok_or_else(|| AdapterError::InvalidInput("email parameter required".into()))?;

                match skills::execute_email_oauth_setup(email).await {
                    Ok(result) => {
                        info!(skill = %skill_name, email = %email, "skill executed successfully");
                        Ok(result)
                    }
                    Err(e) => {
                        error!(skill = %skill_name, error = %e, "skill execution failed");
                        Err(AdapterError::ExecutionFailed(format!("Skill failed: {}", e)))
                    }
                }
            }
            "skill_ip_lookup_lookup" => {
                let args_str = args
                    .get("args")
                    .and_then(|v| v.as_str())
                    .unwrap_or("");

                // Execute the IP lookup skill script
                let script_path = format!("{}/ip-lookup/lookup.sh", self.skills_path);
                let output = tokio::process::Command::new("bash")
                    .arg(&script_path)
                    .arg(args_str)
                    .output()
                    .await
                    .map_err(|e| AdapterError::ExecutionFailed(format!("Failed to execute script: {}", e)))?;

                if output.status.success() {
                    let result = String::from_utf8_lossy(&output.stdout);
                    info!(skill = %skill_name, "skill executed successfully");
                    Ok(result.to_string())
                } else {
                    let error = String::from_utf8_lossy(&output.stderr);
                    error!(skill = %skill_name, error = %error, "skill execution failed");
                    Err(AdapterError::ExecutionFailed(format!("Script failed: {}", error)))
                }
            }
            _ => {
                warn!(skill = %skill_name, "unknown skill requested");
                Err(AdapterError::InvalidInput(format!("Unknown skill: {}", skill_name)))
            }
        }
    }
}

#[async_trait]
impl ToolAdapter for SkillsAdapter {
    fn adapter_id(&self) -> &str {
        "skills"
    }

    fn tool_definitions(&self) -> Vec<ToolDefinition> {
        vec![
            ToolDefinition {
                name: "skill_email_oauth_setup".into(),
                description: "Setup OAuth authentication for an email account. Automatically detects provider (Gmail, Outlook, Yahoo) and guides user through secure OAuth flow.".into(),
                input_schema: json!({
                    "type": "object",
                    "properties": {
                        "email": {
                            "type": "string",
                            "description": "The email address to setup OAuth for (e.g., user@gmail.com)"
                        }
                    },
                    "required": ["email"]
                }),
            },
            ToolDefinition {
                name: "skill_ip_lookup_lookup".into(),
                description: "Look up your public IP address and geolocation information.".into(),
                input_schema: json!({
                    "type": "object",
                    "properties": {
                        "args": {
                            "type": "string",
                            "description": "Arguments to pass to the script (JSON string or plain text)"
                        }
                    }
                }),
            },
        ]
    }

    async fn execute(&self, tool_name: &str, arguments: Value) -> Result<String> {
        debug!(tool = %tool_name, args = %arguments, "executing skill tool");
        self.execute_skill(tool_name, &arguments).await
    }
}

impl Default for SkillsAdapter {
    fn default() -> Self {
        Self::new("/Users/cw/development/OpenIntentOS/skills".to_string())
    }
}