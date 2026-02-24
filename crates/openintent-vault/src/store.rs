//! SQLite-backed encrypted credential store.
//!
//! The [`Vault`] struct wraps a `rusqlite::Connection` and a master encryption
//! key. All credential data is encrypted with AES-256-GCM before being written
//! to SQLite and decrypted on read.
//!
//! # Schema
//!
//! The vault database (`vault.db`) contains three tables:
//!
//! - `credentials` — encrypted credential blobs keyed by provider name.
//! - `policies` — per-provider permission rules.
//! - `audit_log` — immutable record of every vault access.
//!
//! Schema migration is automatic: calling [`Vault::open`] creates or upgrades
//! the database as needed.

use chrono::{DateTime, Utc};
use rusqlite::{Connection, OptionalExtension, params};
use serde::{Deserialize, Serialize};

use crate::crypto;
use crate::error::{Result, VaultError};

// ---------------------------------------------------------------------------
// Types
// ---------------------------------------------------------------------------

/// The type of credential stored in the vault.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CredentialType {
    /// OAuth2 access/refresh token pair.
    OAuth,
    /// Static API key or bearer token.
    ApiKey,
    /// Browser cookie or session token.
    Cookie,
    /// OS keychain reference (meta-credential).
    Keychain,
}

impl CredentialType {
    /// Convert to the string stored in SQLite.
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::OAuth => "oauth",
            Self::ApiKey => "api_key",
            Self::Cookie => "cookie",
            Self::Keychain => "keychain",
        }
    }

    /// Parse from the string stored in SQLite.
    pub fn parse(s: &str) -> Option<Self> {
        match s {
            "oauth" => Some(Self::OAuth),
            "api_key" => Some(Self::ApiKey),
            "cookie" => Some(Self::Cookie),
            "keychain" => Some(Self::Keychain),
            _ => None,
        }
    }
}

impl std::fmt::Display for CredentialType {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

/// A credential stored in the vault.
///
/// The `data` field contains the sensitive material (tokens, keys, etc.)
/// serialized as JSON. It is always encrypted at rest and only decrypted
/// in memory when returned by [`Vault::get_credential`].
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Credential {
    /// The service provider name (e.g. "github", "slack", "anthropic").
    pub provider: String,

    /// The type of credential.
    pub credential_type: CredentialType,

    /// The decrypted credential data as a JSON value.
    ///
    /// For OAuth: `{ "access_token": "...", "refresh_token": "...", "token_type": "Bearer" }`
    /// For ApiKey: `{ "api_key": "sk-..." }`
    /// For Cookie: `{ "cookies": [...] }`
    pub data: serde_json::Value,

    /// OAuth scopes or permission list.
    pub scopes: Option<Vec<String>>,

    /// Human-readable label (e.g. "work account", "personal").
    pub user_label: Option<String>,

    /// When this credential expires (if applicable).
    pub expires_at: Option<DateTime<Utc>>,

    /// When this credential was first stored.
    pub created_at: DateTime<Utc>,

    /// When this credential was last updated.
    pub updated_at: DateTime<Utc>,
}

/// Summary of a stored credential (without the decrypted data).
///
/// Returned by [`Vault::list_credentials`] to avoid decrypting every
/// credential just to list them.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CredentialSummary {
    pub provider: String,
    pub credential_type: CredentialType,
    pub scopes: Option<Vec<String>>,
    pub user_label: Option<String>,
    pub expires_at: Option<DateTime<Utc>>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

// ---------------------------------------------------------------------------
// Vault
// ---------------------------------------------------------------------------

/// Encrypted credential vault backed by SQLite.
///
/// # Example
///
/// ```rust,no_run
/// # use openintent_vault::store::{Vault, CredentialType};
/// # fn example() -> openintent_vault::error::Result<()> {
/// # let master_key = [0u8; 32];
/// let vault = Vault::open("data/vault.db", &master_key)?;
///
/// vault.store_credential(
///     "anthropic",
///     CredentialType::ApiKey,
///     &serde_json::json!({ "api_key": "sk-ant-..." }),
///     None,
///     Some("work"),
///     None,
/// )?;
///
/// let cred = vault.get_credential("anthropic")?;
/// println!("key = {}", cred.data["api_key"]);
/// # Ok(())
/// # }
/// ```
pub struct Vault {
    conn: Connection,
    master_key: Vec<u8>,
}

impl Vault {
    /// Open (or create) a vault database at `path` with the given `master_key`.
    ///
    /// Runs schema migrations automatically.
    ///
    /// # Errors
    ///
    /// Returns [`VaultError::Database`] if the database cannot be opened,
    /// or [`VaultError::MigrationFailed`] if schema setup fails.
    pub fn open(path: impl AsRef<std::path::Path>, master_key: &[u8]) -> Result<Self> {
        let path = path.as_ref();
        tracing::info!(path = %path.display(), "opening vault database");

        let conn = Connection::open(path)?;
        Self::configure_connection(&conn)?;

        let vault = Self {
            conn,
            master_key: master_key.to_vec(),
        };

        vault.run_migrations()?;

        tracing::info!("vault database ready");
        Ok(vault)
    }

    /// Open an in-memory vault (useful for testing).
    pub fn open_in_memory(master_key: &[u8]) -> Result<Self> {
        let conn = Connection::open_in_memory()?;
        Self::configure_connection(&conn)?;

        let vault = Self {
            conn,
            master_key: master_key.to_vec(),
        };

        vault.run_migrations()?;
        Ok(vault)
    }

    /// Configure SQLite pragmas for performance and safety.
    fn configure_connection(conn: &Connection) -> Result<()> {
        conn.execute_batch(
            "PRAGMA journal_mode = WAL;
             PRAGMA synchronous = NORMAL;
             PRAGMA foreign_keys = ON;
             PRAGMA temp_store = MEMORY;
             PRAGMA cache_size = -8000;",
        )?;
        Ok(())
    }

    /// Run database schema migrations.
    fn run_migrations(&self) -> Result<()> {
        tracing::debug!("running vault schema migrations");

        self.conn
            .execute_batch(
                "CREATE TABLE IF NOT EXISTS credentials (
                provider   TEXT PRIMARY KEY,
                type       TEXT NOT NULL CHECK(type IN ('oauth','api_key','cookie','keychain')),
                data       BLOB NOT NULL,
                nonce      BLOB NOT NULL,
                scopes     TEXT,
                user_label TEXT,
                expires_at INTEGER,
                created_at INTEGER NOT NULL,
                updated_at INTEGER NOT NULL
            );

            CREATE TABLE IF NOT EXISTS policies (
                id         INTEGER PRIMARY KEY AUTOINCREMENT,
                provider   TEXT NOT NULL,
                action     TEXT NOT NULL,
                resource   TEXT NOT NULL DEFAULT '*',
                decision   TEXT NOT NULL CHECK(decision IN ('allow','confirm','deny')),
                rate_limit INTEGER,
                created_at INTEGER NOT NULL
            );

            CREATE TABLE IF NOT EXISTS audit_log (
                id        INTEGER PRIMARY KEY AUTOINCREMENT,
                provider  TEXT NOT NULL,
                action    TEXT NOT NULL,
                resource  TEXT,
                decision  TEXT NOT NULL,
                detail    TEXT,
                timestamp INTEGER NOT NULL
            );

            CREATE INDEX IF NOT EXISTS idx_audit_timestamp ON audit_log(timestamp);
            CREATE INDEX IF NOT EXISTS idx_policies_provider ON policies(provider);",
            )
            .map_err(|e| VaultError::MigrationFailed {
                reason: e.to_string(),
            })?;

        tracing::debug!("vault schema migrations complete");
        Ok(())
    }

    // -- Credential CRUD ----------------------------------------------------

    /// Store a new credential in the vault.
    ///
    /// The `data` JSON value is encrypted before storage. If a credential for
    /// the same provider already exists, use [`update_credential`] instead.
    ///
    /// # Errors
    ///
    /// Returns [`VaultError::CredentialAlreadyExists`] if the provider is
    /// already present.
    pub fn store_credential(
        &self,
        provider: &str,
        credential_type: CredentialType,
        data: &serde_json::Value,
        scopes: Option<&[String]>,
        user_label: Option<&str>,
        expires_at: Option<DateTime<Utc>>,
    ) -> Result<()> {
        // Check for existing credential.
        let exists: bool = self.conn.query_row(
            "SELECT EXISTS(SELECT 1 FROM credentials WHERE provider = ?1)",
            params![provider],
            |row| row.get(0),
        )?;

        if exists {
            return Err(VaultError::CredentialAlreadyExists {
                provider: provider.to_string(),
            });
        }

        let plaintext = serde_json::to_vec(data)?;
        let (nonce, ciphertext) = crypto::encrypt(&plaintext, &self.master_key)?;
        let scopes_json = scopes.map(serde_json::to_string).transpose()?;
        let now = Utc::now().timestamp();
        let expires_ts = expires_at.map(|e| e.timestamp());

        self.conn.execute(
            "INSERT INTO credentials (provider, type, data, nonce, scopes, user_label, expires_at, created_at, updated_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)",
            params![
                provider,
                credential_type.as_str(),
                ciphertext,
                nonce.as_slice(),
                scopes_json,
                user_label,
                expires_ts,
                now,
                now,
            ],
        )?;

        tracing::info!(
            provider = provider,
            credential_type = %credential_type,
            "stored credential"
        );

        Ok(())
    }

    /// Retrieve and decrypt a credential by provider name.
    ///
    /// # Errors
    ///
    /// Returns [`VaultError::CredentialNotFound`] if no credential exists for
    /// the given provider.
    pub fn get_credential(&self, provider: &str) -> Result<Credential> {
        let row = self.conn.query_row(
            "SELECT provider, type, data, nonce, scopes, user_label, expires_at, created_at, updated_at
             FROM credentials WHERE provider = ?1",
            params![provider],
            |row| {
                Ok(CredentialRow {
                    provider: row.get(0)?,
                    credential_type: row.get::<_, String>(1)?,
                    data: row.get::<_, Vec<u8>>(2)?,
                    nonce: row.get::<_, Vec<u8>>(3)?,
                    scopes: row.get::<_, Option<String>>(4)?,
                    user_label: row.get::<_, Option<String>>(5)?,
                    expires_at: row.get::<_, Option<i64>>(6)?,
                    created_at: row.get::<_, i64>(7)?,
                    updated_at: row.get::<_, i64>(8)?,
                })
            },
        ).optional()?;

        let row = row.ok_or_else(|| VaultError::CredentialNotFound {
            provider: provider.to_string(),
        })?;

        self.decrypt_credential_row(row)
    }

    /// Update an existing credential's data (re-encrypts with a fresh nonce).
    ///
    /// # Errors
    ///
    /// Returns [`VaultError::CredentialNotFound`] if no credential exists for
    /// the given provider.
    pub fn update_credential(
        &self,
        provider: &str,
        data: &serde_json::Value,
        expires_at: Option<DateTime<Utc>>,
    ) -> Result<()> {
        let plaintext = serde_json::to_vec(data)?;
        let (nonce, ciphertext) = crypto::encrypt(&plaintext, &self.master_key)?;
        let now = Utc::now().timestamp();
        let expires_ts = expires_at.map(|e| e.timestamp());

        let rows = self.conn.execute(
            "UPDATE credentials SET data = ?1, nonce = ?2, expires_at = ?3, updated_at = ?4
             WHERE provider = ?5",
            params![ciphertext, nonce.as_slice(), expires_ts, now, provider],
        )?;

        if rows == 0 {
            return Err(VaultError::CredentialNotFound {
                provider: provider.to_string(),
            });
        }

        tracing::info!(provider = provider, "updated credential");
        Ok(())
    }

    /// Delete a credential by provider name.
    ///
    /// # Errors
    ///
    /// Returns [`VaultError::CredentialNotFound`] if no credential exists for
    /// the given provider.
    pub fn delete_credential(&self, provider: &str) -> Result<()> {
        let rows = self.conn.execute(
            "DELETE FROM credentials WHERE provider = ?1",
            params![provider],
        )?;

        if rows == 0 {
            return Err(VaultError::CredentialNotFound {
                provider: provider.to_string(),
            });
        }

        tracing::info!(provider = provider, "deleted credential");
        Ok(())
    }

    /// List all stored credentials without decrypting their data.
    pub fn list_credentials(&self) -> Result<Vec<CredentialSummary>> {
        let mut stmt = self.conn.prepare(
            "SELECT provider, type, scopes, user_label, expires_at, created_at, updated_at
             FROM credentials ORDER BY provider",
        )?;

        let rows = stmt.query_map([], |row| {
            Ok(CredentialSummaryRow {
                provider: row.get(0)?,
                credential_type: row.get::<_, String>(1)?,
                scopes: row.get::<_, Option<String>>(2)?,
                user_label: row.get::<_, Option<String>>(3)?,
                expires_at: row.get::<_, Option<i64>>(4)?,
                created_at: row.get::<_, i64>(5)?,
                updated_at: row.get::<_, i64>(6)?,
            })
        })?;

        let mut summaries = Vec::new();
        for row in rows {
            let row = row?;
            summaries.push(CredentialSummary {
                provider: row.provider,
                credential_type: CredentialType::parse(&row.credential_type)
                    .unwrap_or(CredentialType::ApiKey),
                scopes: row
                    .scopes
                    .as_deref()
                    .map(serde_json::from_str)
                    .transpose()
                    .map_err(|e| VaultError::Internal(format!("bad scopes JSON: {e}")))?,
                user_label: row.user_label,
                expires_at: row
                    .expires_at
                    .and_then(|ts| DateTime::from_timestamp(ts, 0)),
                created_at: DateTime::from_timestamp(row.created_at, 0).unwrap_or_default(),
                updated_at: DateTime::from_timestamp(row.updated_at, 0).unwrap_or_default(),
            });
        }

        tracing::debug!(count = summaries.len(), "listed credentials");
        Ok(summaries)
    }

    /// Get a reference to the underlying database connection (for policy and
    /// audit operations).
    pub(crate) fn connection(&self) -> &Connection {
        &self.conn
    }

    // -- Internal helpers ---------------------------------------------------

    /// Decrypt a raw credential row into a [`Credential`].
    fn decrypt_credential_row(&self, row: CredentialRow) -> Result<Credential> {
        if row.nonce.len() != crypto::NONCE_LEN_BYTES {
            return Err(VaultError::DecryptionFailed {
                reason: format!(
                    "stored nonce is {} bytes, expected {}",
                    row.nonce.len(),
                    crypto::NONCE_LEN_BYTES
                ),
            });
        }

        let mut nonce = [0u8; crypto::NONCE_LEN_BYTES];
        nonce.copy_from_slice(&row.nonce);

        let plaintext = crypto::decrypt(&nonce, &row.data, &self.master_key)?;
        let data: serde_json::Value = serde_json::from_slice(&plaintext)?;

        let scopes: Option<Vec<String>> = row
            .scopes
            .as_deref()
            .map(serde_json::from_str)
            .transpose()
            .map_err(|e| VaultError::Internal(format!("bad scopes JSON: {e}")))?;

        Ok(Credential {
            provider: row.provider,
            credential_type: CredentialType::parse(&row.credential_type)
                .unwrap_or(CredentialType::ApiKey),
            data,
            scopes,
            user_label: row.user_label,
            expires_at: row
                .expires_at
                .and_then(|ts| DateTime::from_timestamp(ts, 0)),
            created_at: DateTime::from_timestamp(row.created_at, 0).unwrap_or_default(),
            updated_at: DateTime::from_timestamp(row.updated_at, 0).unwrap_or_default(),
        })
    }
}

// ---------------------------------------------------------------------------
// Internal row types (avoid leaking rusqlite details)
// ---------------------------------------------------------------------------

struct CredentialRow {
    provider: String,
    credential_type: String,
    data: Vec<u8>,
    nonce: Vec<u8>,
    scopes: Option<String>,
    user_label: Option<String>,
    expires_at: Option<i64>,
    created_at: i64,
    updated_at: i64,
}

struct CredentialSummaryRow {
    provider: String,
    credential_type: String,
    scopes: Option<String>,
    user_label: Option<String>,
    expires_at: Option<i64>,
    created_at: i64,
    updated_at: i64,
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    fn test_vault() -> Vault {
        let key = crypto::random_bytes(crypto::KEY_LEN).unwrap();
        Vault::open_in_memory(&key).unwrap()
    }

    #[test]
    fn store_and_retrieve_credential() {
        let vault = test_vault();
        let data = serde_json::json!({ "api_key": "sk-test-12345" });

        vault
            .store_credential(
                "anthropic",
                CredentialType::ApiKey,
                &data,
                None,
                Some("work"),
                None,
            )
            .unwrap();

        let cred = vault.get_credential("anthropic").unwrap();
        assert_eq!(cred.provider, "anthropic");
        assert_eq!(cred.credential_type, CredentialType::ApiKey);
        assert_eq!(cred.data["api_key"], "sk-test-12345");
        assert_eq!(cred.user_label.as_deref(), Some("work"));
    }

    #[test]
    fn duplicate_provider_rejected() {
        let vault = test_vault();
        let data = serde_json::json!({ "api_key": "key1" });

        vault
            .store_credential("github", CredentialType::ApiKey, &data, None, None, None)
            .unwrap();

        let result =
            vault.store_credential("github", CredentialType::ApiKey, &data, None, None, None);
        assert!(matches!(
            result,
            Err(VaultError::CredentialAlreadyExists { .. })
        ));
    }

    #[test]
    fn update_credential_data() {
        let vault = test_vault();
        let data1 = serde_json::json!({ "api_key": "old-key" });
        let data2 = serde_json::json!({ "api_key": "new-key" });

        vault
            .store_credential("slack", CredentialType::ApiKey, &data1, None, None, None)
            .unwrap();

        vault.update_credential("slack", &data2, None).unwrap();

        let cred = vault.get_credential("slack").unwrap();
        assert_eq!(cred.data["api_key"], "new-key");
    }

    #[test]
    fn delete_credential() {
        let vault = test_vault();
        let data = serde_json::json!({ "api_key": "key" });

        vault
            .store_credential("notion", CredentialType::ApiKey, &data, None, None, None)
            .unwrap();

        vault.delete_credential("notion").unwrap();

        let result = vault.get_credential("notion");
        assert!(matches!(result, Err(VaultError::CredentialNotFound { .. })));
    }

    #[test]
    fn delete_missing_credential_errors() {
        let vault = test_vault();
        let result = vault.delete_credential("nonexistent");
        assert!(matches!(result, Err(VaultError::CredentialNotFound { .. })));
    }

    #[test]
    fn list_credentials_returns_summaries() {
        let vault = test_vault();

        vault
            .store_credential(
                "github",
                CredentialType::OAuth,
                &serde_json::json!({ "access_token": "gho_xxx" }),
                Some(&["repo".to_string(), "user".to_string()]),
                Some("personal"),
                None,
            )
            .unwrap();

        vault
            .store_credential(
                "anthropic",
                CredentialType::ApiKey,
                &serde_json::json!({ "api_key": "sk-ant-xxx" }),
                None,
                Some("work"),
                None,
            )
            .unwrap();

        let list = vault.list_credentials().unwrap();
        assert_eq!(list.len(), 2);

        // Sorted by provider name.
        assert_eq!(list[0].provider, "anthropic");
        assert_eq!(list[1].provider, "github");
        assert_eq!(list[1].scopes.as_ref().unwrap().len(), 2);
    }

    #[test]
    fn get_missing_credential_errors() {
        let vault = test_vault();
        let result = vault.get_credential("nonexistent");
        assert!(matches!(result, Err(VaultError::CredentialNotFound { .. })));
    }

    #[test]
    fn oauth_credential_with_scopes_and_expiry() {
        let vault = test_vault();
        let expires = Utc::now() + chrono::Duration::hours(1);
        let data = serde_json::json!({
            "access_token": "gho_xxx",
            "refresh_token": "ghr_yyy",
            "token_type": "Bearer"
        });

        vault
            .store_credential(
                "github",
                CredentialType::OAuth,
                &data,
                Some(&["repo".to_string(), "user:email".to_string()]),
                Some("work"),
                Some(expires),
            )
            .unwrap();

        let cred = vault.get_credential("github").unwrap();
        assert_eq!(cred.credential_type, CredentialType::OAuth);
        assert_eq!(cred.data["access_token"], "gho_xxx");
        assert_eq!(cred.scopes.as_ref().unwrap().len(), 2);
        assert!(cred.expires_at.is_some());
    }
}
