//! Multi-user persistence for OpenIntentOS.
//!
//! Provides SQLite-backed storage for user accounts with password
//! hashing via PBKDF2-HMAC-SHA256 (ring). Passwords are stored as
//! `base64(salt):base64(hash)` strings, using 600,000 iterations
//! per OWASP 2023 recommendations.

use base64::Engine;
use base64::engine::general_purpose::STANDARD as BASE64;
use chrono::Utc;
use ring::pbkdf2;
use ring::rand::{SecureRandom, SystemRandom};
use serde::{Deserialize, Serialize};
use tracing::{debug, instrument};
use uuid::Uuid;

use crate::db::Database;
use crate::error::{StoreError, StoreResult};

// ═══════════════════════════════════════════════════════════════════════
//  Types
// ═══════════════════════════════════════════════════════════════════════

/// A user account.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct User {
    /// Unique identifier (UUID v7).
    pub id: String,
    /// Unique login name.
    pub username: String,
    /// Optional display name for UI rendering.
    pub display_name: Option<String>,
    /// Role-based access level.
    pub role: UserRole,
    /// Whether the user can log in.
    pub active: bool,
    /// Unix timestamp when the user was created.
    pub created_at: i64,
    /// Unix timestamp when the user was last updated.
    pub updated_at: i64,
}

/// Role-based access levels for users.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum UserRole {
    /// Full system access, including user management.
    Admin,
    /// Standard access to all features except administration.
    User,
    /// Read-only access.
    Viewer,
}

impl UserRole {
    /// Convert from a database string representation.
    fn from_str(s: &str) -> StoreResult<Self> {
        match s {
            "admin" => Ok(Self::Admin),
            "user" => Ok(Self::User),
            "viewer" => Ok(Self::Viewer),
            other => Err(StoreError::InvalidArgument(format!(
                "unknown user role: {other}"
            ))),
        }
    }

    /// Convert to a database string representation.
    fn as_str(&self) -> &'static str {
        match self {
            Self::Admin => "admin",
            Self::User => "user",
            Self::Viewer => "viewer",
        }
    }
}

impl std::fmt::Display for UserRole {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

// ═══════════════════════════════════════════════════════════════════════
//  Password hashing
// ═══════════════════════════════════════════════════════════════════════

/// PBKDF2-HMAC-SHA256 with 600,000 iterations (OWASP 2023).
const PBKDF2_ITERATIONS: u32 = 600_000;

/// Salt length in bytes.
const SALT_LEN: usize = 32;

/// Derived key length in bytes.
const KEY_LEN: usize = 32;

/// PBKDF2 algorithm.
static PBKDF2_ALG: pbkdf2::Algorithm = pbkdf2::PBKDF2_HMAC_SHA256;

/// Hash a password and return a storable string of the form `base64(salt):base64(hash)`.
fn hash_password(password: &str) -> StoreResult<String> {
    let rng = SystemRandom::new();

    let mut salt = [0u8; SALT_LEN];
    rng.fill(&mut salt)
        .map_err(|_| StoreError::InvalidArgument("failed to generate random salt".into()))?;

    let mut hash = [0u8; KEY_LEN];
    let iterations =
        std::num::NonZeroU32::new(PBKDF2_ITERATIONS).expect("PBKDF2_ITERATIONS is non-zero");
    pbkdf2::derive(
        PBKDF2_ALG,
        iterations,
        &salt,
        password.as_bytes(),
        &mut hash,
    );

    let encoded = format!("{}:{}", BASE64.encode(salt), BASE64.encode(hash));
    Ok(encoded)
}

/// Verify a password against a stored hash string (`base64(salt):base64(hash)`).
fn verify_password(password: &str, stored: &str) -> StoreResult<bool> {
    let parts: Vec<&str> = stored.splitn(2, ':').collect();
    if parts.len() != 2 {
        return Err(StoreError::InvalidArgument(
            "malformed password hash".into(),
        ));
    }

    let salt = BASE64
        .decode(parts[0])
        .map_err(|e| StoreError::InvalidArgument(format!("invalid salt encoding: {e}")))?;
    let expected_hash = BASE64
        .decode(parts[1])
        .map_err(|e| StoreError::InvalidArgument(format!("invalid hash encoding: {e}")))?;

    let iterations =
        std::num::NonZeroU32::new(PBKDF2_ITERATIONS).expect("PBKDF2_ITERATIONS is non-zero");

    Ok(pbkdf2::verify(
        PBKDF2_ALG,
        iterations,
        &salt,
        password.as_bytes(),
        &expected_hash,
    )
    .is_ok())
}

// ═══════════════════════════════════════════════════════════════════════
//  UserStore
// ═══════════════════════════════════════════════════════════════════════

/// CRUD operations on user accounts with password management.
#[derive(Clone)]
pub struct UserStore {
    db: Database,
}

impl UserStore {
    /// Create a new user store backed by `db`.
    pub fn new(db: Database) -> Self {
        Self { db }
    }

    /// Create a new user account.
    ///
    /// The password is hashed with PBKDF2-HMAC-SHA256 before storage.
    /// Returns an error if the username is already taken.
    #[instrument(skip(self, password))]
    pub async fn create(
        &self,
        username: &str,
        display_name: Option<&str>,
        password: &str,
        role: UserRole,
    ) -> StoreResult<User> {
        if username.is_empty() {
            return Err(StoreError::InvalidArgument(
                "username must not be empty".into(),
            ));
        }
        if password.is_empty() {
            return Err(StoreError::InvalidArgument(
                "password must not be empty".into(),
            ));
        }

        let id = Uuid::now_v7().to_string();
        let username = username.to_string();
        let display_name = display_name.map(|s| s.to_string());
        let role_str = role.as_str().to_string();
        let now = Utc::now().timestamp();

        // Hash the password (CPU-intensive, done inside spawn_blocking via db.execute)
        let password_hash = hash_password(password)?;

        let user = User {
            id: id.clone(),
            username: username.clone(),
            display_name: display_name.clone(),
            role,
            active: true,
            created_at: now,
            updated_at: now,
        };

        self.db
            .execute(move |conn| {
                conn.execute(
                    "INSERT INTO users (id, username, display_name, password_hash, role, active, created_at, updated_at) \
                     VALUES (?1, ?2, ?3, ?4, ?5, 1, ?6, ?6)",
                    rusqlite::params![id, username, display_name, password_hash, role_str, now],
                )
                .map_err(|e| {
                    if let rusqlite::Error::SqliteFailure(ref err, _) = e
                        && err.code == rusqlite::ErrorCode::ConstraintViolation {
                            return StoreError::InvalidArgument(format!(
                                "username already taken: {username}"
                            ));
                        }
                    StoreError::Sqlite(e)
                })?;
                Ok(())
            })
            .await?;

        debug!(user_id = %user.id, username = %user.username, "user created");
        Ok(user)
    }

    /// Fetch a single user by ID, returning `None` if not found.
    #[instrument(skip(self))]
    pub async fn get(&self, id: &str) -> StoreResult<Option<User>> {
        let id = id.to_string();
        self.db
            .execute(move |conn| {
                let result = conn.query_row(
                    "SELECT id, username, display_name, role, active, created_at, updated_at \
                     FROM users WHERE id = ?1",
                    rusqlite::params![id],
                    |row| {
                        Ok(UserRow {
                            id: row.get(0)?,
                            username: row.get(1)?,
                            display_name: row.get(2)?,
                            role: row.get(3)?,
                            active: row.get(4)?,
                            created_at: row.get(5)?,
                            updated_at: row.get(6)?,
                        })
                    },
                );
                match result {
                    Ok(row) => row.into_user().map(Some),
                    Err(rusqlite::Error::QueryReturnedNoRows) => Ok(None),
                    Err(e) => Err(StoreError::Sqlite(e)),
                }
            })
            .await
    }

    /// Fetch a single user by username, returning `None` if not found.
    #[instrument(skip(self))]
    pub async fn get_by_username(&self, username: &str) -> StoreResult<Option<User>> {
        let username = username.to_string();
        self.db
            .execute(move |conn| {
                let result = conn.query_row(
                    "SELECT id, username, display_name, role, active, created_at, updated_at \
                     FROM users WHERE username = ?1",
                    rusqlite::params![username],
                    |row| {
                        Ok(UserRow {
                            id: row.get(0)?,
                            username: row.get(1)?,
                            display_name: row.get(2)?,
                            role: row.get(3)?,
                            active: row.get(4)?,
                            created_at: row.get(5)?,
                            updated_at: row.get(6)?,
                        })
                    },
                );
                match result {
                    Ok(row) => row.into_user().map(Some),
                    Err(rusqlite::Error::QueryReturnedNoRows) => Ok(None),
                    Err(e) => Err(StoreError::Sqlite(e)),
                }
            })
            .await
    }

    /// Authenticate a user by username and password.
    ///
    /// Returns `Some(User)` if the credentials are valid and the user is
    /// active, `None` otherwise. Uses constant-time comparison for the
    /// password hash.
    #[instrument(skip(self, password))]
    pub async fn authenticate(&self, username: &str, password: &str) -> StoreResult<Option<User>> {
        let username = username.to_string();
        let password = password.to_string();

        self.db
            .execute(move |conn| {
                let result = conn.query_row(
                    "SELECT id, username, display_name, password_hash, role, active, created_at, updated_at \
                     FROM users WHERE username = ?1",
                    rusqlite::params![username],
                    |row| {
                        Ok(AuthRow {
                            id: row.get(0)?,
                            username: row.get(1)?,
                            display_name: row.get(2)?,
                            password_hash: row.get(3)?,
                            role: row.get(4)?,
                            active: row.get(5)?,
                            created_at: row.get(6)?,
                            updated_at: row.get(7)?,
                        })
                    },
                );

                match result {
                    Ok(row) => {
                        // Inactive users cannot authenticate.
                        if !row.active {
                            return Ok(None);
                        }

                        let valid = verify_password(&password, &row.password_hash)?;
                        if valid {
                            let user = UserRow {
                                id: row.id,
                                username: row.username,
                                display_name: row.display_name,
                                role: row.role,
                                active: row.active,
                                created_at: row.created_at,
                                updated_at: row.updated_at,
                            };
                            user.into_user().map(Some)
                        } else {
                            Ok(None)
                        }
                    }
                    Err(rusqlite::Error::QueryReturnedNoRows) => Ok(None),
                    Err(e) => Err(StoreError::Sqlite(e)),
                }
            })
            .await
    }

    /// List users ordered by creation time, with pagination.
    #[instrument(skip(self))]
    pub async fn list(&self, limit: i64, offset: i64) -> StoreResult<Vec<User>> {
        self.db
            .execute(move |conn| {
                let mut stmt = conn.prepare(
                    "SELECT id, username, display_name, role, active, created_at, updated_at \
                     FROM users ORDER BY created_at ASC LIMIT ?1 OFFSET ?2",
                )?;
                let rows = stmt
                    .query_map(rusqlite::params![limit, offset], |row| {
                        Ok(UserRow {
                            id: row.get(0)?,
                            username: row.get(1)?,
                            display_name: row.get(2)?,
                            role: row.get(3)?,
                            active: row.get(4)?,
                            created_at: row.get(5)?,
                            updated_at: row.get(6)?,
                        })
                    })?
                    .collect::<Result<Vec<_>, _>>()?;

                rows.into_iter().map(|r| r.into_user()).collect()
            })
            .await
    }

    /// Update a user's display name and role.
    #[instrument(skip(self))]
    pub async fn update(
        &self,
        id: &str,
        display_name: Option<&str>,
        role: UserRole,
    ) -> StoreResult<()> {
        let id = id.to_string();
        let display_name = display_name.map(|s| s.to_string());
        let role_str = role.as_str().to_string();
        let now = Utc::now().timestamp();

        self.db
            .execute(move |conn| {
                let updated = conn.execute(
                    "UPDATE users SET display_name = ?2, role = ?3, updated_at = ?4 WHERE id = ?1",
                    rusqlite::params![id, display_name, role_str, now],
                )?;
                if updated == 0 {
                    return Err(StoreError::NotFound { entity: "user", id });
                }
                Ok(())
            })
            .await
    }

    /// Change a user's password.
    ///
    /// The new password is hashed with PBKDF2 before storage.
    #[instrument(skip(self, new_password))]
    pub async fn change_password(&self, id: &str, new_password: &str) -> StoreResult<()> {
        if new_password.is_empty() {
            return Err(StoreError::InvalidArgument(
                "password must not be empty".into(),
            ));
        }

        let id = id.to_string();
        let password_hash = hash_password(new_password)?;
        let now = Utc::now().timestamp();

        self.db
            .execute(move |conn| {
                let updated = conn.execute(
                    "UPDATE users SET password_hash = ?2, updated_at = ?3 WHERE id = ?1",
                    rusqlite::params![id, password_hash, now],
                )?;
                if updated == 0 {
                    return Err(StoreError::NotFound { entity: "user", id });
                }
                Ok(())
            })
            .await
    }

    /// Enable or disable a user account.
    #[instrument(skip(self))]
    pub async fn set_active(&self, id: &str, active: bool) -> StoreResult<()> {
        let id = id.to_string();
        let now = Utc::now().timestamp();

        self.db
            .execute(move |conn| {
                let updated = conn.execute(
                    "UPDATE users SET active = ?2, updated_at = ?3 WHERE id = ?1",
                    rusqlite::params![id, active, now],
                )?;
                if updated == 0 {
                    return Err(StoreError::NotFound { entity: "user", id });
                }
                Ok(())
            })
            .await
    }

    /// Delete a user permanently.
    #[instrument(skip(self))]
    pub async fn delete(&self, id: &str) -> StoreResult<()> {
        let id = id.to_string();
        self.db
            .execute(move |conn| {
                let deleted =
                    conn.execute("DELETE FROM users WHERE id = ?1", rusqlite::params![id])?;
                if deleted == 0 {
                    return Err(StoreError::NotFound { entity: "user", id });
                }
                Ok(())
            })
            .await
    }

    /// Return the total number of users.
    #[instrument(skip(self))]
    pub async fn count(&self) -> StoreResult<i64> {
        self.db
            .execute(|conn| {
                let count: i64 =
                    conn.query_row("SELECT COUNT(*) FROM users", [], |row| row.get(0))?;
                Ok(count)
            })
            .await
    }
}

// ═══════════════════════════════════════════════════════════════════════
//  Internal row mapping
// ═══════════════════════════════════════════════════════════════════════

/// Raw row data from SQLite before role parsing.
struct UserRow {
    id: String,
    username: String,
    display_name: Option<String>,
    role: String,
    active: bool,
    created_at: i64,
    updated_at: i64,
}

impl UserRow {
    fn into_user(self) -> StoreResult<User> {
        let role = UserRole::from_str(&self.role)?;
        Ok(User {
            id: self.id,
            username: self.username,
            display_name: self.display_name,
            role,
            active: self.active,
            created_at: self.created_at,
            updated_at: self.updated_at,
        })
    }
}

/// Raw row data for authentication (includes password_hash).
struct AuthRow {
    id: String,
    username: String,
    display_name: Option<String>,
    password_hash: String,
    role: String,
    active: bool,
    created_at: i64,
    updated_at: i64,
}

// ── tests ────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    /// Create an in-memory database with all tables for testing.
    async fn setup_db() -> Database {
        let db = Database::open_in_memory().unwrap();
        db.run_migrations().await.unwrap();
        db
    }

    fn setup_store(db: Database) -> UserStore {
        UserStore::new(db)
    }

    #[tokio::test]
    async fn create_and_get_user() {
        let db = setup_db().await;
        let store = setup_store(db);

        let user = store
            .create(
                "alice",
                Some("Alice Smith"),
                "secure-password-123",
                UserRole::User,
            )
            .await
            .unwrap();

        assert_eq!(user.username, "alice");
        assert_eq!(user.display_name.as_deref(), Some("Alice Smith"));
        assert_eq!(user.role, UserRole::User);
        assert!(user.active);
        assert!(user.created_at > 0);
        assert_eq!(user.created_at, user.updated_at);

        let fetched = store.get(&user.id).await.unwrap();
        assert!(fetched.is_some());
        let fetched = fetched.unwrap();
        assert_eq!(fetched.id, user.id);
        assert_eq!(fetched.username, "alice");
        assert_eq!(fetched.display_name.as_deref(), Some("Alice Smith"));
    }

    #[tokio::test]
    async fn get_nonexistent_user_returns_none() {
        let db = setup_db().await;
        let store = setup_store(db);

        let result = store.get("nonexistent-id").await.unwrap();
        assert!(result.is_none());
    }

    #[tokio::test]
    async fn get_by_username() {
        let db = setup_db().await;
        let store = setup_store(db);

        store
            .create("bob", None, "password123", UserRole::Admin)
            .await
            .unwrap();

        let found = store.get_by_username("bob").await.unwrap();
        assert!(found.is_some());
        assert_eq!(found.unwrap().username, "bob");

        let not_found = store.get_by_username("nonexistent").await.unwrap();
        assert!(not_found.is_none());
    }

    #[tokio::test]
    async fn authenticate_valid_credentials() {
        let db = setup_db().await;
        let store = setup_store(db);

        store
            .create("charlie", None, "my-secret-pw", UserRole::User)
            .await
            .unwrap();

        let result = store.authenticate("charlie", "my-secret-pw").await.unwrap();
        assert!(result.is_some());
        let user = result.unwrap();
        assert_eq!(user.username, "charlie");
    }

    #[tokio::test]
    async fn authenticate_wrong_password_returns_none() {
        let db = setup_db().await;
        let store = setup_store(db);

        store
            .create("diana", None, "correct-password", UserRole::User)
            .await
            .unwrap();

        let result = store.authenticate("diana", "wrong-password").await.unwrap();
        assert!(result.is_none());
    }

    #[tokio::test]
    async fn authenticate_nonexistent_user_returns_none() {
        let db = setup_db().await;
        let store = setup_store(db);

        let result = store.authenticate("ghost", "any-password").await.unwrap();
        assert!(result.is_none());
    }

    #[tokio::test]
    async fn authenticate_inactive_user_returns_none() {
        let db = setup_db().await;
        let store = setup_store(db);

        let user = store
            .create("eve", None, "password", UserRole::User)
            .await
            .unwrap();

        store.set_active(&user.id, false).await.unwrap();

        let result = store.authenticate("eve", "password").await.unwrap();
        assert!(result.is_none());
    }

    #[tokio::test]
    async fn list_users_with_pagination() {
        let db = setup_db().await;
        let store = setup_store(db);

        for i in 0..5 {
            store
                .create(&format!("user{i}"), None, "password", UserRole::User)
                .await
                .unwrap();
        }

        let all = store.list(10, 0).await.unwrap();
        assert_eq!(all.len(), 5);

        let page1 = store.list(2, 0).await.unwrap();
        assert_eq!(page1.len(), 2);

        let page2 = store.list(2, 2).await.unwrap();
        assert_eq!(page2.len(), 2);

        let page3 = store.list(2, 4).await.unwrap();
        assert_eq!(page3.len(), 1);

        let empty = store.list(10, 10).await.unwrap();
        assert!(empty.is_empty());
    }

    #[tokio::test]
    async fn update_user_profile() {
        let db = setup_db().await;
        let store = setup_store(db);

        let user = store
            .create("frank", None, "password", UserRole::User)
            .await
            .unwrap();

        store
            .update(&user.id, Some("Frank Johnson"), UserRole::Admin)
            .await
            .unwrap();

        let fetched = store.get(&user.id).await.unwrap().unwrap();
        assert_eq!(fetched.display_name.as_deref(), Some("Frank Johnson"));
        assert_eq!(fetched.role, UserRole::Admin);
        assert!(fetched.updated_at >= user.updated_at);
    }

    #[tokio::test]
    async fn update_nonexistent_user_returns_not_found() {
        let db = setup_db().await;
        let store = setup_store(db);

        let result = store.update("nonexistent-id", None, UserRole::User).await;
        assert!(result.is_err());
        match result.unwrap_err() {
            StoreError::NotFound { entity, .. } => assert_eq!(entity, "user"),
            other => panic!("expected NotFound, got: {other}"),
        }
    }

    #[tokio::test]
    async fn change_password() {
        let db = setup_db().await;
        let store = setup_store(db);

        let user = store
            .create("grace", None, "old-password", UserRole::User)
            .await
            .unwrap();

        // Old password works.
        let auth = store.authenticate("grace", "old-password").await.unwrap();
        assert!(auth.is_some());

        // Change it.
        store
            .change_password(&user.id, "new-password")
            .await
            .unwrap();

        // Old password no longer works.
        let auth_old = store.authenticate("grace", "old-password").await.unwrap();
        assert!(auth_old.is_none());

        // New password works.
        let auth_new = store.authenticate("grace", "new-password").await.unwrap();
        assert!(auth_new.is_some());
    }

    #[tokio::test]
    async fn set_active_toggle() {
        let db = setup_db().await;
        let store = setup_store(db);

        let user = store
            .create("hank", None, "password", UserRole::User)
            .await
            .unwrap();
        assert!(user.active);

        store.set_active(&user.id, false).await.unwrap();
        let fetched = store.get(&user.id).await.unwrap().unwrap();
        assert!(!fetched.active);

        store.set_active(&user.id, true).await.unwrap();
        let fetched = store.get(&user.id).await.unwrap().unwrap();
        assert!(fetched.active);
    }

    #[tokio::test]
    async fn set_active_nonexistent_returns_not_found() {
        let db = setup_db().await;
        let store = setup_store(db);

        let result = store.set_active("nonexistent-id", true).await;
        assert!(result.is_err());
        match result.unwrap_err() {
            StoreError::NotFound { entity, .. } => assert_eq!(entity, "user"),
            other => panic!("expected NotFound, got: {other}"),
        }
    }

    #[tokio::test]
    async fn delete_user() {
        let db = setup_db().await;
        let store = setup_store(db);

        let user = store
            .create("ivan", None, "password", UserRole::User)
            .await
            .unwrap();

        store.delete(&user.id).await.unwrap();

        let fetched = store.get(&user.id).await.unwrap();
        assert!(fetched.is_none());
    }

    #[tokio::test]
    async fn delete_nonexistent_returns_not_found() {
        let db = setup_db().await;
        let store = setup_store(db);

        let result = store.delete("nonexistent-id").await;
        assert!(result.is_err());
        match result.unwrap_err() {
            StoreError::NotFound { entity, .. } => assert_eq!(entity, "user"),
            other => panic!("expected NotFound, got: {other}"),
        }
    }

    #[tokio::test]
    async fn count_users() {
        let db = setup_db().await;
        let store = setup_store(db);

        assert_eq!(store.count().await.unwrap(), 0);

        store
            .create("user1", None, "password", UserRole::User)
            .await
            .unwrap();
        assert_eq!(store.count().await.unwrap(), 1);

        store
            .create("user2", None, "password", UserRole::Admin)
            .await
            .unwrap();
        assert_eq!(store.count().await.unwrap(), 2);
    }

    #[tokio::test]
    async fn duplicate_username_rejected() {
        let db = setup_db().await;
        let store = setup_store(db);

        store
            .create("unique_name", None, "password1", UserRole::User)
            .await
            .unwrap();

        let result = store
            .create("unique_name", None, "password2", UserRole::Admin)
            .await;

        assert!(result.is_err());
        match result.unwrap_err() {
            StoreError::InvalidArgument(msg) => {
                assert!(msg.contains("username already taken"), "got: {msg}");
            }
            other => panic!("expected InvalidArgument, got: {other}"),
        }
    }

    #[tokio::test]
    async fn create_all_roles() {
        let db = setup_db().await;
        let store = setup_store(db);

        let admin = store
            .create("admin_user", None, "password", UserRole::Admin)
            .await
            .unwrap();
        assert_eq!(admin.role, UserRole::Admin);

        let regular = store
            .create("regular_user", None, "password", UserRole::User)
            .await
            .unwrap();
        assert_eq!(regular.role, UserRole::User);

        let viewer = store
            .create("viewer_user", None, "password", UserRole::Viewer)
            .await
            .unwrap();
        assert_eq!(viewer.role, UserRole::Viewer);
    }

    #[tokio::test]
    async fn empty_username_rejected() {
        let db = setup_db().await;
        let store = setup_store(db);

        let result = store.create("", None, "password", UserRole::User).await;
        assert!(result.is_err());
        match result.unwrap_err() {
            StoreError::InvalidArgument(msg) => {
                assert!(msg.contains("username must not be empty"), "got: {msg}");
            }
            other => panic!("expected InvalidArgument, got: {other}"),
        }
    }

    #[tokio::test]
    async fn empty_password_rejected() {
        let db = setup_db().await;
        let store = setup_store(db);

        let result = store.create("user", None, "", UserRole::User).await;
        assert!(result.is_err());
        match result.unwrap_err() {
            StoreError::InvalidArgument(msg) => {
                assert!(msg.contains("password must not be empty"), "got: {msg}");
            }
            other => panic!("expected InvalidArgument, got: {other}"),
        }
    }

    #[tokio::test]
    async fn change_password_nonexistent_returns_not_found() {
        let db = setup_db().await;
        let store = setup_store(db);

        let result = store.change_password("nonexistent-id", "new-pw").await;
        assert!(result.is_err());
        match result.unwrap_err() {
            StoreError::NotFound { entity, .. } => assert_eq!(entity, "user"),
            other => panic!("expected NotFound, got: {other}"),
        }
    }

    #[tokio::test]
    async fn password_hash_is_different_for_same_password() {
        // Verify that each hash has a unique salt.
        let hash1 = hash_password("same-password").unwrap();
        let hash2 = hash_password("same-password").unwrap();
        assert_ne!(hash1, hash2, "hashes should differ due to random salt");

        // But both verify correctly.
        assert!(verify_password("same-password", &hash1).unwrap());
        assert!(verify_password("same-password", &hash2).unwrap());
    }
}
