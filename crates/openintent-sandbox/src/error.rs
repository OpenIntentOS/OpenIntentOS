//! Sandbox error types.
//!
//! All sandbox subsystems surface errors through [`SandboxError`], which is the
//! single error type returned by every public API in this crate.

/// Unified error type for the WebAssembly plugin sandbox.
#[derive(Debug, thiserror::Error)]
pub enum SandboxError {
    /// Wasm module failed to compile (e.g. invalid bytecode).
    #[error("wasm compilation error: {0}")]
    Compilation(String),

    /// Wasm module could not be instantiated (e.g. missing imports).
    #[error("wasm instantiation error: {0}")]
    Instantiation(String),

    /// A Wasm function call returned an error.
    #[error("wasm execution error: {0}")]
    Execution(String),

    /// A Wasm trap was raised during execution.
    #[error("wasm trap: {0}")]
    Trap(String),

    /// A plugin-level error (e.g. malformed metadata).
    #[error("plugin error: {reason}")]
    Plugin {
        /// Human-readable description of what went wrong.
        reason: String,
    },

    /// Execution exceeded the configured time limit.
    #[error("timeout: execution exceeded {limit_ms}ms")]
    Timeout {
        /// The configured limit in milliseconds.
        limit_ms: u64,
    },

    /// The Wasm module consumed more memory than allowed.
    #[error("memory limit exceeded: {used} > {limit}")]
    MemoryLimit {
        /// Actual memory usage in bytes.
        used: usize,
        /// Configured maximum in bytes.
        limit: usize,
    },

    /// An I/O error occurred (e.g. reading a `.wasm` file from disk).
    #[error("io error: {0}")]
    Io(#[from] std::io::Error),
}

/// Convenience alias used throughout the sandbox crate.
pub type Result<T> = std::result::Result<T, SandboxError>;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn compilation_error_display() {
        let err = SandboxError::Compilation("bad magic".into());
        assert_eq!(err.to_string(), "wasm compilation error: bad magic");
    }

    #[test]
    fn instantiation_error_display() {
        let err = SandboxError::Instantiation("missing import".into());
        assert_eq!(err.to_string(), "wasm instantiation error: missing import");
    }

    #[test]
    fn execution_error_display() {
        let err = SandboxError::Execution("divide by zero".into());
        assert_eq!(err.to_string(), "wasm execution error: divide by zero");
    }

    #[test]
    fn trap_error_display() {
        let err = SandboxError::Trap("unreachable".into());
        assert_eq!(err.to_string(), "wasm trap: unreachable");
    }

    #[test]
    fn plugin_error_display() {
        let err = SandboxError::Plugin {
            reason: "bad metadata".into(),
        };
        assert_eq!(err.to_string(), "plugin error: bad metadata");
    }

    #[test]
    fn timeout_error_display() {
        let err = SandboxError::Timeout { limit_ms: 5000 };
        assert_eq!(err.to_string(), "timeout: execution exceeded 5000ms");
    }

    #[test]
    fn memory_limit_error_display() {
        let err = SandboxError::MemoryLimit {
            used: 32_000_000,
            limit: 16_000_000,
        };
        assert_eq!(
            err.to_string(),
            "memory limit exceeded: 32000000 > 16000000"
        );
    }

    #[test]
    fn io_error_from_std() {
        let io_err = std::io::Error::new(std::io::ErrorKind::NotFound, "file gone");
        let sandbox_err = SandboxError::from(io_err);
        assert!(sandbox_err.to_string().contains("file gone"));
    }
}
