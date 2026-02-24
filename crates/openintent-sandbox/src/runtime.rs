//! Sandbox runtime.
//!
//! [`SandboxRuntime`] is the main entry point for loading and executing Wasm
//! plugins.  It owns the wasmtime [`Engine`], the [`SandboxConfig`] resource
//! limits, and the [`PluginRegistry`].

use wasmtime::{AsContextMut, Engine, Instance, Linker, Store};

use crate::config::SandboxConfig;
use crate::error::{Result, SandboxError};
use crate::plugin::{PluginInfo, PluginRegistry};

/// Per-call state stored in the wasmtime [`Store`].
///
/// This is the "host state" that wasmtime associates with every store
/// instance.  It carries data needed by host functions during a single
/// invocation.
struct HostState {
    /// JSON-encoded input parameters for the current tool invocation.
    input_json: Vec<u8>,
    /// Buffer where the guest writes its JSON result.
    output_json: Vec<u8>,
}

/// The WebAssembly plugin sandbox runtime.
///
/// Wraps wasmtime and exposes a high-level API for loading plugins, invoking
/// their tools, and enforcing resource limits.
pub struct SandboxRuntime {
    engine: Engine,
    config: SandboxConfig,
    registry: PluginRegistry,
}

impl SandboxRuntime {
    /// Create a new sandbox runtime with the given configuration.
    pub fn new(config: SandboxConfig) -> Result<Self> {
        let mut wasm_config = wasmtime::Config::new();
        wasm_config.consume_fuel(true);
        wasm_config.wasm_memory64(false);

        let engine = Engine::new(&wasm_config)
            .map_err(|e| SandboxError::Compilation(format!("failed to create wasm engine: {e}")))?;

        tracing::info!("sandbox runtime initialized");

        Ok(Self {
            engine,
            config,
            registry: PluginRegistry::new(),
        })
    }

    /// Create a runtime with default configuration.
    pub fn with_defaults() -> Result<Self> {
        Self::new(SandboxConfig::default())
    }

    /// Return a reference to the wasmtime [`Engine`].
    pub fn engine(&self) -> &Engine {
        &self.engine
    }

    /// Return a reference to the current [`SandboxConfig`].
    pub fn config(&self) -> &SandboxConfig {
        &self.config
    }

    /// Load a Wasm plugin from raw bytes.
    pub fn load_plugin(&mut self, name: &str, wasm_bytes: &[u8]) -> Result<&PluginInfo> {
        self.registry.load_plugin(name, wasm_bytes, &self.engine)
    }

    /// Execute a named tool from a loaded plugin.
    ///
    /// The high-level flow:
    /// 1. Locate the plugin in the registry.
    /// 2. Create a fresh [`Store`] with fuel and state.
    /// 3. Build a [`Linker`] and define host functions.
    /// 4. Instantiate the module.
    /// 5. Call the guest `execute_tool` export.
    /// 6. Read back the JSON result from guest memory.
    pub fn execute_tool(
        &self,
        plugin_name: &str,
        tool_name: &str,
        params: serde_json::Value,
    ) -> Result<serde_json::Value> {
        // 1. Find the plugin.
        let loaded = self
            .registry
            .get_loaded(plugin_name)
            .ok_or_else(|| SandboxError::Plugin {
                reason: format!("plugin '{plugin_name}' not found"),
            })?;

        let input_json =
            serde_json::to_vec(&params).map_err(|e| SandboxError::Execution(e.to_string()))?;

        // 2. Create a store with fuel.
        let host_state = HostState {
            input_json,
            output_json: Vec::new(),
        };
        let mut store = Store::new(&self.engine, host_state);
        store
            .set_fuel(self.config.max_fuel)
            .map_err(|e| SandboxError::Execution(e.to_string()))?;

        // 3. Build linker with host functions.
        let mut linker: Linker<HostState> = Linker::new(&self.engine);
        Self::define_host_functions(&mut linker)?;

        // 4. Instantiate the module.
        let instance = linker
            .instantiate(&mut store, &loaded.module)
            .map_err(|e| SandboxError::Instantiation(e.to_string()))?;

        // 5. Write tool_name and params into guest memory, then call
        //    `execute_tool`.
        let result_code = Self::call_execute_tool(&mut store, &instance, tool_name)?;

        if result_code != 0 {
            return Err(SandboxError::Execution(format!(
                "execute_tool returned non-zero code: {result_code}"
            )));
        }

        // 6. Read result from host state.
        let output = &store.data().output_json;
        if output.is_empty() {
            return Ok(serde_json::Value::Null);
        }
        serde_json::from_slice(output)
            .map_err(|e| SandboxError::Execution(format!("invalid result JSON: {e}")))
    }

    /// Define the host functions that Wasm plugins can call.
    fn define_host_functions(linker: &mut Linker<HostState>) -> Result<()> {
        // host_log: let plugins emit tracing events.
        linker
            .func_wrap(
                "env",
                "host_log",
                |mut caller: wasmtime::Caller<'_, HostState>,
                 level: i32,
                 msg_ptr: i32,
                 msg_len: i32| {
                    let memory = match caller.get_export("memory") {
                        Some(wasmtime::Extern::Memory(m)) => m,
                        _ => return,
                    };
                    let data = memory.data(&caller);
                    let start = msg_ptr as usize;
                    let end = start + msg_len as usize;
                    if end > data.len() {
                        return;
                    }
                    if let Ok(msg) = std::str::from_utf8(&data[start..end]) {
                        match level {
                            0 => tracing::error!(plugin_msg = msg),
                            1 => tracing::warn!(plugin_msg = msg),
                            2 => tracing::info!(plugin_msg = msg),
                            3 => tracing::debug!(plugin_msg = msg),
                            _ => tracing::trace!(plugin_msg = msg),
                        }
                    }
                },
            )
            .map_err(|e| SandboxError::Instantiation(e.to_string()))?;

        // host_set_result: plugin writes its JSON result.
        linker
            .func_wrap(
                "env",
                "host_set_result",
                |mut caller: wasmtime::Caller<'_, HostState>, ptr: i32, len: i32| {
                    let memory = match caller.get_export("memory") {
                        Some(wasmtime::Extern::Memory(m)) => m,
                        _ => return,
                    };
                    let data = memory.data(&caller);
                    let start = ptr as usize;
                    let end = start + len as usize;
                    if end > data.len() {
                        return;
                    }
                    let bytes = data[start..end].to_vec();
                    caller.data_mut().output_json = bytes;
                },
            )
            .map_err(|e| SandboxError::Instantiation(e.to_string()))?;

        // host_get_param: plugin reads a parameter by key.
        linker
            .func_wrap(
                "env",
                "host_get_param",
                |mut caller: wasmtime::Caller<'_, HostState>,
                 _key_ptr: i32,
                 _key_len: i32,
                 val_ptr: i32,
                 val_len: i32|
                 -> i32 {
                    // For simplicity, write the full input JSON into the
                    // provided buffer (up to val_len bytes).
                    let input = caller.data().input_json.clone();
                    let memory = match caller.get_export("memory") {
                        Some(wasmtime::Extern::Memory(m)) => m,
                        _ => return -1,
                    };
                    let write_len = std::cmp::min(input.len(), val_len as usize);
                    let dest_start = val_ptr as usize;
                    let dest_end = dest_start + write_len;
                    let mem_data = memory.data_mut(&mut caller);
                    if dest_end > mem_data.len() {
                        return -1;
                    }
                    mem_data[dest_start..dest_end].copy_from_slice(&input[..write_len]);
                    write_len as i32
                },
            )
            .map_err(|e| SandboxError::Instantiation(e.to_string()))?;

        Ok(())
    }

    /// Call the guest's `execute_tool` export.
    ///
    /// Expected signature:
    /// `execute_tool(tool_name_ptr: i32, tool_name_len: i32, params_ptr: i32, params_len: i32) -> i32`
    fn call_execute_tool(
        store: &mut Store<HostState>,
        instance: &Instance,
        tool_name: &str,
    ) -> Result<i32> {
        // Get guest memory.
        let memory = instance
            .get_memory(store.as_context_mut(), "memory")
            .ok_or_else(|| SandboxError::Execution("module has no exported memory".into()))?;

        // Write tool_name bytes into guest memory at a fixed offset.
        let tool_bytes = tool_name.as_bytes();
        let tool_name_ptr: usize = 0;
        let tool_name_len = tool_bytes.len();

        let params_bytes = &store.data().input_json.clone();
        let params_ptr = tool_name_ptr + tool_name_len;
        let params_len = params_bytes.len();

        let total_needed = params_ptr + params_len;
        let mem_data = memory.data_mut(store.as_context_mut());
        if total_needed > mem_data.len() {
            return Err(SandboxError::MemoryLimit {
                used: total_needed,
                limit: mem_data.len(),
            });
        }
        mem_data[tool_name_ptr..tool_name_ptr + tool_name_len].copy_from_slice(tool_bytes);
        mem_data[params_ptr..params_ptr + params_len].copy_from_slice(params_bytes);

        // Call the export.
        let execute_fn = instance
            .get_typed_func::<(i32, i32, i32, i32), i32>(store.as_context_mut(), "execute_tool")
            .map_err(|e| SandboxError::Execution(format!("missing execute_tool export: {e}")))?;

        let result = execute_fn
            .call(
                store,
                (
                    tool_name_ptr as i32,
                    tool_name_len as i32,
                    params_ptr as i32,
                    params_len as i32,
                ),
            )
            .map_err(|e| SandboxError::Trap(e.to_string()))?;

        Ok(result)
    }

    /// List all loaded plugins.
    pub fn list_plugins(&self) -> Vec<&PluginInfo> {
        self.registry.list_plugins()
    }

    /// Unload a plugin by name.
    pub fn unload_plugin(&mut self, name: &str) -> Result<()> {
        self.registry.unload_plugin(name)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn minimal_wasm() -> Vec<u8> {
        vec![0x00, 0x61, 0x73, 0x6D, 0x01, 0x00, 0x00, 0x00]
    }

    #[test]
    fn create_runtime_with_defaults() {
        let rt = SandboxRuntime::with_defaults();
        assert!(rt.is_ok());
        let rt = rt.unwrap();
        assert_eq!(rt.config().max_fuel, 1_000_000);
    }

    #[test]
    fn create_runtime_with_custom_config() {
        let cfg = SandboxConfig::new().with_max_fuel(42);
        let rt = SandboxRuntime::new(cfg);
        assert!(rt.is_ok());
        assert_eq!(rt.unwrap().config().max_fuel, 42);
    }

    #[test]
    fn load_plugin_via_runtime() {
        let mut rt = SandboxRuntime::with_defaults().unwrap();
        let result = rt.load_plugin("test", &minimal_wasm());
        assert!(result.is_ok());
        assert_eq!(result.unwrap().name, "test");
    }

    #[test]
    fn list_plugins_via_runtime() {
        let mut rt = SandboxRuntime::with_defaults().unwrap();
        assert!(rt.list_plugins().is_empty());
        rt.load_plugin("a", &minimal_wasm()).unwrap();
        rt.load_plugin("b", &minimal_wasm()).unwrap();
        assert_eq!(rt.list_plugins().len(), 2);
    }

    #[test]
    fn unload_plugin_via_runtime() {
        let mut rt = SandboxRuntime::with_defaults().unwrap();
        rt.load_plugin("bye", &minimal_wasm()).unwrap();
        assert!(rt.unload_plugin("bye").is_ok());
        assert!(rt.list_plugins().is_empty());
    }

    #[test]
    fn execute_tool_missing_plugin_returns_error() {
        let rt = SandboxRuntime::with_defaults().unwrap();
        let result = rt.execute_tool("nonexistent", "tool", serde_json::json!({}));
        assert!(result.is_err());
    }

    #[test]
    fn execute_tool_minimal_module_fails_gracefully() {
        // The minimal wasm module has no exports, so execute_tool should
        // return an instantiation or execution error (not panic).
        let mut rt = SandboxRuntime::with_defaults().unwrap();
        rt.load_plugin("empty", &minimal_wasm()).unwrap();
        let result = rt.execute_tool("empty", "anything", serde_json::json!({}));
        assert!(result.is_err());
    }

    #[test]
    fn load_invalid_wasm_via_runtime() {
        let mut rt = SandboxRuntime::with_defaults().unwrap();
        let result = rt.load_plugin("bad", b"garbage bytes");
        assert!(result.is_err());
    }

    #[test]
    fn engine_is_accessible() {
        let rt = SandboxRuntime::with_defaults().unwrap();
        // Just verify we can access the engine without panicking.
        let _engine = rt.engine();
    }
}
