//! Skill adapter — bridges loaded skills into the [`Adapter`] trait.
//!
//! Skills contribute to the agent in two ways:
//!
//! 1. **Prompt injection** — skill instructions are appended to the system
//!    prompt so the LLM knows how to use existing tools to accomplish the
//!    skill's purpose.
//!
//! 2. **Script tools** — skills that include executable scripts (`.sh`, `.py`,
//!    `.js`, `.ts`) are exposed as additional tools the agent can invoke.
//!    Scripts are executed via subprocess with captured stdout/stderr.

use std::process::Stdio;

use async_trait::async_trait;
use serde_json::{Value, json};

use openintent_adapters::error::Result;
use openintent_adapters::traits::{
    Adapter, AdapterType, AuthRequirement, HealthStatus, ToolDefinition,
};

use crate::types::{SkillDefinition, SkillScript};

/// An adapter that exposes script-based skill tools and manages prompt
/// injection for all loaded skills.
pub struct SkillAdapter {
    id: String,
    connected: bool,
    /// Script tools discovered from loaded skills.
    script_tools: Vec<ScriptTool>,
}

/// A tool backed by an executable script.
#[derive(Clone)]
struct ScriptTool {
    /// Tool name exposed to the LLM (e.g. `skill_todoist_run`).
    name: String,
    /// Description for the LLM.
    description: String,
    /// The skill this tool belongs to.
    skill_name: String,
    /// The script to execute.
    script: SkillScript,
}

impl SkillAdapter {
    /// Create a new skill adapter from loaded skills.
    ///
    /// Skills with executable scripts will have their scripts exposed as tools.
    /// All skills contribute prompt extensions regardless of whether they have
    /// scripts.
    pub fn new(id: impl Into<String>, skills: &[SkillDefinition]) -> Self {
        let mut script_tools = Vec::new();

        for skill in skills {
            for script in &skill.scripts {
                let tool_name = format!(
                    "skill_{}_{}",
                    skill.name.replace('-', "_"),
                    script
                        .filename
                        .rsplit('.')
                        .next_back()
                        .unwrap_or(&script.filename)
                        .replace('-', "_")
                );

                script_tools.push(ScriptTool {
                    name: tool_name,
                    description: format!(
                        "Execute the `{}` script from skill `{}`. {}",
                        script.filename, skill.name, skill.description
                    ),
                    skill_name: skill.name.clone(),
                    script: script.clone(),
                });
            }
        }

        tracing::info!(
            script_tools = script_tools.len(),
            skills = skills.len(),
            "skill adapter initialized"
        );

        Self {
            id: id.into(),
            connected: false,
            script_tools,
        }
    }

    /// Execute a script tool and return its output.
    async fn execute_script(&self, tool: &ScriptTool, params: Value) -> Result<Value> {
        tracing::debug!(
            skill = %tool.skill_name,
            script = %tool.script.filename,
            "executing skill script"
        );

        let interpreter = tool.script.interpreter;
        let script_path = &tool.script.path;

        let mut cmd = tokio::process::Command::new(interpreter.command());

        // Add interpreter-specific args.
        for arg in interpreter.args() {
            cmd.arg(arg);
        }

        cmd.arg(script_path);

        // Pass parameters as JSON string via stdin.
        cmd.stdin(Stdio::piped());
        cmd.stdout(Stdio::piped());
        cmd.stderr(Stdio::piped());

        // Pass individual parameters as environment variables.
        if let Some(obj) = params.as_object() {
            for (key, value) in obj {
                let val_str = match value {
                    Value::String(s) => s.clone(),
                    other => other.to_string(),
                };
                cmd.env(format!("SKILL_PARAM_{}", key.to_uppercase()), val_str);
            }
        }

        // Also pass full JSON as SKILL_PARAMS.
        cmd.env("SKILL_PARAMS", params.to_string());

        let child =
            cmd.spawn()
                .map_err(|e| openintent_adapters::AdapterError::ExecutionFailed {
                    tool_name: tool.name.clone(),
                    reason: format!("failed to spawn script: {e}"),
                })?;

        let output =
            tokio::time::timeout(std::time::Duration::from_secs(60), child.wait_with_output())
                .await
                .map_err(|_| openintent_adapters::AdapterError::Timeout {
                    seconds: 60,
                    reason: format!("script `{}` timed out", tool.script.filename),
                })?
                .map_err(|e| openintent_adapters::AdapterError::ExecutionFailed {
                    tool_name: tool.name.clone(),
                    reason: format!("script execution error: {e}"),
                })?;

        let stdout = String::from_utf8_lossy(&output.stdout);
        let stderr = String::from_utf8_lossy(&output.stderr);

        if output.status.success() {
            // Try to parse stdout as JSON, otherwise return as string.
            if let Ok(json_val) = serde_json::from_str::<Value>(stdout.trim()) {
                Ok(json_val)
            } else {
                Ok(json!({
                    "output": stdout.trim(),
                    "exit_code": 0,
                }))
            }
        } else {
            let code = output.status.code().unwrap_or(-1);
            Ok(json!({
                "error": true,
                "exit_code": code,
                "stdout": stdout.trim(),
                "stderr": stderr.trim(),
            }))
        }
    }
}

#[async_trait]
impl Adapter for SkillAdapter {
    fn id(&self) -> &str {
        &self.id
    }

    fn adapter_type(&self) -> AdapterType {
        AdapterType::Productivity
    }

    async fn connect(&mut self) -> Result<()> {
        self.connected = true;
        tracing::info!(
            id = %self.id,
            tools = self.script_tools.len(),
            "skill adapter connected"
        );
        Ok(())
    }

    async fn disconnect(&mut self) -> Result<()> {
        self.connected = false;
        tracing::info!(id = %self.id, "skill adapter disconnected");
        Ok(())
    }

    async fn health_check(&self) -> Result<HealthStatus> {
        if self.connected {
            Ok(HealthStatus::Healthy)
        } else {
            Ok(HealthStatus::Unhealthy)
        }
    }

    fn tools(&self) -> Vec<ToolDefinition> {
        self.script_tools
            .iter()
            .map(|t| ToolDefinition {
                name: t.name.clone(),
                description: t.description.clone(),
                parameters: json!({
                    "type": "object",
                    "properties": {
                        "args": {
                            "type": "string",
                            "description": "Arguments to pass to the script (JSON string or plain text)"
                        }
                    }
                }),
            })
            .collect()
    }

    async fn execute_tool(&self, name: &str, params: Value) -> Result<Value> {
        if !self.connected {
            return Err(openintent_adapters::AdapterError::ExecutionFailed {
                tool_name: name.to_owned(),
                reason: format!("skill adapter `{}` is not connected", self.id),
            });
        }

        let tool = self.script_tools.iter().find(|t| t.name == name).ok_or(
            openintent_adapters::AdapterError::ToolNotFound {
                adapter_id: self.id.clone(),
                tool_name: name.to_owned(),
            },
        )?;

        self.execute_script(tool, params).await
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
    use crate::types::{ScriptInterpreter, SkillMetadata, SkillSource};

    #[test]
    fn adapter_no_scripts() {
        let skills = vec![SkillDefinition {
            name: "prompt-only".into(),
            description: "A prompt-only skill.".into(),
            version: None,
            metadata: SkillMetadata::default(),
            instructions: "Do something.".into(),
            source: SkillSource::Builtin,
            scripts: Vec::new(),
        }];

        let adapter = SkillAdapter::new("skills", &skills);
        assert!(adapter.tools().is_empty());
    }

    #[test]
    fn adapter_with_scripts() {
        let skills = vec![SkillDefinition {
            name: "my-tool".into(),
            description: "A tool skill.".into(),
            version: None,
            metadata: SkillMetadata::default(),
            instructions: "Run the script.".into(),
            source: SkillSource::Builtin,
            scripts: vec![SkillScript {
                filename: "run.sh".into(),
                path: "/tmp/skills/my-tool/run.sh".into(),
                interpreter: ScriptInterpreter::Shell,
            }],
        }];

        let adapter = SkillAdapter::new("skills", &skills);
        let tools = adapter.tools();
        assert_eq!(tools.len(), 1);
        assert!(tools[0].name.contains("my_tool"));
    }
}
