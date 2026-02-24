//! AES-256-GCM encryption and decryption using the `ring` crate.
//!
//! This module provides the core cryptographic primitives for the vault:
//!
//! - **Encryption/decryption**: AES-256-GCM authenticated encryption with
//!   randomly generated 96-bit nonces.
//! - **Key derivation**: PBKDF2-HMAC-SHA256 to derive a 256-bit encryption
//!   key from a master password and a random salt.
//! - **Random generation**: Cryptographically secure random bytes via `ring`.
//!
//! # Security Notes
//!
//! - Nonces are generated randomly for each encryption operation. With a
//!   96-bit nonce and random generation, the probability of a collision is
//!   negligible for up to ~2^32 encryptions under the same key.
//! - PBKDF2 iteration count is set to 600,000 as recommended by OWASP (2023).
//! - Keys and sensitive material should be zeroized after use in production.
//!   This is left as a future enhancement (see `zeroize` crate).

use ring::aead::{self, Aad, BoundKey, NONCE_LEN, Nonce, NonceSequence, SealingKey, UnboundKey};
use ring::pbkdf2;
use ring::rand::{SecureRandom, SystemRandom};

use crate::error::{Result, VaultError};

/// Length of the AES-256-GCM key in bytes.
pub const KEY_LEN: usize = 32;

/// Length of the AES-256-GCM nonce in bytes (96 bits).
pub const NONCE_LEN_BYTES: usize = NONCE_LEN;

/// Length of the PBKDF2 salt in bytes.
pub const SALT_LEN: usize = 32;

/// PBKDF2 iteration count — 600,000 per OWASP 2023 recommendation for
/// HMAC-SHA256.
const PBKDF2_ITERATIONS: u32 = 600_000;

/// PBKDF2 algorithm: HMAC-SHA256.
static PBKDF2_ALG: pbkdf2::Algorithm = pbkdf2::PBKDF2_HMAC_SHA256;

/// AES-256-GCM algorithm from `ring`.
static AEAD_ALG: &aead::Algorithm = &aead::AES_256_GCM;

// ---------------------------------------------------------------------------
// Nonce handling
// ---------------------------------------------------------------------------

/// A single-use nonce sequence that yields exactly one nonce and then errors.
///
/// `ring` requires a [`NonceSequence`] for sealing operations. Since we
/// generate a fresh random nonce per encryption call, this wrapper ensures
/// each sealing key is used exactly once.
struct SingleNonce(Option<[u8; NONCE_LEN_BYTES]>);

impl SingleNonce {
    fn new(bytes: [u8; NONCE_LEN_BYTES]) -> Self {
        Self(Some(bytes))
    }
}

impl NonceSequence for SingleNonce {
    fn advance(&mut self) -> std::result::Result<Nonce, ring::error::Unspecified> {
        self.0
            .take()
            .map(Nonce::assume_unique_for_key)
            .ok_or(ring::error::Unspecified)
    }
}

// ---------------------------------------------------------------------------
// Encryption
// ---------------------------------------------------------------------------

/// Encrypt `plaintext` with AES-256-GCM using the given 256-bit `key`.
///
/// Returns `(nonce, ciphertext)` where `nonce` is a randomly generated 96-bit
/// value and `ciphertext` includes the 128-bit authentication tag appended by
/// `ring`.
///
/// # Errors
///
/// Returns [`VaultError::EncryptionFailed`] if the key length is wrong or
/// `ring` reports a failure.
pub fn encrypt(plaintext: &[u8], key: &[u8]) -> Result<([u8; NONCE_LEN_BYTES], Vec<u8>)> {
    if key.len() != KEY_LEN {
        return Err(VaultError::EncryptionFailed {
            reason: format!("key must be {} bytes, got {}", KEY_LEN, key.len()),
        });
    }

    let rng = SystemRandom::new();

    // Generate a random 96-bit nonce.
    let mut nonce_bytes = [0u8; NONCE_LEN_BYTES];
    rng.fill(&mut nonce_bytes)
        .map_err(|_| VaultError::EncryptionFailed {
            reason: "failed to generate random nonce".into(),
        })?;

    let unbound_key = UnboundKey::new(AEAD_ALG, key).map_err(|_| VaultError::EncryptionFailed {
        reason: "failed to create AES-256-GCM key".into(),
    })?;

    let mut sealing_key = SealingKey::new(unbound_key, SingleNonce::new(nonce_bytes));

    // `ring` encrypts in-place and appends the authentication tag.
    let mut in_out = plaintext.to_vec();
    sealing_key
        .seal_in_place_append_tag(Aad::empty(), &mut in_out)
        .map_err(|_| VaultError::EncryptionFailed {
            reason: "seal_in_place failed".into(),
        })?;

    tracing::trace!(
        plaintext_len = plaintext.len(),
        ciphertext_len = in_out.len(),
        "encrypted data"
    );

    Ok((nonce_bytes, in_out))
}

/// Decrypt `ciphertext` (which includes the GCM tag) using the given `nonce`
/// and 256-bit `key`.
///
/// Returns the decrypted plaintext.
///
/// # Errors
///
/// Returns [`VaultError::DecryptionFailed`] if the key is wrong, the
/// ciphertext has been tampered with, or the nonce does not match.
pub fn decrypt(nonce: &[u8; NONCE_LEN_BYTES], ciphertext: &[u8], key: &[u8]) -> Result<Vec<u8>> {
    if key.len() != KEY_LEN {
        return Err(VaultError::DecryptionFailed {
            reason: format!("key must be {} bytes, got {}", KEY_LEN, key.len()),
        });
    }

    let unbound_key = UnboundKey::new(AEAD_ALG, key).map_err(|_| VaultError::DecryptionFailed {
        reason: "failed to create AES-256-GCM key".into(),
    })?;

    let mut opening_key = aead::OpeningKey::new(unbound_key, SingleNonce::new(*nonce));

    let mut in_out = ciphertext.to_vec();
    let plaintext = opening_key
        .open_in_place(Aad::empty(), &mut in_out)
        .map_err(|_| VaultError::DecryptionFailed {
            reason: "authentication failed — wrong key or corrupted data".into(),
        })?;

    let result = plaintext.to_vec();

    tracing::trace!(
        ciphertext_len = ciphertext.len(),
        plaintext_len = result.len(),
        "decrypted data"
    );

    Ok(result)
}

// ---------------------------------------------------------------------------
// Key derivation
// ---------------------------------------------------------------------------

/// Derive a 256-bit encryption key from a `password` using PBKDF2-HMAC-SHA256.
///
/// A random 256-bit `salt` is generated and returned alongside the derived
/// key. The caller must store the salt to re-derive the same key later.
///
/// # Errors
///
/// Returns [`VaultError::KeyDerivationFailed`] if random salt generation
/// fails.
pub fn derive_key_from_password(password: &[u8]) -> Result<([u8; SALT_LEN], [u8; KEY_LEN])> {
    let rng = SystemRandom::new();

    let mut salt = [0u8; SALT_LEN];
    rng.fill(&mut salt)
        .map_err(|_| VaultError::KeyDerivationFailed {
            reason: "failed to generate random salt".into(),
        })?;

    let mut key = [0u8; KEY_LEN];
    derive_key_with_salt(password, &salt, &mut key);

    tracing::debug!("derived encryption key from password via PBKDF2");

    Ok((salt, key))
}

/// Derive a 256-bit encryption key from a `password` and a known `salt`.
///
/// This is the deterministic counterpart of [`derive_key_from_password`],
/// used when the salt was previously stored.
pub fn derive_key_with_salt(password: &[u8], salt: &[u8], out: &mut [u8; KEY_LEN]) {
    let iterations =
        std::num::NonZeroU32::new(PBKDF2_ITERATIONS).expect("PBKDF2_ITERATIONS is non-zero");
    pbkdf2::derive(PBKDF2_ALG, iterations, salt, password, out);
}

/// Verify that a `password` produces the same key as the stored `salt` and
/// `expected_key`.
///
/// Returns `true` if the password matches, `false` otherwise. Uses
/// constant-time comparison internally (via `ring`).
pub fn verify_password(password: &[u8], salt: &[u8], expected_key: &[u8]) -> bool {
    let iterations =
        std::num::NonZeroU32::new(PBKDF2_ITERATIONS).expect("PBKDF2_ITERATIONS is non-zero");
    pbkdf2::verify(PBKDF2_ALG, iterations, salt, password, expected_key).is_ok()
}

// ---------------------------------------------------------------------------
// Random bytes
// ---------------------------------------------------------------------------

/// Generate `len` cryptographically secure random bytes.
///
/// # Errors
///
/// Returns [`VaultError::Internal`] if the system CSPRNG fails.
pub fn random_bytes(len: usize) -> Result<Vec<u8>> {
    let rng = SystemRandom::new();
    let mut buf = vec![0u8; len];
    rng.fill(&mut buf)
        .map_err(|_| VaultError::Internal("failed to generate random bytes".into()))?;
    Ok(buf)
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn encrypt_decrypt_roundtrip() {
        let key = random_bytes(KEY_LEN).unwrap();
        let plaintext = b"hello, OpenIntentOS vault!";

        let (nonce, ciphertext) = encrypt(plaintext, &key).unwrap();
        let decrypted = decrypt(&nonce, &ciphertext, &key).unwrap();

        assert_eq!(decrypted, plaintext);
    }

    #[test]
    fn decrypt_with_wrong_key_fails() {
        let key1 = random_bytes(KEY_LEN).unwrap();
        let key2 = random_bytes(KEY_LEN).unwrap();
        let plaintext = b"secret data";

        let (nonce, ciphertext) = encrypt(plaintext, &key1).unwrap();
        let result = decrypt(&nonce, &ciphertext, &key2);

        assert!(result.is_err());
    }

    #[test]
    fn decrypt_with_tampered_ciphertext_fails() {
        let key = random_bytes(KEY_LEN).unwrap();
        let plaintext = b"secret data";

        let (nonce, mut ciphertext) = encrypt(plaintext, &key).unwrap();
        // Flip a bit in the ciphertext.
        if let Some(byte) = ciphertext.first_mut() {
            *byte ^= 0x01;
        }

        let result = decrypt(&nonce, &ciphertext, &key);
        assert!(result.is_err());
    }

    #[test]
    fn invalid_key_length_rejected() {
        let short_key = vec![0u8; 16]; // AES-128, not AES-256
        let result = encrypt(b"test", &short_key);
        assert!(result.is_err());
    }

    #[test]
    fn pbkdf2_derive_and_verify() {
        let password = b"correct horse battery staple";

        let (salt, key) = derive_key_from_password(password).unwrap();

        assert!(verify_password(password, &salt, &key));
        assert!(!verify_password(b"wrong password", &salt, &key));
    }

    #[test]
    fn pbkdf2_deterministic_with_same_salt() {
        let password = b"my-password";
        let (salt, key1) = derive_key_from_password(password).unwrap();

        let mut key2 = [0u8; KEY_LEN];
        derive_key_with_salt(password, &salt, &mut key2);

        assert_eq!(key1, key2);
    }

    #[test]
    fn empty_plaintext_roundtrip() {
        let key = random_bytes(KEY_LEN).unwrap();
        let plaintext = b"";

        let (nonce, ciphertext) = encrypt(plaintext, &key).unwrap();
        let decrypted = decrypt(&nonce, &ciphertext, &key).unwrap();

        assert_eq!(decrypted, plaintext);
    }

    #[test]
    fn large_plaintext_roundtrip() {
        let key = random_bytes(KEY_LEN).unwrap();
        let plaintext = vec![0xAB_u8; 1_000_000]; // 1 MB

        let (nonce, ciphertext) = encrypt(&plaintext, &key).unwrap();
        let decrypted = decrypt(&nonce, &ciphertext, &key).unwrap();

        assert_eq!(decrypted, plaintext);
    }
}
