//! Adapter bridge: converts [`openintent_adapters::Adapter`] to
//! [`openintent_agent::runtime::ToolAdapter`].
//!
//! The two traits have slightly different signatures (different field names,
//! `Value` vs `String` return types), so this struct handles the conversion.

use std::sync::Arc;

use async_trait::async_trait;
use serde_json::Value;

use openintent_agent::runtime::ToolAdapter;

/// Bridges an adapter-crate `Adapter` to the agent-crate `ToolAdapter`.
pub struct AdapterBridge {
    adapter: Arc<dyn openintent_adapters::Adapter>,
}

impl AdapterBridge {
    pub fn new(adapter: impl openintent_adapters::Adapter + 'static) -> Self {
        Self {
            adapter: Arc::new(adapter),
        }
    }

    /// Convert an adapter-side `ToolDefinition` to an agent-side `ToolDefinition`.
    fn convert_tool_def(
        td: &openintent_adapters::ToolDefinition,
    ) -> openintent_agent::ToolDefinition {
        openintent_agent::ToolDefinition {
            name: td.name.clone(),
            description: td.description.clone(),
            input_schema: td.parameters.clone(),
        }
    }
}

#[async_trait]
impl ToolAdapter for AdapterBridge {
    fn adapter_id(&self) -> &str {
        self.adapter.id()
    }

    fn tool_definitions(&self) -> Vec<openintent_agent::ToolDefinition> {
        self.adapter
            .tools()
            .iter()
            .map(Self::convert_tool_def)
            .collect()
    }

    async fn execute(&self, tool_name: &str, arguments: Value) -> openintent_agent::Result<String> {
        let result = self
            .adapter
            .execute_tool(tool_name, arguments)
            .await
            .map_err(|e| openintent_agent::AgentError::ToolExecutionFailed {
                tool_name: tool_name.to_owned(),
                reason: e.to_string(),
            })?;

        let text = serde_json::to_string_pretty(&result).unwrap_or_else(|_| result.to_string());
        Ok(text)
    }
}
