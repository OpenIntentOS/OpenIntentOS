//! Plugin adapter bridge.
//!
//! [`PluginAdapter`] implements the [`Adapter`](openintent_adapters::Adapter)
//! trait for a single loaded WASM plugin, forwarding tool discovery and
//! execution to the underlying [`SandboxRuntime`].

use std::sync::Arc;

use async_trait::async_trait;
use serde_json::Value;
use tokio::sync::Mutex;

use openintent_adapters::error::{AdapterError, Result};
use openintent_adapters::traits::{
    Adapter, AdapterType, AuthRequirement, HealthStatus, ToolDefinition,
};

use crate::plugin::PluginInfo;
use crate::runtime::SandboxRuntime;

/// Bridges a single WASM plugin to the adapter system.
///
/// Each loaded WASM plugin becomes an adapter that exposes its tools to the
/// agent.  The agent calls tools through the normal adapter interface, and the
/// `PluginAdapter` forwards them to the [`SandboxRuntime`].
///
/// The runtime is shared via `Arc<Mutex<_>>` so that multiple plugin adapters
/// (each representing a different plugin) can coexist and share the same
/// underlying sandbox engine.
pub struct PluginAdapter {
    /// Adapter identifier, prefixed with `wasm:` (e.g. `"wasm:notion"`).
    id: String,
    /// Name of the plugin as registered in the sandbox runtime.
    plugin_name: String,
    /// Cached snapshot of the plugin's metadata at load time.
    plugin_info: PluginInfo,
    /// Shared handle to the sandbox runtime.
    runtime: Arc<Mutex<SandboxRuntime>>,
}

impl PluginAdapter {
    /// Create a new plugin adapter.
    ///
    /// `plugin_info` is the metadata snapshot captured when the plugin was
    /// loaded.  `runtime` is the shared sandbox runtime that owns the compiled
    /// WASM module.
    pub fn new(plugin_info: PluginInfo, runtime: Arc<Mutex<SandboxRuntime>>) -> Self {
        let id = format!("wasm:{}", plugin_info.name);
        let plugin_name = plugin_info.name.clone();
        Self {
            id,
            plugin_name,
            plugin_info,
            runtime,
        }
    }

    /// Return the plugin name (without the `wasm:` prefix).
    pub fn plugin_name(&self) -> &str {
        &self.plugin_name
    }

    /// Return a reference to the cached plugin metadata.
    pub fn plugin_info(&self) -> &PluginInfo {
        &self.plugin_info
    }

    /// Return a clone of the shared runtime handle.
    pub fn runtime(&self) -> Arc<Mutex<SandboxRuntime>> {
        Arc::clone(&self.runtime)
    }
}

#[async_trait]
impl Adapter for PluginAdapter {
    fn id(&self) -> &str {
        &self.id
    }

    fn adapter_type(&self) -> AdapterType {
        AdapterType::Productivity
    }

    /// No-op -- plugins are loaded separately via the [`PluginLoader`](crate::loader::PluginLoader).
    async fn connect(&mut self) -> Result<()> {
        tracing::debug!(adapter = %self.id, "connect called (no-op for wasm plugins)");
        Ok(())
    }

    /// No-op -- plugin unloading is handled through the loader.
    async fn disconnect(&mut self) -> Result<()> {
        tracing::debug!(adapter = %self.id, "disconnect called (no-op for wasm plugins)");
        Ok(())
    }

    /// Returns [`HealthStatus::Healthy`] if the plugin is still registered in
    /// the runtime, [`HealthStatus::Unhealthy`] otherwise.
    async fn health_check(&self) -> Result<HealthStatus> {
        let rt = self.runtime.lock().await;
        let plugins = rt.list_plugins();
        let loaded = plugins.iter().any(|p| p.name == self.plugin_name);
        if loaded {
            Ok(HealthStatus::Healthy)
        } else {
            Ok(HealthStatus::Unhealthy)
        }
    }

    /// Return tool definitions converted from the plugin's [`PluginTool`](crate::plugin::PluginTool) metadata.
    fn tools(&self) -> Vec<ToolDefinition> {
        self.plugin_info
            .tools
            .iter()
            .map(|t| ToolDefinition {
                name: t.name.clone(),
                description: t.description.clone(),
                parameters: t.parameters_schema.clone(),
            })
            .collect()
    }

    /// Forward tool execution to the sandbox runtime.
    ///
    /// Because [`SandboxRuntime::execute_tool`] is a synchronous, potentially
    /// CPU-heavy call, we run it on a blocking thread via
    /// [`tokio::task::spawn_blocking`] to avoid starving the tokio runtime.
    async fn execute_tool(&self, name: &str, params: Value) -> Result<Value> {
        let rt = Arc::clone(&self.runtime);
        let plugin_name = self.plugin_name.clone();
        let tool_name = name.to_owned();
        let adapter_id = self.id.clone();

        tracing::debug!(
            adapter = %adapter_id,
            plugin = %plugin_name,
            tool = %tool_name,
            "executing tool via wasm sandbox"
        );

        let result = tokio::task::spawn_blocking(move || {
            // We must block on acquiring the mutex inside the blocking thread
            // because spawn_blocking does not run inside a tokio async context
            // by default, but tokio::sync::Mutex requires an async .lock().
            // Instead, use a small dedicated runtime to lock the mutex.
            let handle = tokio::runtime::Handle::current();
            let rt_guard = handle.block_on(rt.lock());
            rt_guard.execute_tool(&plugin_name, &tool_name, params)
        })
        .await
        .map_err(|e| AdapterError::ExecutionFailed {
            tool_name: name.to_owned(),
            reason: format!("blocking task panicked: {e}"),
        })?;

        result.map_err(|sandbox_err| AdapterError::ExecutionFailed {
            tool_name: name.to_owned(),
            reason: sandbox_err.to_string(),
        })
    }

    /// WASM plugins do not require external authentication.
    fn required_auth(&self) -> Option<AuthRequirement> {
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Minimal valid Wasm module (magic + version, no sections).
    fn minimal_wasm() -> Vec<u8> {
        vec![0x00, 0x61, 0x73, 0x6D, 0x01, 0x00, 0x00, 0x00]
    }

    fn make_runtime() -> Arc<Mutex<SandboxRuntime>> {
        let rt = SandboxRuntime::with_defaults().expect("runtime creation must succeed in tests");
        Arc::new(Mutex::new(rt))
    }

    fn make_plugin_info(name: &str) -> PluginInfo {
        PluginInfo {
            name: name.to_owned(),
            version: "0.1.0".to_owned(),
            description: "Test plugin".to_owned(),
            tools: vec![crate::plugin::PluginTool {
                name: "do_thing".to_owned(),
                description: "Does a thing".to_owned(),
                parameters_schema: serde_json::json!({"type": "object"}),
            }],
        }
    }

    #[test]
    fn adapter_id_is_prefixed() {
        let rt = make_runtime();
        let info = make_plugin_info("notion");
        let adapter = PluginAdapter::new(info, rt);
        assert_eq!(adapter.id(), "wasm:notion");
    }

    #[test]
    fn adapter_type_is_productivity() {
        let rt = make_runtime();
        let info = make_plugin_info("test");
        let adapter = PluginAdapter::new(info, rt);
        assert_eq!(adapter.adapter_type(), AdapterType::Productivity);
    }

    #[test]
    fn tools_converts_from_plugin_info() {
        let rt = make_runtime();
        let info = make_plugin_info("test");
        let adapter = PluginAdapter::new(info, rt);
        let tools = adapter.tools();
        assert_eq!(tools.len(), 1);
        assert_eq!(tools[0].name, "do_thing");
        assert_eq!(tools[0].description, "Does a thing");
        assert_eq!(tools[0].parameters, serde_json::json!({"type": "object"}));
    }

    #[test]
    fn required_auth_is_none() {
        let rt = make_runtime();
        let info = make_plugin_info("test");
        let adapter = PluginAdapter::new(info, rt);
        assert!(adapter.required_auth().is_none());
    }

    #[test]
    fn plugin_name_accessor() {
        let rt = make_runtime();
        let info = make_plugin_info("calendar");
        let adapter = PluginAdapter::new(info, rt);
        assert_eq!(adapter.plugin_name(), "calendar");
    }

    #[tokio::test]
    async fn connect_is_noop() {
        let rt = make_runtime();
        let info = make_plugin_info("test");
        let mut adapter = PluginAdapter::new(info, rt);
        assert!(adapter.connect().await.is_ok());
    }

    #[tokio::test]
    async fn disconnect_is_noop() {
        let rt = make_runtime();
        let info = make_plugin_info("test");
        let mut adapter = PluginAdapter::new(info, rt);
        assert!(adapter.disconnect().await.is_ok());
    }

    #[tokio::test]
    async fn health_check_healthy_when_loaded() {
        let rt = make_runtime();

        // Load a plugin so it is registered in the runtime.
        {
            let mut guard = rt.lock().await;
            guard
                .load_plugin("checker", &minimal_wasm())
                .expect("load must succeed in tests");
        }

        let info = make_plugin_info("checker");
        let adapter = PluginAdapter::new(info, rt);
        let status = adapter
            .health_check()
            .await
            .expect("health_check must succeed");
        assert_eq!(status, HealthStatus::Healthy);
    }

    #[tokio::test]
    async fn health_check_unhealthy_when_not_loaded() {
        let rt = make_runtime();
        // Do NOT load the plugin -- the runtime has no "ghost" plugin.
        let info = make_plugin_info("ghost");
        let adapter = PluginAdapter::new(info, rt);
        let status = adapter
            .health_check()
            .await
            .expect("health_check must succeed");
        assert_eq!(status, HealthStatus::Unhealthy);
    }

    #[tokio::test]
    async fn execute_tool_returns_error_for_missing_plugin() {
        let rt = make_runtime();
        let info = make_plugin_info("missing");
        let adapter = PluginAdapter::new(info, rt);
        let result = adapter
            .execute_tool("do_thing", serde_json::json!({}))
            .await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn execute_tool_returns_error_for_minimal_module() {
        let rt = make_runtime();

        // The minimal wasm module has no `execute_tool` export, so execution
        // should fail gracefully.
        {
            let mut guard = rt.lock().await;
            guard
                .load_plugin("empty", &minimal_wasm())
                .expect("load must succeed in tests");
        }

        let info = make_plugin_info("empty");
        let adapter = PluginAdapter::new(info, rt);
        let result = adapter
            .execute_tool("anything", serde_json::json!({}))
            .await;
        assert!(result.is_err());
    }

    #[test]
    fn empty_tools_when_plugin_has_none() {
        let rt = make_runtime();
        let info = PluginInfo {
            name: "bare".to_owned(),
            version: "0.0.0".to_owned(),
            description: String::new(),
            tools: Vec::new(),
        };
        let adapter = PluginAdapter::new(info, rt);
        assert!(adapter.tools().is_empty());
    }

    #[test]
    fn multiple_tools_converted_correctly() {
        let rt = make_runtime();
        let info = PluginInfo {
            name: "multi".to_owned(),
            version: "1.0.0".to_owned(),
            description: "Multi-tool plugin".to_owned(),
            tools: vec![
                crate::plugin::PluginTool {
                    name: "tool_a".to_owned(),
                    description: "First tool".to_owned(),
                    parameters_schema: serde_json::json!({"type": "string"}),
                },
                crate::plugin::PluginTool {
                    name: "tool_b".to_owned(),
                    description: "Second tool".to_owned(),
                    parameters_schema: serde_json::json!({"type": "number"}),
                },
            ],
        };
        let adapter = PluginAdapter::new(info, rt);
        let tools = adapter.tools();
        assert_eq!(tools.len(), 2);
        assert_eq!(tools[0].name, "tool_a");
        assert_eq!(tools[1].name, "tool_b");
    }
}
