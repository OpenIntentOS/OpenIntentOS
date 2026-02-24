//! Plugin loader.
//!
//! [`PluginLoader`] discovers and loads WASM plugins from a directory on disk,
//! returning [`PluginAdapter`] instances that integrate with the adapter system.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use tokio::sync::Mutex;

use crate::adapter::PluginAdapter;
use crate::config::SandboxConfig;
use crate::error::{Result, SandboxError};
use crate::runtime::SandboxRuntime;

/// Discovers and loads WASM plugins from a directory.
///
/// Scans the given directory for `.wasm` files, loads each into the sandbox
/// runtime, and returns [`PluginAdapter`] instances for them. The runtime is
/// shared across all adapters produced by this loader.
pub struct PluginLoader {
    /// Shared sandbox runtime.
    runtime: Arc<Mutex<SandboxRuntime>>,
    /// Directory to scan for `.wasm` plugin files.
    plugins_dir: PathBuf,
    /// Names of plugins that have been loaded through this loader.
    loaded_names: Vec<String>,
}

impl PluginLoader {
    /// Create a new plugin loader.
    ///
    /// Initializes a fresh [`SandboxRuntime`] with the given configuration and
    /// sets the plugin directory to scan.
    pub fn new(plugins_dir: PathBuf, config: SandboxConfig) -> Result<Self> {
        let runtime = SandboxRuntime::new(config)?;
        tracing::info!(plugins_dir = %plugins_dir.display(), "plugin loader created");
        Ok(Self {
            runtime: Arc::new(Mutex::new(runtime)),
            plugins_dir,
            loaded_names: Vec::new(),
        })
    }

    /// Create a plugin loader with an existing shared runtime.
    ///
    /// Useful when multiple loaders or other components need to share the same
    /// sandbox runtime.
    pub fn with_runtime(plugins_dir: PathBuf, runtime: Arc<Mutex<SandboxRuntime>>) -> Self {
        Self {
            runtime,
            plugins_dir,
            loaded_names: Vec::new(),
        }
    }

    /// Return a clone of the shared runtime handle.
    pub fn runtime(&self) -> Arc<Mutex<SandboxRuntime>> {
        Arc::clone(&self.runtime)
    }

    /// Return the configured plugins directory.
    pub fn plugins_dir(&self) -> &Path {
        &self.plugins_dir
    }

    /// Scan `plugins_dir` for `.wasm` files and load each one.
    ///
    /// Returns a [`PluginAdapter`] for every successfully loaded plugin.
    /// Plugins that fail to load are logged and skipped rather than aborting
    /// the entire batch.
    pub async fn load_all(&mut self) -> Result<Vec<PluginAdapter>> {
        let dir = &self.plugins_dir;

        if !dir.exists() {
            tracing::warn!(path = %dir.display(), "plugins directory does not exist");
            return Ok(Vec::new());
        }

        if !dir.is_dir() {
            return Err(SandboxError::Io(std::io::Error::new(
                std::io::ErrorKind::NotADirectory,
                format!("{} is not a directory", dir.display()),
            )));
        }

        let mut entries: Vec<PathBuf> = Vec::new();

        let mut read_dir = tokio::fs::read_dir(dir).await.map_err(SandboxError::Io)?;

        while let Some(entry) = read_dir.next_entry().await.map_err(SandboxError::Io)? {
            let path = entry.path();
            if path.extension().and_then(|e| e.to_str()) == Some("wasm") {
                entries.push(path);
            }
        }

        // Sort for deterministic load order.
        entries.sort();

        tracing::info!(
            plugins_dir = %dir.display(),
            count = entries.len(),
            "discovered wasm plugin files"
        );

        let mut adapters = Vec::with_capacity(entries.len());

        for path in &entries {
            match self.load_plugin(path).await {
                Ok(adapter) => {
                    tracing::info!(plugin = %adapter.plugin_name(), path = %path.display(), "loaded plugin");
                    adapters.push(adapter);
                }
                Err(err) => {
                    tracing::warn!(
                        path = %path.display(),
                        error = %err,
                        "failed to load plugin, skipping"
                    );
                }
            }
        }

        Ok(adapters)
    }

    /// Load a single `.wasm` file by path.
    ///
    /// The plugin name is derived from the file stem (e.g. `notion.wasm`
    /// becomes plugin name `"notion"`).
    pub async fn load_plugin(&mut self, path: &Path) -> Result<PluginAdapter> {
        let name = path
            .file_stem()
            .and_then(|s| s.to_str())
            .ok_or_else(|| SandboxError::Plugin {
                reason: format!("cannot derive plugin name from path: {}", path.display()),
            })?
            .to_owned();

        let wasm_bytes = tokio::fs::read(path).await.map_err(SandboxError::Io)?;

        tracing::debug!(
            plugin = %name,
            path = %path.display(),
            size_bytes = wasm_bytes.len(),
            "read wasm bytes from disk"
        );

        // Load into the runtime (synchronous, CPU-bound compilation).
        let plugin_info = {
            let rt = Arc::clone(&self.runtime);
            let name_clone = name.clone();
            tokio::task::spawn_blocking(move || {
                let handle = tokio::runtime::Handle::current();
                let mut rt_guard = handle.block_on(rt.lock());
                let info = rt_guard.load_plugin(&name_clone, &wasm_bytes)?;
                Ok::<_, SandboxError>(info.clone())
            })
            .await
            .map_err(|e| SandboxError::Execution(format!("blocking task panicked: {e}")))?
        }?;

        self.loaded_names.push(name);

        Ok(PluginAdapter::new(plugin_info, Arc::clone(&self.runtime)))
    }

    /// Unload a plugin by name.
    ///
    /// Removes the plugin from the sandbox runtime and from the internal
    /// tracking list.
    pub async fn unload_plugin(&mut self, name: &str) -> Result<()> {
        {
            let mut rt = self.runtime.lock().await;
            rt.unload_plugin(name)?;
        }

        self.loaded_names.retain(|n| n != name);
        tracing::info!(plugin = %name, "unloaded plugin via loader");
        Ok(())
    }

    /// List all plugin names loaded through this loader.
    pub fn list_loaded(&self) -> Vec<String> {
        self.loaded_names.clone()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use openintent_adapters::Adapter;
    use std::fs;

    /// Minimal valid Wasm module (magic + version, no sections).
    fn minimal_wasm() -> Vec<u8> {
        vec![0x00, 0x61, 0x73, 0x6D, 0x01, 0x00, 0x00, 0x00]
    }

    #[test]
    fn new_loader_with_defaults() {
        let dir = PathBuf::from("/tmp/openintent-test-plugins");
        let loader = PluginLoader::new(dir.clone(), SandboxConfig::default());
        assert!(loader.is_ok());
        let loader = loader.unwrap();
        assert_eq!(loader.plugins_dir(), dir);
        assert!(loader.list_loaded().is_empty());
    }

    #[test]
    fn with_runtime_shares_runtime() {
        let rt = SandboxRuntime::with_defaults().expect("runtime creation must succeed in tests");
        let rt = Arc::new(Mutex::new(rt));
        let loader = PluginLoader::with_runtime(PathBuf::from("/tmp"), Arc::clone(&rt));
        // Both should point to the same allocation.
        assert!(Arc::ptr_eq(&loader.runtime(), &rt));
    }

    #[tokio::test]
    async fn load_all_returns_empty_for_missing_dir() {
        let dir = PathBuf::from("/tmp/openintent-nonexistent-dir-test");
        // Make sure it does not exist.
        let _ = fs::remove_dir_all(&dir);
        let mut loader =
            PluginLoader::new(dir, SandboxConfig::default()).expect("loader creation must succeed");
        let adapters = loader.load_all().await;
        assert!(adapters.is_ok());
        assert!(adapters.unwrap().is_empty());
    }

    #[tokio::test]
    async fn load_all_discovers_wasm_files() {
        let tmp = tempfile::tempdir().expect("tempdir creation must succeed in tests");
        let dir = tmp.path().to_path_buf();

        // Write some minimal wasm files.
        fs::write(dir.join("alpha.wasm"), minimal_wasm()).expect("write must succeed");
        fs::write(dir.join("beta.wasm"), minimal_wasm()).expect("write must succeed");
        // Write a non-wasm file that should be ignored.
        fs::write(dir.join("readme.txt"), b"not a plugin").expect("write must succeed");

        let mut loader =
            PluginLoader::new(dir, SandboxConfig::default()).expect("loader creation must succeed");

        let adapters = loader.load_all().await.expect("load_all must succeed");
        assert_eq!(adapters.len(), 2);

        let names: Vec<&str> = adapters.iter().map(|a| a.plugin_name()).collect();
        assert!(names.contains(&"alpha"));
        assert!(names.contains(&"beta"));

        let loaded = loader.list_loaded();
        assert_eq!(loaded.len(), 2);
    }

    #[tokio::test]
    async fn load_single_plugin() {
        let tmp = tempfile::tempdir().expect("tempdir creation must succeed in tests");
        let wasm_path = tmp.path().join("myplugin.wasm");
        fs::write(&wasm_path, minimal_wasm()).expect("write must succeed");

        let mut loader = PluginLoader::new(tmp.path().to_path_buf(), SandboxConfig::default())
            .expect("loader creation must succeed");

        let adapter = loader.load_plugin(&wasm_path).await;
        assert!(adapter.is_ok());
        let adapter = adapter.unwrap();
        assert_eq!(adapter.plugin_name(), "myplugin");
        assert_eq!(adapter.id(), "wasm:myplugin");
        assert_eq!(loader.list_loaded(), vec!["myplugin".to_owned()]);
    }

    #[tokio::test]
    async fn unload_plugin_removes_from_tracking() {
        let tmp = tempfile::tempdir().expect("tempdir creation must succeed in tests");
        let wasm_path = tmp.path().join("removeme.wasm");
        fs::write(&wasm_path, minimal_wasm()).expect("write must succeed");

        let mut loader = PluginLoader::new(tmp.path().to_path_buf(), SandboxConfig::default())
            .expect("loader creation must succeed");

        loader
            .load_plugin(&wasm_path)
            .await
            .expect("load must succeed");
        assert_eq!(loader.list_loaded().len(), 1);

        loader
            .unload_plugin("removeme")
            .await
            .expect("unload must succeed");
        assert!(loader.list_loaded().is_empty());
    }

    #[tokio::test]
    async fn unload_nonexistent_returns_error() {
        let tmp = tempfile::tempdir().expect("tempdir creation must succeed in tests");
        let mut loader = PluginLoader::new(tmp.path().to_path_buf(), SandboxConfig::default())
            .expect("loader creation must succeed");

        let result = loader.unload_plugin("ghost").await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn load_invalid_wasm_returns_error() {
        let tmp = tempfile::tempdir().expect("tempdir creation must succeed in tests");
        let bad_path = tmp.path().join("bad.wasm");
        fs::write(&bad_path, b"not valid wasm bytes").expect("write must succeed");

        let mut loader = PluginLoader::new(tmp.path().to_path_buf(), SandboxConfig::default())
            .expect("loader creation must succeed");

        let result = loader.load_plugin(&bad_path).await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn load_all_skips_invalid_plugins() {
        let tmp = tempfile::tempdir().expect("tempdir creation must succeed in tests");
        let dir = tmp.path().to_path_buf();

        // One valid, one invalid.
        fs::write(dir.join("good.wasm"), minimal_wasm()).expect("write must succeed");
        fs::write(dir.join("bad.wasm"), b"garbage").expect("write must succeed");

        let mut loader =
            PluginLoader::new(dir, SandboxConfig::default()).expect("loader creation must succeed");

        let adapters = loader.load_all().await.expect("load_all must succeed");
        // Only the valid plugin should have loaded.
        assert_eq!(adapters.len(), 1);
        assert_eq!(adapters[0].plugin_name(), "good");
    }

    #[tokio::test]
    async fn load_all_errors_on_non_directory() {
        let tmp = tempfile::tempdir().expect("tempdir creation must succeed in tests");
        let file_path = tmp.path().join("not_a_dir.txt");
        fs::write(&file_path, b"hello").expect("write must succeed");

        let mut loader = PluginLoader::new(file_path, SandboxConfig::default())
            .expect("loader creation must succeed");

        let result = loader.load_all().await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn load_nonexistent_file_returns_error() {
        let tmp = tempfile::tempdir().expect("tempdir creation must succeed in tests");
        let missing = tmp.path().join("does_not_exist.wasm");

        let mut loader = PluginLoader::new(tmp.path().to_path_buf(), SandboxConfig::default())
            .expect("loader creation must succeed");

        let result = loader.load_plugin(&missing).await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn duplicate_load_returns_error() {
        let tmp = tempfile::tempdir().expect("tempdir creation must succeed in tests");
        let wasm_path = tmp.path().join("dupe.wasm");
        fs::write(&wasm_path, minimal_wasm()).expect("write must succeed");

        let mut loader = PluginLoader::new(tmp.path().to_path_buf(), SandboxConfig::default())
            .expect("loader creation must succeed");

        loader
            .load_plugin(&wasm_path)
            .await
            .expect("first load must succeed");
        let result = loader.load_plugin(&wasm_path).await;
        assert!(result.is_err());
    }
}
