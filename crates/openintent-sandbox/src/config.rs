//! Sandbox configuration.
//!
//! [`SandboxConfig`] controls the resource limits and permissions applied to
//! every Wasm plugin instance.  Sensible defaults are provided via the
//! [`Default`] implementation, and a builder-style API allows callers to
//! customise individual fields fluently.

/// Resource limits and permissions for the Wasm sandbox.
#[derive(Debug, Clone)]
pub struct SandboxConfig {
    /// Maximum linear memory a plugin may allocate, in bytes.
    ///
    /// Default: **16 MiB** (16 * 1024 * 1024).
    pub max_memory: usize,

    /// Maximum wall-clock time a single tool invocation may run, in
    /// milliseconds.
    ///
    /// Default: **5 000 ms** (5 seconds).
    pub max_execution_ms: u64,

    /// Maximum fuel (abstract instruction count) per execution.
    ///
    /// Fuel metering is the primary mechanism wasmtime uses to bound CPU usage
    /// deterministically, independent of wall-clock time.
    ///
    /// Default: **1 000 000**.
    pub max_fuel: u64,

    /// Whether plugins are allowed to access the host filesystem.
    ///
    /// Default: **false**.
    pub allow_fs: bool,

    /// Whether plugins are allowed to make network requests.
    ///
    /// Default: **false**.
    pub allow_network: bool,
}

impl Default for SandboxConfig {
    fn default() -> Self {
        Self {
            max_memory: 16 * 1024 * 1024,
            max_execution_ms: 5000,
            max_fuel: 1_000_000,
            allow_fs: false,
            allow_network: false,
        }
    }
}

impl SandboxConfig {
    /// Create a new configuration with default values.
    pub fn new() -> Self {
        Self::default()
    }

    /// Set the maximum memory limit (in bytes).
    pub fn with_max_memory(mut self, bytes: usize) -> Self {
        self.max_memory = bytes;
        self
    }

    /// Set the maximum execution time (in milliseconds).
    pub fn with_max_execution_ms(mut self, ms: u64) -> Self {
        self.max_execution_ms = ms;
        self
    }

    /// Set the maximum fuel (instruction count).
    pub fn with_max_fuel(mut self, fuel: u64) -> Self {
        self.max_fuel = fuel;
        self
    }

    /// Enable or disable filesystem access for plugins.
    pub fn with_allow_fs(mut self, allow: bool) -> Self {
        self.allow_fs = allow;
        self
    }

    /// Enable or disable network access for plugins.
    pub fn with_allow_network(mut self, allow: bool) -> Self {
        self.allow_network = allow;
        self
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_config_values() {
        let cfg = SandboxConfig::default();
        assert_eq!(cfg.max_memory, 16 * 1024 * 1024);
        assert_eq!(cfg.max_execution_ms, 5000);
        assert_eq!(cfg.max_fuel, 1_000_000);
        assert!(!cfg.allow_fs);
        assert!(!cfg.allow_network);
    }

    #[test]
    fn new_equals_default() {
        let a = SandboxConfig::new();
        let b = SandboxConfig::default();
        assert_eq!(a.max_memory, b.max_memory);
        assert_eq!(a.max_execution_ms, b.max_execution_ms);
        assert_eq!(a.max_fuel, b.max_fuel);
        assert_eq!(a.allow_fs, b.allow_fs);
        assert_eq!(a.allow_network, b.allow_network);
    }

    #[test]
    fn builder_with_max_memory() {
        let cfg = SandboxConfig::new().with_max_memory(64 * 1024 * 1024);
        assert_eq!(cfg.max_memory, 64 * 1024 * 1024);
    }

    #[test]
    fn builder_with_max_execution_ms() {
        let cfg = SandboxConfig::new().with_max_execution_ms(10_000);
        assert_eq!(cfg.max_execution_ms, 10_000);
    }

    #[test]
    fn builder_with_max_fuel() {
        let cfg = SandboxConfig::new().with_max_fuel(2_000_000);
        assert_eq!(cfg.max_fuel, 2_000_000);
    }

    #[test]
    fn builder_with_permissions() {
        let cfg = SandboxConfig::new()
            .with_allow_fs(true)
            .with_allow_network(true);
        assert!(cfg.allow_fs);
        assert!(cfg.allow_network);
    }

    #[test]
    fn builder_chaining() {
        let cfg = SandboxConfig::new()
            .with_max_memory(32 * 1024 * 1024)
            .with_max_execution_ms(1000)
            .with_max_fuel(500_000)
            .with_allow_fs(false)
            .with_allow_network(true);
        assert_eq!(cfg.max_memory, 32 * 1024 * 1024);
        assert_eq!(cfg.max_execution_ms, 1000);
        assert_eq!(cfg.max_fuel, 500_000);
        assert!(!cfg.allow_fs);
        assert!(cfg.allow_network);
    }

    #[test]
    fn config_is_clone() {
        let original = SandboxConfig::new().with_max_fuel(42);
        let cloned = original.clone();
        assert_eq!(cloned.max_fuel, 42);
    }
}
