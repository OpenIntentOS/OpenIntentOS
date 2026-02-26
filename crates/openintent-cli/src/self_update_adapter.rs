//! Self-update tool adapter.
//!
//! Exposes a `system_self_update` tool to the agent runtime so the LLM can
//! trigger a binary update in response to any user request — regardless of
//! language or phrasing.  No keyword matching is needed: the LLM reads the
//! tool description and decides when to call it.

use std::sync::{Arc, Mutex};

use async_trait::async_trait;
use openintent_agent::{AgentError, ToolAdapter, ToolDefinition};
use serde_json::{Value, json};

/// Shared signal: set to the new version string when the binary has been
/// replaced, so the bot loop can persist the notification and exit cleanly.
pub type RestartSignal = Arc<Mutex<Option<String>>>;

/// Tool adapter that exposes `system_self_update` to the LLM.
pub struct SelfUpdateAdapter {
    restart_signal: RestartSignal,
}

impl SelfUpdateAdapter {
    pub fn new(restart_signal: RestartSignal) -> Self {
        Self { restart_signal }
    }
}

#[async_trait]
impl ToolAdapter for SelfUpdateAdapter {
    fn adapter_id(&self) -> &str {
        "self_update"
    }

    fn tool_definitions(&self) -> Vec<ToolDefinition> {
        vec![ToolDefinition {
            name: "system_self_update".into(),
            description: "\
                Check for a newer release of OpenIntentOS and install it automatically. \
                The system restarts after updating. \
                Call this whenever the user asks to update, upgrade, or get the latest \
                version — in any language or phrasing.\
            "
            .into(),
            input_schema: json!({
                "type": "object",
                "properties": {},
                "required": []
            }),
        }]
    }

    async fn execute(&self, tool_name: &str, _arguments: Value) -> Result<String, AgentError> {
        if tool_name != "system_self_update" {
            return Err(AgentError::UnknownTool { tool_name: tool_name.into() });
        }

        match crate::update::check_and_apply_update().await {
            Ok(outcome) if outcome.updated => {
                // Signal the bot loop to persist state and restart after the
                // LLM has sent its reply to the user.
                *self.restart_signal.lock().unwrap() = Some(outcome.latest_version.clone());
                Ok(json!({
                    "status": "updated",
                    "from": outcome.current_version,
                    "to": outcome.latest_version,
                })
                .to_string())
            }
            Ok(outcome) => Ok(json!({
                "status": "up_to_date",
                "current": outcome.current_version,
            })
            .to_string()),
            Err(e) => Ok(json!({
                "status": "error",
                "message": format!("{e:#}"),
            })
            .to_string()),
        }
    }
}
