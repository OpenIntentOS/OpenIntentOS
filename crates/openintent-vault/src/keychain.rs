//! OS keychain integration for master key storage.
//!
//! The master encryption key must never be stored as plaintext on disk. This
//! module provides a [`KeychainProvider`] trait that abstracts over different
//! platform-specific secure storage backends:
//!
//! - **macOS**: Keychain Services (TODO)
//! - **Windows**: DPAPI (TODO)
//! - **Linux**: Secret Service / libsecret (TODO)
//! - **Fallback**: File-based encrypted storage using a device-derived key
//!
//! The [`FileKeychain`] implementation is the cross-platform fallback. It
//! derives an encryption key from a combination of machine-specific data
//! (hostname, username) and a hardcoded application salt. This is not as
//! secure as a proper OS keychain but ensures the master key is never stored
//! in plaintext.
//!
//! # Security Notes
//!
//! - The file-based fallback is a compromise. The device-derived key can be
//!   reconstructed by anyone with access to the same machine. A real OS
//!   keychain provides hardware-backed or OS-protected key storage.
//! - The master key file permissions should be restricted to the current user
//!   (mode 0600 on Unix).

use std::path::{Path, PathBuf};

use crate::crypto;
use crate::error::{Result, VaultError};

// ---------------------------------------------------------------------------
// Trait
// ---------------------------------------------------------------------------

/// Abstraction over platform-specific secure key storage.
///
/// Implementations must be `Send + Sync` so the vault can be used across
/// async tasks.
pub trait KeychainProvider: Send + Sync {
    /// Retrieve the master encryption key.
    ///
    /// Returns [`VaultError::MasterKeyNotFound`] if no key has been stored yet.
    fn get_master_key(&self) -> Result<Vec<u8>>;

    /// Store (or overwrite) the master encryption key.
    fn set_master_key(&self, key: &[u8]) -> Result<()>;

    /// Check whether a master key has been stored.
    fn has_master_key(&self) -> Result<bool>;

    /// Delete the stored master key (e.g. during vault reset).
    fn delete_master_key(&self) -> Result<()>;
}

// ---------------------------------------------------------------------------
// File-based fallback
// ---------------------------------------------------------------------------

/// Application salt mixed into the device-derived key. Changing this
/// invalidates all previously stored master keys. Must be exactly
/// [`crypto::SALT_LEN`] (32) bytes.
const APP_SALT: &[u8; crypto::SALT_LEN] = b"openintent-vault-keychain-v1\x00\x00\x00\x00";

/// File-based keychain that stores the master key encrypted with a
/// device-derived key.
///
/// The key file layout (binary):
/// ```text
/// [32 bytes: PBKDF2 salt]
/// [12 bytes: AES-256-GCM nonce]
/// [remaining: AES-256-GCM ciphertext + 16-byte tag]
/// ```
pub struct FileKeychain {
    /// Path to the encrypted master key file.
    key_file: PathBuf,
}

impl FileKeychain {
    /// Create a new file-based keychain that stores keys at `key_file`.
    ///
    /// The parent directory must exist. The file itself is created on
    /// [`set_master_key`](KeychainProvider::set_master_key).
    pub fn new(key_file: impl Into<PathBuf>) -> Self {
        Self {
            key_file: key_file.into(),
        }
    }

    /// Default key file location: `<vault_dir>/master.key`.
    pub fn default_path(vault_dir: &Path) -> PathBuf {
        vault_dir.join("master.key")
    }

    /// Derive an encryption key from machine-specific data.
    ///
    /// This combines the hostname, username, and an application salt to
    /// produce a deterministic 256-bit key that is unique per machine/user
    /// combination.
    fn device_derived_key(&self) -> Result<[u8; crypto::KEY_LEN]> {
        let hostname = Self::get_hostname();
        let username = std::env::var("USER")
            .or_else(|_| std::env::var("USERNAME"))
            .unwrap_or_else(|_| "unknown-user".into());

        // Combine machine identifiers with the application salt.
        let mut material = Vec::with_capacity(hostname.len() + username.len() + APP_SALT.len());
        material.extend_from_slice(hostname.as_bytes());
        material.extend_from_slice(username.as_bytes());
        material.extend_from_slice(APP_SALT);

        let mut key = [0u8; crypto::KEY_LEN];
        crypto::derive_key_with_salt(&material, APP_SALT, &mut key);

        Ok(key)
    }

    /// Get the system hostname using platform-specific APIs.
    ///
    /// Falls back to "unknown-host" if the hostname cannot be determined.
    fn get_hostname() -> String {
        #[cfg(unix)]
        {
            // Use POSIX gethostname via libc-free approach: read /etc/hostname
            // or fall back to the HOSTNAME environment variable.
            std::fs::read_to_string("/etc/hostname")
                .map(|s| s.trim().to_string())
                .or_else(|_| std::env::var("HOSTNAME"))
                .or_else(|_| std::env::var("HOST"))
                .unwrap_or_else(|_| "unknown-host".into())
        }

        #[cfg(not(unix))]
        {
            std::env::var("COMPUTERNAME")
                .or_else(|_| std::env::var("HOSTNAME"))
                .unwrap_or_else(|_| "unknown-host".into())
        }
    }
}

impl KeychainProvider for FileKeychain {
    fn get_master_key(&self) -> Result<Vec<u8>> {
        if !self.key_file.exists() {
            return Err(VaultError::MasterKeyNotFound);
        }

        let data = std::fs::read(&self.key_file)?;

        // Minimum size: salt (32) + nonce (12) + tag (16) = 60 bytes.
        if data.len() < crypto::SALT_LEN + crypto::NONCE_LEN_BYTES + 16 {
            return Err(VaultError::DecryptionFailed {
                reason: "master key file is too small / corrupted".into(),
            });
        }

        let device_key = self.device_derived_key()?;

        // Parse the file layout.
        let (_salt, rest) = data.split_at(crypto::SALT_LEN);
        let (nonce_bytes, ciphertext) = rest.split_at(crypto::NONCE_LEN_BYTES);

        let mut nonce = [0u8; crypto::NONCE_LEN_BYTES];
        nonce.copy_from_slice(nonce_bytes);

        let master_key = crypto::decrypt(&nonce, ciphertext, &device_key)?;

        tracing::debug!("retrieved master key from file keychain");
        Ok(master_key)
    }

    fn set_master_key(&self, key: &[u8]) -> Result<()> {
        let device_key = self.device_derived_key()?;

        let (nonce, ciphertext) = crypto::encrypt(key, &device_key)?;

        // Build the file: [salt][nonce][ciphertext+tag]
        let mut data =
            Vec::with_capacity(crypto::SALT_LEN + crypto::NONCE_LEN_BYTES + ciphertext.len());
        data.extend_from_slice(APP_SALT); // Store the salt used for device key derivation.
        data.extend_from_slice(&nonce);
        data.extend_from_slice(&ciphertext);

        // Ensure the parent directory exists.
        if let Some(parent) = self.key_file.parent() {
            std::fs::create_dir_all(parent)?;
        }

        std::fs::write(&self.key_file, &data)?;

        // Restrict file permissions on Unix (owner read/write only).
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let perms = std::fs::Permissions::from_mode(0o600);
            std::fs::set_permissions(&self.key_file, perms)?;
        }

        tracing::info!(path = %self.key_file.display(), "stored master key in file keychain");
        Ok(())
    }

    fn has_master_key(&self) -> Result<bool> {
        Ok(self.key_file.exists())
    }

    fn delete_master_key(&self) -> Result<()> {
        if self.key_file.exists() {
            std::fs::remove_file(&self.key_file)?;
            tracing::info!(path = %self.key_file.display(), "deleted master key from file keychain");
        }
        Ok(())
    }
}

// ---------------------------------------------------------------------------
// Platform-specific implementations (TODO)
// ---------------------------------------------------------------------------

// TODO: macOS Keychain Services implementation
//
// Use the `security-framework` crate to access the macOS Keychain.
// Store the master key as a generic password item with:
//   - Service: "com.openintentos.vault"
//   - Account: "master-key"
//
// pub struct MacOsKeychain;

// TODO: Windows DPAPI implementation
//
// Use the `windows` crate to call `CryptProtectData` / `CryptUnprotectData`.
// The master key is encrypted with a key tied to the current Windows user.
//
// pub struct WindowsDpapiKeychain;

// TODO: Linux Secret Service implementation
//
// Use the `secret-service` or `keyring` crate to store the master key via
// the D-Bus Secret Service API (GNOME Keyring / KDE Wallet).
//
// pub struct LinuxSecretServiceKeychain;

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    fn temp_key_file() -> PathBuf {
        let dir = std::env::temp_dir().join("openintent-vault-test");
        fs::create_dir_all(&dir).unwrap();
        dir.join(format!("test-master-{}.key", std::process::id()))
    }

    #[test]
    fn roundtrip_master_key() {
        let path = temp_key_file();
        let keychain = FileKeychain::new(&path);

        // Clean up from previous test runs.
        let _ = keychain.delete_master_key();

        assert!(!keychain.has_master_key().unwrap());

        let original_key = crypto::random_bytes(crypto::KEY_LEN).unwrap();
        keychain.set_master_key(&original_key).unwrap();

        assert!(keychain.has_master_key().unwrap());

        let retrieved = keychain.get_master_key().unwrap();
        assert_eq!(retrieved, original_key);

        // Clean up.
        keychain.delete_master_key().unwrap();
        assert!(!keychain.has_master_key().unwrap());
    }

    #[test]
    fn get_missing_key_returns_not_found() {
        let path = temp_key_file().with_extension("missing");
        let keychain = FileKeychain::new(&path);

        let result = keychain.get_master_key();
        assert!(matches!(result, Err(VaultError::MasterKeyNotFound)));
    }

    #[test]
    fn overwrite_master_key() {
        let path = temp_key_file().with_extension("overwrite");
        let keychain = FileKeychain::new(&path);
        let _ = keychain.delete_master_key();

        let key1 = crypto::random_bytes(crypto::KEY_LEN).unwrap();
        let key2 = crypto::random_bytes(crypto::KEY_LEN).unwrap();

        keychain.set_master_key(&key1).unwrap();
        keychain.set_master_key(&key2).unwrap();

        let retrieved = keychain.get_master_key().unwrap();
        assert_eq!(retrieved, key2);

        keychain.delete_master_key().unwrap();
    }
}
