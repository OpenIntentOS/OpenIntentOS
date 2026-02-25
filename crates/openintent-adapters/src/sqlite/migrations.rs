//! Database migration management for SQLite adapter.

use std::collections::HashMap;
use sqlx::SqlitePool;
use tracing::{info, debug, error};

use crate::sqlite::error::SqliteError;

/// Manages database schema migrations.
#[derive(Debug)]
pub struct MigrationManager {
    pool: SqlitePool,
    migrations: Vec<Migration>,
}

/// A single database migration.
#[derive(Debug, Clone)]
pub struct Migration {
    pub version: i32,
    pub name: String,
    pub up_sql: String,
    pub down_sql: Option<String>,
}

impl MigrationManager {
    /// Create a new migration manager.
    pub fn new(pool: SqlitePool) -> Self {
        Self {
            pool,
            migrations: Vec::new(),
        }
    }

    /// Add a migration to the manager.
    pub fn add_migration(mut self, migration: Migration) -> Self {
        self.migrations.push(migration);
        self.migrations.sort_by_key(|m| m.version);
        self
    }

    /// Add multiple migrations.
    pub fn add_migrations(mut self, migrations: Vec<Migration>) -> Self {
        self.migrations.extend(migrations);
        self.migrations.sort_by_key(|m| m.version);
        self
    }

    /// Initialize the migrations table if it doesn't exist.
    pub async fn init(&self) -> Result<(), SqliteError> {
        debug!("initializing migrations table");

        sqlx::query(
            r#"
            CREATE TABLE IF NOT EXISTS _migrations (
                version INTEGER PRIMARY KEY,
                name TEXT NOT NULL,
                applied_at DATETIME DEFAULT CURRENT_TIMESTAMP
            )
            "#,
        )
        .execute(&self.pool)
        .await
        .map_err(|e| {
            error!(error = %e, "failed to create migrations table");
            SqliteError::MigrationFailed(format!("failed to create migrations table: {e}"))
        })?;

        Ok(())
    }

    /// Get the current schema version.
    pub async fn current_version(&self) -> Result<i32, SqliteError> {
        let row = sqlx::query_scalar::<_, i32>(
            "SELECT COALESCE(MAX(version), 0) FROM _migrations"
        )
        .fetch_one(&self.pool)
        .await
        .map_err(|e| {
            error!(error = %e, "failed to get current migration version");
            SqliteError::QueryExecution(e.to_string())
        })?;

        Ok(row)
    }

    /// Get list of applied migrations.
    pub async fn applied_migrations(&self) -> Result<Vec<i32>, SqliteError> {
        let rows = sqlx::query_scalar::<_, i32>(
            "SELECT version FROM _migrations ORDER BY version"
        )
        .fetch_all(&self.pool)
        .await
        .map_err(|e| {
            error!(error = %e, "failed to get applied migrations");
            SqliteError::QueryExecution(e.to_string())
        })?;

        Ok(rows)
    }

    /// Run all pending migrations.
    pub async fn migrate(&self) -> Result<Vec<i32>, SqliteError> {
        self.init().await?;

        let current_version = self.current_version().await?;
        let applied = self.applied_migrations().await?;
        let applied_set: std::collections::HashSet<_> = applied.into_iter().collect();

        let mut applied_migrations = Vec::new();

        for migration in &self.migrations {
            if migration.version <= current_version && applied_set.contains(&migration.version) {
                continue; // Already applied
            }

            info!(
                version = migration.version,
                name = %migration.name,
                "applying migration"
            );

            // Start transaction
            let mut tx = self.pool.begin().await.map_err(|e| {
                error!(error = %e, "failed to start migration transaction");
                SqliteError::MigrationFailed(format!("failed to start transaction: {e}"))
            })?;

            // Execute migration SQL
            sqlx::query(&migration.up_sql)
                .execute(&mut *tx)
                .await
                .map_err(|e| {
                    error!(
                        error = %e,
                        version = migration.version,
                        name = %migration.name,
                        "migration failed"
                    );
                    SqliteError::MigrationFailed(format!(
                        "migration {} failed: {e}",
                        migration.version
                    ))
                })?;

            // Record migration
            sqlx::query(
                "INSERT INTO _migrations (version, name) VALUES (?, ?)"
            )
            .bind(migration.version)
            .bind(&migration.name)
            .execute(&mut *tx)
            .await
            .map_err(|e| {
                error!(error = %e, "failed to record migration");
                SqliteError::MigrationFailed(format!("failed to record migration: {e}"))
            })?;

            // Commit transaction
            tx.commit().await.map_err(|e| {
                error!(error = %e, "failed to commit migration transaction");
                SqliteError::MigrationFailed(format!("failed to commit migration: {e}"))
            })?;

            applied_migrations.push(migration.version);

            info!(
                version = migration.version,
                name = %migration.name,
                "migration applied successfully"
            );
        }

        if applied_migrations.is_empty() {
            info!("no pending migrations");
        } else {
            info!(
                count = applied_migrations.len(),
                versions = ?applied_migrations,
                "migrations completed"
            );
        }

        Ok(applied_migrations)
    }

    /// Rollback to a specific version.
    pub async fn rollback_to(&self, target_version: i32) -> Result<Vec<i32>, SqliteError> {
        let current_version = self.current_version().await?;
        
        if target_version >= current_version {
            return Ok(Vec::new()); // Nothing to rollback
        }

        let applied = self.applied_migrations().await?;
        let mut rolled_back = Vec::new();

        // Find migrations to rollback (in reverse order)
        let mut migrations_to_rollback: Vec<_> = self.migrations
            .iter()
            .filter(|m| m.version > target_version && applied.contains(&m.version))
            .collect();
        migrations_to_rollback.sort_by_key(|m| std::cmp::Reverse(m.version));

        for migration in migrations_to_rollback {
            let down_sql = migration.down_sql.as_ref().ok_or_else(|| {
                SqliteError::MigrationFailed(format!(
                    "no rollback SQL for migration {}",
                    migration.version
                ))
            })?;

            info!(
                version = migration.version,
                name = %migration.name,
                "rolling back migration"
            );

            // Start transaction
            let mut tx = self.pool.begin().await.map_err(|e| {
                error!(error = %e, "failed to start rollback transaction");
                SqliteError::MigrationFailed(format!("failed to start transaction: {e}"))
            })?;

            // Execute rollback SQL
            sqlx::query(down_sql)
                .execute(&mut *tx)
                .await
                .map_err(|e| {
                    error!(
                        error = %e,
                        version = migration.version,
                        name = %migration.name,
                        "rollback failed"
                    );
                    SqliteError::MigrationFailed(format!(
                        "rollback {} failed: {e}",
                        migration.version
                    ))
                })?;

            // Remove migration record
            sqlx::query("DELETE FROM _migrations WHERE version = ?")
                .bind(migration.version)
                .execute(&mut *tx)
                .await
                .map_err(|e| {
                    error!(error = %e, "failed to remove migration record");
                    SqliteError::MigrationFailed(format!("failed to remove migration record: {e}"))
                })?;

            // Commit transaction
            tx.commit().await.map_err(|e| {
                error!(error = %e, "failed to commit rollback transaction");
                SqliteError::MigrationFailed(format!("failed to commit rollback: {e}"))
            })?;

            rolled_back.push(migration.version);

            info!(
                version = migration.version,
                name = %migration.name,
                "migration rolled back successfully"
            );
        }

        Ok(rolled_back)
    }

    /// Get migration status information.
    pub async fn status(&self) -> Result<MigrationStatus, SqliteError> {
        self.init().await?;

        let current_version = self.current_version().await?;
        let applied = self.applied_migrations().await?;
        let applied_set: std::collections::HashSet<_> = applied.into_iter().collect();

        let mut pending = Vec::new();
        let mut applied_info = HashMap::new();

        for migration in &self.migrations {
            if applied_set.contains(&migration.version) {
                applied_info.insert(migration.version, migration.name.clone());
            } else if migration.version > current_version {
                pending.push((migration.version, migration.name.clone()));
            }
        }

        Ok(MigrationStatus {
            current_version,
            applied_migrations: applied_info,
            pending_migrations: pending,
        })
    }
}

/// Information about migration status.
#[derive(Debug)]
pub struct MigrationStatus {
    pub current_version: i32,
    pub applied_migrations: HashMap<i32, String>,
    pub pending_migrations: Vec<(i32, String)>,
}

impl Migration {
    /// Create a new migration.
    pub fn new(
        version: i32,
        name: impl Into<String>,
        up_sql: impl Into<String>,
    ) -> Self {
        Self {
            version,
            name: name.into(),
            up_sql: up_sql.into(),
            down_sql: None,
        }
    }

    /// Add rollback SQL to the migration.
    pub fn with_down_sql(mut self, down_sql: impl Into<String>) -> Self {
        self.down_sql = Some(down_sql.into());
        self
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn migration_creation() {
        let migration = Migration::new(
            1,
            "create_users_table",
            "CREATE TABLE users (id INTEGER PRIMARY KEY, name TEXT)"
        );

        assert_eq!(migration.version, 1);
        assert_eq!(migration.name, "create_users_table");
        assert!(migration.down_sql.is_none());
    }

    #[test]
    fn migration_with_rollback() {
        let migration = Migration::new(
            1,
            "create_users_table",
            "CREATE TABLE users (id INTEGER PRIMARY KEY, name TEXT)"
        ).with_down_sql("DROP TABLE users");

        assert!(migration.down_sql.is_some());
        assert_eq!(migration.down_sql.unwrap(), "DROP TABLE users");
    }
}