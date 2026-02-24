//! Vault error types.
//!
//! All vault subsystems surface errors through [`VaultError`], which is the
//! single error type returned by every public API in this crate.  Each variant
//! carries enough context for callers to decide how to handle the failure
//! without inspecting opaque strings.

/// Unified error type for the OpenIntentOS credential vault.
#[derive(Debug, thiserror::Error)]
pub enum VaultError {
    // -- Crypto errors ------------------------------------------------------
    /// Encryption failed (e.g. invalid key length, ring internal error).
    #[error("encryption failed: {reason}")]
    EncryptionFailed { reason: String },

    /// Decryption failed (e.g. wrong key, corrupted ciphertext, bad nonce).
    #[error("decryption failed: {reason}")]
    DecryptionFailed { reason: String },

    /// Key derivation failed (e.g. invalid parameters for PBKDF2).
    #[error("key derivation failed: {reason}")]
    KeyDerivationFailed { reason: String },

    // -- Keychain errors ----------------------------------------------------
    /// The master key could not be retrieved from the keychain.
    #[error("master key not found in keychain")]
    MasterKeyNotFound,

    /// Writing the master key to the keychain failed.
    #[error("failed to store master key: {reason}")]
    MasterKeyStoreFailed { reason: String },

    /// The keychain backend is unavailable or unsupported on this platform.
    #[error("keychain unavailable: {reason}")]
    KeychainUnavailable { reason: String },

    // -- Store errors -------------------------------------------------------
    /// The requested credential does not exist.
    #[error("credential not found: provider={provider}")]
    CredentialNotFound { provider: String },

    /// A credential for this provider already exists.
    #[error("credential already exists: provider={provider}")]
    CredentialAlreadyExists { provider: String },

    /// Database schema migration failed.
    #[error("migration failed: {reason}")]
    MigrationFailed { reason: String },

    // -- Policy errors ------------------------------------------------------
    /// The requested action was denied by policy.
    #[error("action denied by policy: provider={provider}, action={action}")]
    ActionDenied { provider: String, action: String },

    /// A policy with conflicting rules was detected.
    #[error("conflicting policy: {reason}")]
    ConflictingPolicy { reason: String },

    /// The referenced policy does not exist.
    #[error("policy not found: id={policy_id}")]
    PolicyNotFound { policy_id: i64 },

    // -- Underlying errors --------------------------------------------------
    /// SQLite error from `rusqlite`.
    #[error("database error: {0}")]
    Database(#[from] rusqlite::Error),

    /// JSON serialization/deserialization error.
    #[error("serialization error: {0}")]
    Serialization(#[from] serde_json::Error),

    /// I/O error from the filesystem (keychain file operations, etc.).
    #[error("io error: {0}")]
    Io(#[from] std::io::Error),

    // -- Generic ------------------------------------------------------------
    /// Catch-all for unexpected internal errors that don't fit a specific
    /// variant.  Prefer a typed variant whenever possible.
    #[error("internal vault error: {0}")]
    Internal(String),
}

/// Convenience alias used throughout the vault crate.
pub type Result<T> = std::result::Result<T, VaultError>;
