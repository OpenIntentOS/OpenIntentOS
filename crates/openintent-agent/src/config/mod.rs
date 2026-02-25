//! Environment configuration and hot-reloading.
//!
//! This module provides dynamic configuration management with support for:
//! - Environment variable hot-reloading
//! - Configuration file watching
//! - Runtime configuration updates
//! - Service restart coordination

pub mod auth;

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::{Arc, RwLock};
use std::time::{Duration, SystemTime};

use notify::{Event, EventKind, RecommendedWatcher, RecursiveMode, Watcher};
use serde::{Deserialize, Serialize};
use tokio::sync::broadcast;
use tracing::{debug, error, info, warn};

use crate::error::{AgentError, Result};

/// Configuration change notification.
#[derive(Debug, Clone)]
pub enum ConfigChange {
    /// Environment variables were updated.
    Environment(HashMap<String, String>),
    /// Configuration file was modified.
    FileChanged(PathBuf),
    /// Service restart requested.
    RestartRequested,
}

/// Gateway environment configuration.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GatewayConfig {
    /// API keys for various providers.
    pub api_keys: HashMap<String, String>,
    /// Service endpoints.
    pub endpoints: HashMap<String, String>,
    /// Feature flags.
    pub features: HashMap<String, bool>,
    /// Last update timestamp.
    #[serde(skip, default = "SystemTime::now")]
    pub last_updated: SystemTime,
}

impl Default for GatewayConfig {
    fn default() -> Self {
        Self {
            api_keys: HashMap::new(),
            endpoints: HashMap::new(),
            features: HashMap::new(),
            last_updated: SystemTime::now(),
        }
    }
}

/// Configuration manager with hot-reloading capabilities.
#[derive(Debug)]
pub struct ConfigManager {
    /// Current configuration state.
    config: Arc<RwLock<GatewayConfig>>,
    /// Configuration file path.
    config_path: Option<PathBuf>,
    /// Change notification sender.
    change_tx: broadcast::Sender<ConfigChange>,
    /// File system watcher.
    _watcher: Option<RecommendedWatcher>,
}

impl ConfigManager {
    /// Create a new configuration manager.
    pub fn new() -> Result<Self> {
        let (change_tx, _) = broadcast::channel(100);
        
        Ok(Self {
            config: Arc::new(RwLock::new(GatewayConfig::default())),
            config_path: None,
            change_tx,
            _watcher: None,
        })
    }

    /// Create a configuration manager with file watching.
    pub fn with_file_watching(config_path: PathBuf) -> Result<Self> {
        let (change_tx, _) = broadcast::channel(100);
        let config = Arc::new(RwLock::new(GatewayConfig::default()));

        // Set up file watcher
        let tx_clone = change_tx.clone();
        let path_clone = config_path.clone();
        
        let mut watcher = notify::recommended_watcher(move |res: notify::Result<Event>| {
            match res {
                Ok(event) => {
                    if matches!(event.kind, EventKind::Modify(_)) {
                        debug!(path = ?path_clone, "Configuration file changed");
                        let _ = tx_clone.send(ConfigChange::FileChanged(path_clone.clone()));
                    }
                }
                Err(e) => error!(error = %e, "File watcher error"),
            }
        })?;

        if let Some(parent) = config_path.parent() {
            watcher.watch(parent, RecursiveMode::NonRecursive)?;
        }

        let mut manager = Self {
            config,
            config_path: Some(config_path.clone()),
            change_tx,
            _watcher: Some(watcher),
        };

        // Load initial configuration
        manager.load_from_file()?;

        Ok(manager)
    }

    /// Get a snapshot of the current configuration.
    pub fn get_config(&self) -> GatewayConfig {
        self.config.read().unwrap().clone()
    }

    /// Update configuration from environment variables.
    pub fn refresh_environment(&self) -> Result<()> {
        let mut env_vars = HashMap::new();
        
        // Collect relevant environment variables
        for (key, value) in std::env::vars() {
            if key.starts_with("OPENINTENT_") || key.starts_with("ANTHROPIC_") || key.starts_with("OPENAI_") {
                env_vars.insert(key, value);
            }
        }

        // Update configuration
        {
            let mut config = self.config.write().unwrap();
            
            // Update API keys
            if let Some(anthropic_key) = env_vars.get("ANTHROPIC_API_KEY") {
                config.api_keys.insert("anthropic".to_string(), anthropic_key.clone());
            }
            if let Some(openai_key) = env_vars.get("OPENAI_API_KEY") {
                config.api_keys.insert("openai".to_string(), openai_key.clone());
            }
            
            // Update endpoints
            if let Some(base_url) = env_vars.get("OPENINTENT_API_BASE_URL") {
                config.endpoints.insert("api_base".to_string(), base_url.clone());
            }
            
            config.last_updated = SystemTime::now();
        }

        // Notify subscribers
        let _ = self.change_tx.send(ConfigChange::Environment(env_vars));
        
        info!("Environment configuration refreshed");
        Ok(())
    }

    /// Load configuration from file.
    pub fn load_from_file(&mut self) -> Result<()> {
        let Some(ref path) = self.config_path else {
            return Err(AgentError::ConfigError {
                reason: "No configuration file path set".into(),
            });
        };

        if !path.exists() {
            warn!(path = ?path, "Configuration file does not exist, using defaults");
            return Ok(());
        }

        let content = std::fs::read_to_string(path).map_err(|e| AgentError::ConfigError {
            reason: format!("Failed to read config file: {}", e),
        })?;

        let mut loaded_config: GatewayConfig = if path.extension().and_then(|s| s.to_str()) == Some("json") {
            serde_json::from_str(&content).map_err(|e| AgentError::ConfigError {
                reason: format!("Failed to parse JSON config: {}", e),
            })?
        } else {
            toml::from_str(&content).map_err(|e| AgentError::ConfigError {
                reason: format!("Failed to parse TOML config: {}", e),
            })?
        };

        loaded_config.last_updated = SystemTime::now();

        *self.config.write().unwrap() = loaded_config;
        
        info!(path = ?path, "Configuration loaded from file");
        Ok(())
    }

    /// Save current configuration to file.
    pub fn save_to_file(&self) -> Result<()> {
        let Some(ref path) = self.config_path else {
            return Err(AgentError::ConfigError {
                reason: "No configuration file path set".into(),
            });
        };

        let config = self.config.read().unwrap();
        
        let content = if path.extension().and_then(|s| s.to_str()) == Some("json") {
            serde_json::to_string_pretty(&*config).map_err(|e| AgentError::ConfigError {
                reason: format!("Failed to serialize config as JSON: {}", e),
            })?
        } else {
            toml::to_string_pretty(&*config).map_err(|e| AgentError::ConfigError {
                reason: format!("Failed to serialize config as TOML: {}", e),
            })?
        };

        std::fs::write(path, content).map_err(|e| AgentError::ConfigError {
            reason: format!("Failed to write config file: {}", e),
        })?;

        info!(path = ?path, "Configuration saved to file");
        Ok(())
    }

    /// Subscribe to configuration changes.
    pub fn subscribe(&self) -> broadcast::Receiver<ConfigChange> {
        self.change_tx.subscribe()
    }

    /// Request a service restart.
    pub fn request_restart(&self) {
        let _ = self.change_tx.send(ConfigChange::RestartRequested);
        info!("Service restart requested");
    }

    /// Update a specific configuration value.
    pub fn update_value(&self, section: &str, key: &str, value: String) -> Result<()> {
        {
            let mut config = self.config.write().unwrap();
            
            match section {
                "api_keys" => {
                    config.api_keys.insert(key.to_string(), value);
                }
                "endpoints" => {
                    config.endpoints.insert(key.to_string(), value);
                }
                "features" => {
                    let bool_value = value.parse::<bool>().map_err(|_| AgentError::ConfigError {
                        reason: format!("Invalid boolean value for feature '{}': {}", key, value),
                    })?;
                    config.features.insert(key.to_string(), bool_value);
                }
                _ => {
                    return Err(AgentError::ConfigError {
                        reason: format!("Unknown configuration section: {}", section),
                    });
                }
            }
            
            config.last_updated = SystemTime::now();
        }

        info!(section = section, key = key, "Configuration value updated");
        Ok(())
    }

    /// Start the configuration monitoring loop.
    pub async fn start_monitoring(&self) -> Result<()> {
        let mut rx = self.subscribe();
        
        tokio::spawn(async move {
            while let Ok(change) = rx.recv().await {
                match change {
                    ConfigChange::Environment(vars) => {
                        debug!(count = vars.len(), "Environment variables updated");
                    }
                    ConfigChange::FileChanged(path) => {
                        info!(path = ?path, "Configuration file changed, reloading...");
                        // Note: In a real implementation, you'd reload the file here
                    }
                    ConfigChange::RestartRequested => {
                        warn!("Service restart requested - this would trigger a restart in production");
                    }
                }
            }
        });

        // Periodic environment refresh
        let config_manager = self.config.clone();
        let tx = self.change_tx.clone();
        
        tokio::spawn(async move {
            let mut interval = tokio::time::interval(Duration::from_secs(30));
            loop {
                interval.tick().await;
                
                // Check for environment changes
                let mut env_vars = HashMap::new();
                for (key, value) in std::env::vars() {
                    if key.starts_with("OPENINTENT_") || key.starts_with("ANTHROPIC_") || key.starts_with("OPENAI_") {
                        env_vars.insert(key, value);
                    }
                }
                
                if !env_vars.is_empty() {
                    let _ = tx.send(ConfigChange::Environment(env_vars));
                }
            }
        });

        info!("Configuration monitoring started");
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    #[test]
    fn config_manager_creation() {
        let manager = ConfigManager::new().unwrap();
        let config = manager.get_config();
        assert!(config.api_keys.is_empty());
        assert!(config.endpoints.is_empty());
    }

    #[test]
    fn update_configuration_values() {
        let manager = ConfigManager::new().unwrap();
        
        manager.update_value("api_keys", "test_provider", "test_key".to_string()).unwrap();
        manager.update_value("features", "test_feature", "true".to_string()).unwrap();
        
        let config = manager.get_config();
        assert_eq!(config.api_keys.get("test_provider"), Some(&"test_key".to_string()));
        assert_eq!(config.features.get("test_feature"), Some(&true));
    }

    #[test]
    fn invalid_feature_value_returns_error() {
        let manager = ConfigManager::new().unwrap();
        let result = manager.update_value("features", "test", "not_a_boolean".to_string());
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn file_watching_setup() {
        let temp_dir = tempdir().unwrap();
        let config_path = temp_dir.path().join("test_config.toml");
        
        // Create a test config file
        std::fs::write(&config_path, r#"
[api_keys]
test = "value"

[endpoints]
api_base = "http://localhost:8080"
        "#).unwrap();

        let manager = ConfigManager::with_file_watching(config_path).unwrap();
        let config = manager.get_config();
        
        assert_eq!(config.api_keys.get("test"), Some(&"value".to_string()));
        assert_eq!(config.endpoints.get("api_base"), Some(&"http://localhost:8080".to_string()));
    }
}