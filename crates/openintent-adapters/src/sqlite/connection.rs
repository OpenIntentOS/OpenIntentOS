use std::collections::HashMap;
use std::path::PathBuf;
use sqlx::{SqlitePool, sqlite::SqlitePoolOptions};
use tracing::debug;

use crate::sqlite::SqliteError;

/// SQLite connection pool manager
pub struct SqliteConnectionPool {
    default_db_path: PathBuf,
    pools: HashMap<String, SqlitePool>,
}

impl SqliteConnectionPool {
    /// Create a new connection pool manager
    pub async fn new(default_db_path: Option<PathBuf>) -> Result<Self, SqliteError> {
        let default_path = default_db_path.unwrap_or_else(|| {
            PathBuf::from("data/openintent.db")
        });

        // Ensure the directory exists
        if let Some(parent) = default_path.parent() {
            tokio::fs::create_dir_all(parent).await
                .map_err(|e| SqliteError::Connection(format!("Failed to create database directory: {}", e)))?;
        }

        Ok(Self {
            default_db_path: default_path,
            pools: HashMap::new(),
        })
    }

    /// Get or create a database connection pool
    pub async fn get_or_create_database(&mut self, db_name: &str) -> Result<SqlitePool, SqliteError> {
        if let Some(pool) = self.pools.get(db_name) {
            return Ok(pool.clone());
        }

        let db_path = if db_name == "main" {
            self.default_db_path.clone()
        } else {
            let mut path = self.default_db_path.clone();
            path.set_file_name(format!("{}.db", db_name));
            path
        };

        // Ensure the directory exists
        if let Some(parent) = db_path.parent() {
            tokio::fs::create_dir_all(parent).await
                .map_err(|e| SqliteError::Connection(format!("Failed to create database directory: {}", e)))?;
        }

        let database_url = format!("sqlite:{}", db_path.display());
        
        let pool = SqlitePoolOptions::new()
            .max_connections(10)
            .connect(&database_url)
            .await
            .map_err(|e| SqliteError::Connection(format!("Failed to connect to database {}: {}", db_name, e)))?;

        // Run basic setup
        sqlx::query("PRAGMA foreign_keys = ON")
            .execute(&pool)
            .await
            .map_err(|e| SqliteError::QueryExecution(format!("Failed to enable foreign keys: {}", e)))?;

        sqlx::query("PRAGMA journal_mode = WAL")
            .execute(&pool)
            .await
            .map_err(|e| SqliteError::QueryExecution(format!("Failed to set WAL mode: {}", e)))?;

        self.pools.insert(db_name.to_string(), pool.clone());
        Ok(pool)
    }

    /// Get an existing database pool
    pub fn get_database(&self, db_name: &str) -> Option<&SqlitePool> {
        self.pools.get(db_name)
    }

    /// List all database names
    pub fn list_databases(&self) -> Vec<String> {
        self.pools.keys().cloned().collect()
    }

    /// Close a database connection
    pub async fn close_database(&mut self, db_name: &str) -> Result<(), SqliteError> {
        if let Some(pool) = self.pools.remove(db_name) {
            pool.close().await;
        }
        Ok(())
    }

    /// Close all database connections
    pub async fn close_all(&mut self) {
        for (_, pool) in self.pools.drain() {
            pool.close().await;
        }
    }

    /// Backup a database to a file
    pub async fn backup_database(&self, db_name: &str, backup_path: &str) -> Result<(), SqliteError> {
        let pool = self.pools.get(db_name)
            .ok_or_else(|| SqliteError::Configuration(format!("Database '{}' not found", db_name)))?;

        // Simple backup using VACUUM INTO
        let backup_query = format!("VACUUM INTO '{}'", backup_path);
        sqlx::query(&backup_query)
            .execute(pool)
            .await
            .map_err(|e| SqliteError::QueryExecution(format!("Backup failed: {}", e)))?;

        Ok(())
    }

    /// Get database file size in bytes
    pub async fn get_database_size(&self, db_name: &str) -> Result<u64, SqliteError> {
        let db_path = if db_name == "main" {
            self.default_db_path.clone()
        } else {
            let mut path = self.default_db_path.clone();
            path.set_file_name(format!("{}.db", db_name));
            path
        };

        let metadata = tokio::fs::metadata(&db_path).await
            .map_err(|e| SqliteError::Configuration(format!("Failed to get database size: {}", e)))?;

        Ok(metadata.len())
    }

    /// Check if a database exists
    pub async fn database_exists(&self, db_name: &str) -> bool {
        let db_path = if db_name == "main" {
            self.default_db_path.clone()
        } else {
            let mut path = self.default_db_path.clone();
            path.set_file_name(format!("{}.db", db_name));
            path
        };

        db_path.exists()
    }

    /// Get the main database pool
    pub fn get_main_pool(&self) -> &SqlitePool {
        self.pools.get("main").expect("Main database should always be available")
    }
}