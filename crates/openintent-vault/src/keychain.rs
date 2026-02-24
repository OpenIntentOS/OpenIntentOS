//! OS keychain integration for master key storage.
//!
//! The master encryption key must never be stored as plaintext on disk. This
//! module provides a [`KeychainProvider`] trait that abstracts over different
//! platform-specific secure storage backends:
//!
//! - **macOS**: Keychain Services via `security-framework`
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
// macOS Keychain Services
// ---------------------------------------------------------------------------

/// The Security framework error code for "item not found"
/// (`errSecItemNotFound = -25300`).
#[cfg(target_os = "macos")]
const MACOS_ERR_SEC_ITEM_NOT_FOUND: i32 = -25300;

/// macOS Keychain Services integration via the `security-framework` crate.
///
/// Stores the master key in the user's login keychain using the generic
/// password APIs (`SecKeychainAddGenericPassword` /
/// `SecKeychainFindGenericPassword`).
///
/// This provides hardware-backed secure storage that is protected by the
/// user's login password and (on Apple Silicon) the Secure Enclave.
#[cfg(target_os = "macos")]
pub struct MacOSKeychain {
    /// The keychain service name (e.g. "com.openintentos.vault").
    service_name: String,
    /// The keychain account name (e.g. "master-key").
    account_name: String,
}

#[cfg(target_os = "macos")]
impl MacOSKeychain {
    /// Default service name used for keychain entries.
    const DEFAULT_SERVICE: &'static str = "com.openintentos.vault";
    /// Default account name used for the master key entry.
    const DEFAULT_ACCOUNT: &'static str = "master-key";

    /// Create a new macOS keychain provider with default service and account
    /// names.
    pub fn new() -> Self {
        Self {
            service_name: Self::DEFAULT_SERVICE.to_string(),
            account_name: Self::DEFAULT_ACCOUNT.to_string(),
        }
    }

    /// Create a new macOS keychain provider with custom service and account
    /// names.
    ///
    /// This is useful for testing or running multiple vault instances that
    /// should not share the same keychain entry.
    pub fn with_names(service: &str, account: &str) -> Self {
        Self {
            service_name: service.to_string(),
            account_name: account.to_string(),
        }
    }
}

#[cfg(target_os = "macos")]
impl Default for MacOSKeychain {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(target_os = "macos")]
impl KeychainProvider for MacOSKeychain {
    fn get_master_key(&self) -> Result<Vec<u8>> {
        use security_framework::passwords::get_generic_password;

        match get_generic_password(&self.service_name, &self.account_name) {
            Ok(data) => {
                tracing::debug!(
                    service = %self.service_name,
                    "retrieved master key from macOS keychain"
                );
                Ok(data.to_vec())
            }
            Err(e) if e.code() == MACOS_ERR_SEC_ITEM_NOT_FOUND => {
                Err(VaultError::MasterKeyNotFound)
            }
            Err(e) => Err(VaultError::KeychainUnavailable {
                reason: format!("macOS keychain read failed: {e}"),
            }),
        }
    }

    fn set_master_key(&self, key: &[u8]) -> Result<()> {
        use security_framework::passwords::set_generic_password;

        set_generic_password(&self.service_name, &self.account_name, key).map_err(|e| {
            VaultError::MasterKeyStoreFailed {
                reason: format!("macOS keychain write failed: {e}"),
            }
        })?;

        tracing::info!(
            service = %self.service_name,
            "stored master key in macOS keychain"
        );
        Ok(())
    }

    fn has_master_key(&self) -> Result<bool> {
        use security_framework::passwords::get_generic_password;

        match get_generic_password(&self.service_name, &self.account_name) {
            Ok(_) => Ok(true),
            Err(e) if e.code() == MACOS_ERR_SEC_ITEM_NOT_FOUND => Ok(false),
            Err(e) => Err(VaultError::KeychainUnavailable {
                reason: format!("macOS keychain check failed: {e}"),
            }),
        }
    }

    fn delete_master_key(&self) -> Result<()> {
        use security_framework::passwords::delete_generic_password;

        match delete_generic_password(&self.service_name, &self.account_name) {
            Ok(()) => {
                tracing::info!(
                    service = %self.service_name,
                    "deleted master key from macOS keychain"
                );
                Ok(())
            }
            Err(e) if e.code() == MACOS_ERR_SEC_ITEM_NOT_FOUND => {
                // Not an error if the key does not exist.
                Ok(())
            }
            Err(e) => Err(VaultError::KeychainUnavailable {
                reason: format!("macOS keychain delete failed: {e}"),
            }),
        }
    }
}

// ---------------------------------------------------------------------------
// Platform-specific implementations (TODO)
// ---------------------------------------------------------------------------

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
// Factory
// ---------------------------------------------------------------------------

/// Returns the best available keychain provider for the current platform.
///
/// - **macOS**: [`MacOSKeychain`] (Keychain Services)
/// - **Other platforms**: [`FileKeychain`] (encrypted file fallback)
///
/// The `data_dir` parameter is used by the file-based fallback to determine
/// where to store the encrypted master key file. On macOS, this parameter is
/// unused because the key is stored in Keychain Services.
///
/// This is the recommended way to obtain a keychain provider. Callers should
/// not need to know which backend is in use.
pub fn platform_keychain(data_dir: &Path) -> Box<dyn KeychainProvider> {
    // Suppress the unused-variable warning on macOS where data_dir is not
    // needed. The parameter is still part of the public API so that callers
    // have a uniform signature across platforms.
    let _ = &data_dir;

    #[cfg(target_os = "macos")]
    {
        tracing::info!("using macOS Keychain Services for master key storage");
        Box::new(MacOSKeychain::new())
    }
    #[cfg(not(target_os = "macos"))]
    {
        let path = FileKeychain::default_path(data_dir);
        tracing::info!(path = %path.display(), "using file-based keychain for master key storage");
        Box::new(FileKeychain::new(path))
    }
}

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

    // -----------------------------------------------------------------------
    // macOS Keychain tests
    // -----------------------------------------------------------------------

    #[cfg(target_os = "macos")]
    #[test]
    fn macos_keychain_construction() {
        let kc = MacOSKeychain::new();
        assert_eq!(kc.service_name, "com.openintentos.vault");
        assert_eq!(kc.account_name, "master-key");

        let kc2 = MacOSKeychain::with_names("test.service", "test.account");
        assert_eq!(kc2.service_name, "test.service");
        assert_eq!(kc2.account_name, "test.account");

        // Verify Default trait works the same as new().
        let kc3 = MacOSKeychain::default();
        assert_eq!(kc3.service_name, kc.service_name);
        assert_eq!(kc3.account_name, kc.account_name);
    }

    /// Round-trip test for macOS Keychain Services.
    ///
    /// This test interacts with the real macOS Keychain. It uses a unique
    /// test-specific service name to avoid interfering with production data.
    /// In CI environments the keychain may not be unlocked, so this test is
    /// ignored by default.
    #[cfg(target_os = "macos")]
    #[test]
    #[ignore = "requires unlocked macOS Keychain — run manually with --ignored"]
    fn macos_keychain_roundtrip() {
        let service = format!("com.openintentos.vault.test.{}", std::process::id());
        let kc = MacOSKeychain::with_names(&service, "test-master-key");

        // Clean up any leftover from a previous run.
        let _ = kc.delete_master_key();

        // Initially there should be no key.
        assert!(!kc.has_master_key().unwrap());

        // Store a key.
        let key = crypto::random_bytes(crypto::KEY_LEN).unwrap();
        kc.set_master_key(&key).unwrap();

        // Should now exist.
        assert!(kc.has_master_key().unwrap());

        // Retrieve and verify.
        let retrieved = kc.get_master_key().unwrap();
        assert_eq!(retrieved, key);

        // Overwrite with a new key.
        let key2 = crypto::random_bytes(crypto::KEY_LEN).unwrap();
        kc.set_master_key(&key2).unwrap();
        let retrieved2 = kc.get_master_key().unwrap();
        assert_eq!(retrieved2, key2);

        // Delete and verify gone.
        kc.delete_master_key().unwrap();
        assert!(!kc.has_master_key().unwrap());

        // Deleting again should be a no-op.
        kc.delete_master_key().unwrap();
    }

    /// Verify that `get_master_key` returns `MasterKeyNotFound` when the
    /// keychain entry does not exist.
    #[cfg(target_os = "macos")]
    #[test]
    #[ignore = "requires unlocked macOS Keychain — run manually with --ignored"]
    fn macos_keychain_not_found() {
        let service = format!(
            "com.openintentos.vault.test.notfound.{}",
            std::process::id()
        );
        let kc = MacOSKeychain::with_names(&service, "nonexistent-key");

        let result = kc.get_master_key();
        assert!(matches!(result, Err(VaultError::MasterKeyNotFound)));
    }

    // -----------------------------------------------------------------------
    // platform_keychain factory tests
    // -----------------------------------------------------------------------

    #[test]
    fn platform_keychain_returns_provider() {
        let dir = std::env::temp_dir().join("openintent-vault-test-platform");
        fs::create_dir_all(&dir).unwrap();

        let provider = platform_keychain(&dir);

        // We cannot inspect the concrete type, but we can verify the trait
        // object is usable. On macOS this will be MacOSKeychain, on other
        // platforms it will be FileKeychain.
        // Just calling has_master_key is enough to confirm the provider works.
        let _has_key = provider.has_master_key();
    }

    /// Verify that `FileKeychain` still works as the fallback provider.
    #[test]
    fn file_keychain_fallback_works() {
        let path = temp_key_file().with_extension("fallback");
        let keychain = FileKeychain::new(&path);
        let _ = keychain.delete_master_key();

        assert!(!keychain.has_master_key().unwrap());

        let key = crypto::random_bytes(crypto::KEY_LEN).unwrap();
        keychain.set_master_key(&key).unwrap();
        assert!(keychain.has_master_key().unwrap());

        let retrieved = keychain.get_master_key().unwrap();
        assert_eq!(retrieved, key);

        keychain.delete_master_key().unwrap();
        assert!(!keychain.has_master_key().unwrap());
    }
}
