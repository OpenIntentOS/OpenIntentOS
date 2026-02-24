//! Plugin metadata and registry.
//!
//! This module defines the data model for loaded Wasm plugins and provides a
//! [`PluginRegistry`] that manages the full lifecycle -- loading, querying,
//! listing, and unloading plugins.

use serde::{Deserialize, Serialize};
use wasmtime::{Engine, Module};

use crate::error::{Result, SandboxError};

/// Metadata describing a loaded plugin.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PluginInfo {
    /// Unique name of the plugin (used as lookup key).
    pub name: String,
    /// Semantic version string (e.g. `"0.1.0"`).
    pub version: String,
    /// Human-readable description of what the plugin does.
    pub description: String,
    /// Tools exposed by this plugin.
    pub tools: Vec<PluginTool>,
}

/// A single tool exposed by a plugin.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PluginTool {
    /// Tool name, unique within the plugin.
    pub name: String,
    /// Human-readable description of the tool.
    pub description: String,
    /// JSON Schema describing the expected input parameters.
    pub parameters_schema: serde_json::Value,
}

/// A compiled Wasm module together with its metadata.
pub(crate) struct LoadedPlugin {
    pub info: PluginInfo,
    pub module: Module,
}

/// Registry of loaded Wasm plugins.
///
/// Plugins are stored in insertion order. Lookup by name is O(n) which is
/// acceptable for the expected number of loaded plugins (tens, not thousands).
pub struct PluginRegistry {
    plugins: Vec<LoadedPlugin>,
}

impl PluginRegistry {
    /// Create an empty registry.
    pub fn new() -> Self {
        Self {
            plugins: Vec::new(),
        }
    }

    /// Compile and register a new plugin from raw `.wasm` bytes.
    ///
    /// If a plugin with the same `name` already exists the call will fail
    /// rather than silently overwrite it.
    pub fn load_plugin(
        &mut self,
        name: &str,
        wasm_bytes: &[u8],
        engine: &Engine,
    ) -> Result<&PluginInfo> {
        // Reject duplicates.
        if self.plugins.iter().any(|p| p.info.name == name) {
            return Err(SandboxError::Plugin {
                reason: format!("plugin '{name}' is already loaded"),
            });
        }

        // Compile the module.
        let module = Module::new(engine, wasm_bytes)
            .map_err(|e| SandboxError::Compilation(e.to_string()))?;

        tracing::info!(plugin = name, "compiled wasm module");

        // Build minimal metadata. A real implementation would call the
        // module's `get_plugin_info` export to obtain richer metadata; for now
        // we construct a placeholder that is sufficient for the registry API.
        let info = PluginInfo {
            name: name.to_owned(),
            version: "0.0.0".to_owned(),
            description: String::new(),
            tools: Vec::new(),
        };

        self.plugins.push(LoadedPlugin { info, module });

        // We just pushed, so last() is guaranteed to be Some.
        Ok(&self.plugins[self.plugins.len() - 1].info)
    }

    /// Return a slice of references to all loaded plugin metadata.
    pub fn list_plugins(&self) -> Vec<&PluginInfo> {
        self.plugins.iter().map(|p| &p.info).collect()
    }

    /// Look up a plugin by name.
    pub fn get_plugin(&self, name: &str) -> Option<&PluginInfo> {
        self.plugins
            .iter()
            .find(|p| p.info.name == name)
            .map(|p| &p.info)
    }

    /// Look up a loaded plugin (including the compiled module) by name.
    pub(crate) fn get_loaded(&self, name: &str) -> Option<&LoadedPlugin> {
        self.plugins.iter().find(|p| p.info.name == name)
    }

    /// Remove a plugin from the registry.
    ///
    /// Returns an error if the plugin is not found.
    pub fn unload_plugin(&mut self, name: &str) -> Result<()> {
        let idx = self
            .plugins
            .iter()
            .position(|p| p.info.name == name)
            .ok_or_else(|| SandboxError::Plugin {
                reason: format!("plugin '{name}' not found"),
            })?;
        self.plugins.remove(idx);
        tracing::info!(plugin = name, "unloaded plugin");
        Ok(())
    }

    /// Returns the number of loaded plugins.
    pub fn len(&self) -> usize {
        self.plugins.len()
    }

    /// Returns `true` if no plugins are loaded.
    pub fn is_empty(&self) -> bool {
        self.plugins.is_empty()
    }
}

impl Default for PluginRegistry {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // Minimal valid Wasm module (empty module: magic + version + no sections).
    // (module)  =>  \0asm\x01\x00\x00\x00
    fn minimal_wasm() -> Vec<u8> {
        vec![0x00, 0x61, 0x73, 0x6D, 0x01, 0x00, 0x00, 0x00]
    }

    fn make_engine() -> Engine {
        let mut cfg = wasmtime::Config::new();
        cfg.consume_fuel(true);
        Engine::new(&cfg).expect("engine creation must succeed in tests")
    }

    #[test]
    fn new_registry_is_empty() {
        let reg = PluginRegistry::new();
        assert!(reg.is_empty());
        assert_eq!(reg.len(), 0);
        assert!(reg.list_plugins().is_empty());
    }

    #[test]
    fn load_and_get_plugin() {
        let engine = make_engine();
        let mut reg = PluginRegistry::new();
        let info = reg.load_plugin("hello", &minimal_wasm(), &engine);
        assert!(info.is_ok());
        let info = info.unwrap();
        assert_eq!(info.name, "hello");

        assert_eq!(reg.len(), 1);
        assert!(!reg.is_empty());

        let found = reg.get_plugin("hello");
        assert!(found.is_some());
        assert_eq!(found.unwrap().name, "hello");
    }

    #[test]
    fn list_plugins_returns_all() {
        let engine = make_engine();
        let mut reg = PluginRegistry::new();
        reg.load_plugin("a", &minimal_wasm(), &engine).unwrap();
        reg.load_plugin("b", &minimal_wasm(), &engine).unwrap();
        let names: Vec<&str> = reg.list_plugins().iter().map(|p| p.name.as_str()).collect();
        assert_eq!(names, vec!["a", "b"]);
    }

    #[test]
    fn get_nonexistent_returns_none() {
        let reg = PluginRegistry::new();
        assert!(reg.get_plugin("ghost").is_none());
    }

    #[test]
    fn unload_plugin_removes_it() {
        let engine = make_engine();
        let mut reg = PluginRegistry::new();
        reg.load_plugin("rm-me", &minimal_wasm(), &engine).unwrap();
        assert_eq!(reg.len(), 1);

        reg.unload_plugin("rm-me").unwrap();
        assert!(reg.is_empty());
        assert!(reg.get_plugin("rm-me").is_none());
    }

    #[test]
    fn unload_nonexistent_returns_error() {
        let mut reg = PluginRegistry::new();
        let result = reg.unload_plugin("nope");
        assert!(result.is_err());
    }

    #[test]
    fn duplicate_load_returns_error() {
        let engine = make_engine();
        let mut reg = PluginRegistry::new();
        reg.load_plugin("dup", &minimal_wasm(), &engine).unwrap();
        let result = reg.load_plugin("dup", &minimal_wasm(), &engine);
        assert!(result.is_err());
    }

    #[test]
    fn load_invalid_wasm_returns_error() {
        let engine = make_engine();
        let mut reg = PluginRegistry::new();
        let result = reg.load_plugin("bad", b"not wasm at all", &engine);
        assert!(result.is_err());
        match result.unwrap_err() {
            SandboxError::Compilation(msg) => {
                assert!(!msg.is_empty());
            }
            other => panic!("expected Compilation error, got: {other}"),
        }
    }

    #[test]
    fn plugin_info_fields() {
        let info = PluginInfo {
            name: "test".into(),
            version: "1.2.3".into(),
            description: "A test plugin".into(),
            tools: vec![PluginTool {
                name: "do_thing".into(),
                description: "Does a thing".into(),
                parameters_schema: serde_json::json!({"type": "object"}),
            }],
        };
        assert_eq!(info.name, "test");
        assert_eq!(info.tools.len(), 1);
        assert_eq!(info.tools[0].name, "do_thing");
    }

    #[test]
    fn plugin_info_serialization_roundtrip() {
        let info = PluginInfo {
            name: "roundtrip".into(),
            version: "0.1.0".into(),
            description: "test".into(),
            tools: vec![],
        };
        let json = serde_json::to_string(&info).unwrap();
        let back: PluginInfo = serde_json::from_str(&json).unwrap();
        assert_eq!(back.name, "roundtrip");
    }

    #[test]
    fn default_registry_is_empty() {
        let reg = PluginRegistry::default();
        assert!(reg.is_empty());
    }
}
