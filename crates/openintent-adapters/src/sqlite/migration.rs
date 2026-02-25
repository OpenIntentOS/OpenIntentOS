//! Migration management for SQLite databases.

use std::path::Path;
use sqlx::{SqlitePool, Row};
use tracing::{debug, info, warn};

use crate::sqlite::SqliteError;

/// Migration manager for SQLite databases
pub struct MigrationManager {
    // Future: store migration metadata
}

impl MigrationManager {
    /// Create a new migration manager
    pub fn new() -> Self {
        Self {}
    }

    /// Run migrations for a database
    pub async fn run_migrations(
        &self,
        db_name: &str,
        migration_dir: Option<&str>,
    ) -> Result<(), SqliteError> {
        info!("Running migrations for database: {}", db_name);

        if let Some(dir) = migration_dir {
            self.run_migrations_from_directory(db_name, dir).await?;
        } else {
            self.run_default_migrations(db_name).await?;
        }

        Ok(())
    }

    /// Run migrations from a specific directory
    async fn run_migrations_from_directory(
        &self,
        _db_name: &str,
        _migration_dir: &str,
    ) -> Result<(), SqliteError> {
        // TODO: Implement file-based migration system
        warn!("File-based migrations not yet implemented");
        Ok(())
    }

    /// Run default system migrations
    async fn run_default_migrations(&self, _db_name: &str) -> Result<(), SqliteError> {
        debug!("Running default migrations");
        
        // For now, just ensure the migrations table exists
        // TODO: Implement actual migration system
        
        Ok(())
    }

    /// Create the migrations tracking table
    async fn ensure_migrations_table(&self, pool: &SqlitePool) -> Result<(), SqliteError> {
        sqlx::query(
            r#"
            CREATE TABLE IF NOT EXISTS __migrations (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                name TEXT NOT NULL UNIQUE,
                applied_at DATETIME DEFAULT CURRENT_TIMESTAMP
            )
            "#,
        )
        .execute(pool)
        .await
        .map_err(|e| SqliteError::MigrationFailed(format!("Failed to create migrations table: {}", e)))?;

        Ok(())
    }

    /// Check if a migration has been applied
    async fn is_migration_applied(&self, pool: &SqlitePool, name: &str) -> Result<bool, SqliteError> {
        let row = sqlx::query("SELECT COUNT(*) as count FROM __migrations WHERE name = ?")
            .bind(name)
            .fetch_one(pool)
            .await
            .map_err(|e| SqliteError::QueryExecution(e.to_string()))?;

        let count: i64 = row.get("count");
        Ok(count > 0)
    }

    /// Mark a migration as applied
    async fn mark_migration_applied(&self, pool: &SqlitePool, name: &str) -> Result<(), SqliteError> {
        sqlx::query("INSERT INTO __migrations (name) VALUES (?)")
            .bind(name)
            .execute(pool)
            .await
            .map_err(|e| SqliteError::MigrationFailed(format!("Failed to mark migration as applied: {}", e)))?;

        Ok(())
    }

    /// Get list of applied migrations
    pub async fn get_applied_migrations(&self, pool: &SqlitePool) -> Result<Vec<String>, SqliteError> {
        self.ensure_migrations_table(pool).await?;

        let rows = sqlx::query("SELECT name FROM __migrations ORDER BY applied_at")
            .fetch_all(pool)
            .await
            .map_err(|e| SqliteError::QueryExecution(e.to_string()))?;

        let migrations = rows
            .into_iter()
            .map(|row| row.get::<String, _>("name"))
            .collect();

        Ok(migrations)
    }
}

impl Default for MigrationManager {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use sqlx::SqlitePool;

    async fn create_test_pool() -> SqlitePool {
        SqlitePool::connect("sqlite::memory:")
            .await
            .expect("Failed to create test database")
    }

    #[tokio::test]
    async fn test_migrations_table_creation() {
        let pool = create_test_pool().await;
        let manager = MigrationManager::new();

        manager.ensure_migrations_table(&pool).await.unwrap();

        // Verify table exists
        let result = sqlx::query("SELECT name FROM sqlite_master WHERE type='table' AND name='__migrations'")
            .fetch_optional(&pool)
            .await
            .unwrap();

        assert!(result.is_some());
    }

    #[tokio::test]
    async fn test_migration_tracking() {
        let pool = create_test_pool().await;
        let manager = MigrationManager::new();

        manager.ensure_migrations_table(&pool).await.unwrap();

        // Initially no migrations
        assert!(!manager.is_migration_applied(&pool, "test_migration").await.unwrap());

        // Mark as applied
        manager.mark_migration_applied(&pool, "test_migration").await.unwrap();

        // Should now be applied
        assert!(manager.is_migration_applied(&pool, "test_migration").await.unwrap());

        // Check list
        let applied = manager.get_applied_migrations(&pool).await.unwrap();
        assert_eq!(applied, vec!["test_migration"]);
    }
}