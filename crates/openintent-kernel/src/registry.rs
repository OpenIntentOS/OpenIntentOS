//! Service and adapter registry.
//!
//! The registry tracks the lifecycle of every adapter (service connector)
//! known to the kernel: its connection status, when it was last health-checked,
//! and any error information.
//!
//! Internally the registry is backed by [`DashMap`] which provides lock-free
//! concurrent reads and fine-grained write locking, making it safe to share
//! across tasks without a global `RwLock`.
//!
//! # Example
//!
//! ```rust
//! # use openintent_kernel::registry::{AdapterRegistry, AdapterStatus};
//! let registry = AdapterRegistry::new();
//! registry.register("email", "Email IMAP/SMTP adapter");
//!
//! registry.set_status("email", AdapterStatus::Connected);
//! let info = registry.get("email").unwrap();
//! assert_eq!(info.status, AdapterStatus::Connected);
//! ```

use std::sync::Arc;

use chrono::{DateTime, Utc};
use dashmap::DashMap;
use serde::{Deserialize, Serialize};

use crate::error::{KernelError, Result};

// ---------------------------------------------------------------------------
// Public types
// ---------------------------------------------------------------------------

/// Connection status of a registered adapter.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum AdapterStatus {
    /// The adapter is registered but has not yet attempted to connect.
    Registered,
    /// The adapter is connected and healthy.
    Connected,
    /// The adapter has been explicitly disconnected.
    Disconnected,
    /// The adapter encountered an error and is not usable.
    Error,
}

/// Metadata about a registered adapter.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AdapterInfo {
    /// Unique identifier for this adapter (e.g. "email", "feishu").
    pub id: String,
    /// Human-readable description.
    pub description: String,
    /// Current connection status.
    pub status: AdapterStatus,
    /// When the adapter was registered with the kernel.
    pub registered_at: DateTime<Utc>,
    /// Timestamp of the most recent successful health check (if any).
    pub last_health_check: Option<DateTime<Utc>>,
    /// If `status == Error`, contains a human-readable error message.
    pub last_error: Option<String>,
}

// ---------------------------------------------------------------------------
// Registry
// ---------------------------------------------------------------------------

/// Concurrent adapter registry backed by [`DashMap`].
///
/// The registry is cheaply cloneable (`Arc`-backed) and `Send + Sync`.
#[derive(Clone)]
pub struct AdapterRegistry {
    inner: Arc<DashMap<String, AdapterInfo>>,
}

impl AdapterRegistry {
    /// Create an empty registry.
    #[must_use]
    pub fn new() -> Self {
        Self {
            inner: Arc::new(DashMap::new()),
        }
    }

    /// Register a new adapter.
    ///
    /// If an adapter with the same `id` already exists, it is overwritten.
    pub fn register(&self, id: impl Into<String>, description: impl Into<String>) {
        let id = id.into();
        let description = description.into();

        tracing::info!(adapter_id = %id, "adapter registered");

        self.inner.insert(
            id.clone(),
            AdapterInfo {
                id,
                description,
                status: AdapterStatus::Registered,
                registered_at: Utc::now(),
                last_health_check: None,
                last_error: None,
            },
        );
    }

    /// Remove an adapter from the registry.
    ///
    /// Returns the removed [`AdapterInfo`] if it existed.
    pub fn unregister(&self, id: &str) -> Option<AdapterInfo> {
        let removed = self.inner.remove(id).map(|(_, info)| info);
        if removed.is_some() {
            tracing::info!(adapter_id = %id, "adapter unregistered");
        }
        removed
    }

    /// Retrieve a snapshot of an adapter's info.
    pub fn get(&self, id: &str) -> Result<AdapterInfo> {
        self.inner
            .get(id)
            .map(|entry| entry.value().clone())
            .ok_or_else(|| KernelError::AdapterNotFound {
                adapter_id: id.to_string(),
            })
    }

    /// Update the status of a registered adapter.
    pub fn set_status(&self, id: &str, status: AdapterStatus) -> Result<()> {
        let mut entry = self
            .inner
            .get_mut(id)
            .ok_or_else(|| KernelError::AdapterNotFound {
                adapter_id: id.to_string(),
            })?;

        let old = entry.status;
        entry.status = status;

        // Clear the error message when transitioning away from Error.
        if old == AdapterStatus::Error && status != AdapterStatus::Error {
            entry.last_error = None;
        }

        tracing::debug!(
            adapter_id = %id,
            old_status = ?old,
            new_status = ?status,
            "adapter status changed"
        );

        Ok(())
    }

    /// Record a failed state with an error message.
    pub fn set_error(&self, id: &str, error: impl Into<String>) -> Result<()> {
        let mut entry = self
            .inner
            .get_mut(id)
            .ok_or_else(|| KernelError::AdapterNotFound {
                adapter_id: id.to_string(),
            })?;

        let error = error.into();
        entry.status = AdapterStatus::Error;
        entry.last_error = Some(error.clone());

        tracing::warn!(adapter_id = %id, error = %error, "adapter entered error state");

        Ok(())
    }

    /// Record a successful health check for the given adapter.
    pub fn record_health_check(&self, id: &str) -> Result<()> {
        let mut entry = self
            .inner
            .get_mut(id)
            .ok_or_else(|| KernelError::AdapterNotFound {
                adapter_id: id.to_string(),
            })?;

        let now = Utc::now();
        entry.last_health_check = Some(now);

        tracing::trace!(adapter_id = %id, timestamp = %now, "health check recorded");

        Ok(())
    }

    /// Return a list of all registered adapter IDs.
    pub fn list_ids(&self) -> Vec<String> {
        self.inner.iter().map(|e| e.key().clone()).collect()
    }

    /// Return a snapshot of all registered adapters.
    pub fn list_all(&self) -> Vec<AdapterInfo> {
        self.inner.iter().map(|e| e.value().clone()).collect()
    }

    /// Return only adapters that are in the given status.
    pub fn list_by_status(&self, status: AdapterStatus) -> Vec<AdapterInfo> {
        self.inner
            .iter()
            .filter(|e| e.value().status == status)
            .map(|e| e.value().clone())
            .collect()
    }

    /// Return the total number of registered adapters.
    pub fn count(&self) -> usize {
        self.inner.len()
    }

    /// Check whether an adapter exists and is in the `Connected` state.
    pub fn is_available(&self, id: &str) -> bool {
        self.inner
            .get(id)
            .map(|e| e.status == AdapterStatus::Connected)
            .unwrap_or(false)
    }
}

impl Default for AdapterRegistry {
    fn default() -> Self {
        Self::new()
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn register_and_retrieve() {
        let registry = AdapterRegistry::new();
        registry.register("email", "Email adapter");

        let info = registry.get("email").expect("adapter should exist");
        assert_eq!(info.id, "email");
        assert_eq!(info.status, AdapterStatus::Registered);
        assert!(info.last_health_check.is_none());
    }

    #[test]
    fn set_status_transitions() {
        let registry = AdapterRegistry::new();
        registry.register("feishu", "Feishu adapter");

        registry
            .set_status("feishu", AdapterStatus::Connected)
            .expect("set connected");
        assert_eq!(
            registry.get("feishu").unwrap().status,
            AdapterStatus::Connected
        );

        registry
            .set_status("feishu", AdapterStatus::Disconnected)
            .expect("set disconnected");
        assert_eq!(
            registry.get("feishu").unwrap().status,
            AdapterStatus::Disconnected
        );
    }

    #[test]
    fn error_state_with_message() {
        let registry = AdapterRegistry::new();
        registry.register("github", "GitHub adapter");

        registry
            .set_error("github", "rate limited")
            .expect("set error");

        let info = registry.get("github").unwrap();
        assert_eq!(info.status, AdapterStatus::Error);
        assert_eq!(info.last_error.as_deref(), Some("rate limited"));

        // Transitioning away from error clears the message.
        registry
            .set_status("github", AdapterStatus::Connected)
            .expect("recover");
        let info = registry.get("github").unwrap();
        assert_eq!(info.status, AdapterStatus::Connected);
        assert!(info.last_error.is_none());
    }

    #[test]
    fn health_check_recording() {
        let registry = AdapterRegistry::new();
        registry.register("shell", "Shell adapter");

        assert!(registry.get("shell").unwrap().last_health_check.is_none());

        registry.record_health_check("shell").expect("record");
        assert!(registry.get("shell").unwrap().last_health_check.is_some());
    }

    #[test]
    fn list_by_status() {
        let registry = AdapterRegistry::new();
        registry.register("a", "A");
        registry.register("b", "B");
        registry.register("c", "C");

        registry.set_status("a", AdapterStatus::Connected).unwrap();
        registry.set_status("b", AdapterStatus::Connected).unwrap();
        // "c" remains Registered.

        let connected = registry.list_by_status(AdapterStatus::Connected);
        assert_eq!(connected.len(), 2);

        let registered = registry.list_by_status(AdapterStatus::Registered);
        assert_eq!(registered.len(), 1);
        assert_eq!(registered[0].id, "c");
    }

    #[test]
    fn unregister() {
        let registry = AdapterRegistry::new();
        registry.register("temp", "Temporary");
        assert_eq!(registry.count(), 1);

        let removed = registry.unregister("temp");
        assert!(removed.is_some());
        assert_eq!(registry.count(), 0);
        assert!(registry.get("temp").is_err());
    }

    #[test]
    fn not_found_error() {
        let registry = AdapterRegistry::new();
        let result = registry.get("nonexistent");
        assert!(matches!(result, Err(KernelError::AdapterNotFound { .. })));
    }

    #[test]
    fn is_available() {
        let registry = AdapterRegistry::new();
        registry.register("x", "X");

        assert!(!registry.is_available("x")); // Registered != Connected
        assert!(!registry.is_available("missing"));

        registry.set_status("x", AdapterStatus::Connected).unwrap();
        assert!(registry.is_available("x"));
    }
}
