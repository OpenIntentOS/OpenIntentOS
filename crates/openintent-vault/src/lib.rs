//! Encrypted credential vault for OpenIntentOS.
//!
//! This crate provides secure credential storage, access control, and audit
//! logging for the OpenIntentOS AI operating system. All sensitive data is
//! encrypted at rest using AES-256-GCM and the master encryption key is
//! protected by the OS keychain (or a file-based fallback).
//!
//! # Modules
//!
//! - [`crypto`] — AES-256-GCM encryption/decryption, PBKDF2 key derivation.
//! - [`keychain`] — OS keychain integration for master key storage.
//! - [`store`] — SQLite-backed encrypted credential CRUD.
//! - [`policy`] — Permission policy engine and audit logging.
//! - [`error`] — Unified error types.
//!
//! # Quick Start
//!
//! ```rust,no_run
//! use openintent_vault::crypto;
//! use openintent_vault::store::{Vault, CredentialType};
//! use openintent_vault::policy::{PolicyEngine, PolicyDecision};
//!
//! # fn example() -> openintent_vault::error::Result<()> {
//! // Derive a master key from a password (or load from keychain).
//! let (salt, master_key) = crypto::derive_key_from_password(b"my-secret")?;
//!
//! // Open the vault.
//! let vault = Vault::open("data/vault.db", &master_key)?;
//!
//! // Store an API key.
//! vault.store_credential(
//!     "anthropic",
//!     CredentialType::ApiKey,
//!     &serde_json::json!({ "api_key": "sk-ant-..." }),
//!     None,
//!     Some("work"),
//!     None,
//! )?;
//!
//! // Set up policies.
//! let engine = PolicyEngine::new(&vault);
//! engine.add_policy("anthropic", "chat", "*", PolicyDecision::Allow, None)?;
//!
//! // Evaluate an action.
//! let decision = engine.evaluate("anthropic", "chat", "model:opus")?;
//! assert_eq!(decision, PolicyDecision::Allow);
//! # Ok(())
//! # }
//! ```

pub mod crypto;
pub mod error;
pub mod keychain;
pub mod policy;
pub mod store;

// Re-export the most commonly used types at the crate root for convenience.
pub use error::{Result, VaultError};
pub use keychain::{FileKeychain, KeychainProvider};
pub use policy::{PolicyDecision, PolicyEngine};
pub use store::{Credential, CredentialType, Vault};
