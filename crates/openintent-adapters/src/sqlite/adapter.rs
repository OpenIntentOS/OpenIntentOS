//! SQLite database adapter for OpenIntentOS.
//!
//! Provides structured data storage capabilities using SQLite.

use std::collections::HashMap;
use std::path::PathBuf;

use async_trait::async_trait;
use serde_json::Value;
use sqlx::{Column, Row, SqlitePool};

use crate::error::{AdapterError, Result};
use crate::sqlite::{MigrationManager, SqliteConnectionPool};
use crate::traits::{Adapter, AdapterType, AuthRequirement, HealthStatus, ToolDefinition};

/// SQLite database adapter for OpenIntentOS.
pub struct SqliteAdapter {
    pool: SqliteConnectionPool,
    migrations: MigrationManager,
    databases: HashMap<String, SqlitePool>,
    connected: bool,
}

impl SqliteAdapter {
    /// Create a new SQLite adapter.
    pub async fn new(default_db_path: Option<PathBuf>) -> Result<Self> {
        let pool = SqliteConnectionPool::new(default_db_path)
            .await
            .map_err(|e| AdapterError::ConfigError(e.to_string()))?;
        let migrations = MigrationManager::new();

        Ok(Self {
            pool,
            migrations,
            databases: HashMap::new(),
            connected: false,
        })
    }

    /// Execute a SQL SELECT query and return results as JSON.
    async fn execute_query(
        &self,
        db_name: &str,
        query: &str,
        params: Vec<Value>,
    ) -> Result<Value> {
        let pool = self.resolve_pool(db_name)?;

        let mut sql_query = sqlx::query(query);
        for param in params {
            sql_query = Self::bind_json_param(sql_query, param)?;
        }

        let rows = sql_query.fetch_all(pool).await.map_err(|e| {
            AdapterError::ExecutionFailed {
                tool_name: "sqlite_query".into(),
                reason: e.to_string(),
            }
        })?;

        let mut results = Vec::new();
        for row in rows {
            let mut row_data = serde_json::Map::new();
            for (i, column) in row.columns().iter().enumerate() {
                let col_name = column.name();
                let value: Value = if let Ok(val) = row.try_get::<String, _>(i) {
                    Value::String(val)
                } else if let Ok(val) = row.try_get::<i64, _>(i) {
                    Value::Number(serde_json::Number::from(val))
                } else if let Ok(val) = row.try_get::<f64, _>(i) {
                    serde_json::Number::from_f64(val)
                        .map(Value::Number)
                        .unwrap_or(Value::Null)
                } else if let Ok(val) = row.try_get::<bool, _>(i) {
                    Value::Bool(val)
                } else {
                    Value::Null
                };
                row_data.insert(col_name.to_string(), value);
            }
            results.push(Value::Object(row_data));
        }

        Ok(Value::Array(results))
    }

    /// Execute a SQL command (INSERT, UPDATE, DELETE) and return affected rows.
    async fn execute_command(
        &self,
        db_name: &str,
        query: &str,
        params: Vec<Value>,
    ) -> Result<Value> {
        let pool = self.resolve_pool(db_name)?;

        let mut sql_query = sqlx::query(query);
        for param in params {
            sql_query = Self::bind_json_param(sql_query, param)?;
        }

        let result = sql_query.execute(pool).await.map_err(|e| {
            AdapterError::ExecutionFailed {
                tool_name: "sqlite_execute".into(),
                reason: e.to_string(),
            }
        })?;

        Ok(serde_json::json!({
            "rows_affected": result.rows_affected(),
            "last_insert_rowid": result.last_insert_rowid()
        }))
    }

    /// Resolve a pool reference for the given database name.
    fn resolve_pool(&self, db_name: &str) -> Result<&SqlitePool> {
        if db_name == "main" {
            if self.databases.contains_key("main") {
                Ok(self.databases.get("main").unwrap())
            } else {
                Err(AdapterError::ExecutionFailed {
                    tool_name: "sqlite".into(),
                    reason: "main database not connected; call connect() first".into(),
                })
            }
        } else {
            self.databases.get(db_name).ok_or_else(|| {
                AdapterError::ExecutionFailed {
                    tool_name: "sqlite".into(),
                    reason: format!("database '{db_name}' not found"),
                }
            })
        }
    }

    /// Bind a JSON value to an sqlx query.
    fn bind_json_param<'q>(
        query: sqlx::query::Query<'q, sqlx::Sqlite, sqlx::sqlite::SqliteArguments<'q>>,
        param: Value,
    ) -> Result<sqlx::query::Query<'q, sqlx::Sqlite, sqlx::sqlite::SqliteArguments<'q>>> {
        match param {
            Value::String(s) => Ok(query.bind(s)),
            Value::Number(n) => {
                if let Some(i) = n.as_i64() {
                    Ok(query.bind(i))
                } else if let Some(f) = n.as_f64() {
                    Ok(query.bind(f))
                } else {
                    Err(AdapterError::InvalidParams {
                        tool_name: "sqlite".into(),
                        reason: format!("unsupported number: {n}"),
                    })
                }
            }
            Value::Bool(b) => Ok(query.bind(b)),
            Value::Null => Ok(query.bind(Option::<String>::None)),
            other => Err(AdapterError::InvalidParams {
                tool_name: "sqlite".into(),
                reason: format!("unsupported parameter type: {other}"),
            }),
        }
    }
}

#[async_trait]
impl Adapter for SqliteAdapter {
    fn id(&self) -> &str {
        "sqlite"
    }

    fn adapter_type(&self) -> AdapterType {
        AdapterType::System
    }

    async fn connect(&mut self) -> Result<()> {
        // Initialize the "main" database pool.
        let pool = self
            .pool
            .get_or_create_database("main")
            .await
            .map_err(|e| AdapterError::ConfigError(e.to_string()))?;
        self.databases.insert("main".into(), pool);
        self.connected = true;
        tracing::info!("sqlite adapter connected");
        Ok(())
    }

    async fn disconnect(&mut self) -> Result<()> {
        self.pool.close_all().await;
        self.databases.clear();
        self.connected = false;
        tracing::info!("sqlite adapter disconnected");
        Ok(())
    }

    async fn health_check(&self) -> Result<HealthStatus> {
        if !self.connected {
            return Ok(HealthStatus::Unhealthy);
        }
        if let Some(pool) = self.databases.get("main") {
            match sqlx::query("SELECT 1").execute(pool).await {
                Ok(_) => Ok(HealthStatus::Healthy),
                Err(_) => Ok(HealthStatus::Degraded),
            }
        } else {
            Ok(HealthStatus::Unhealthy)
        }
    }

    fn tools(&self) -> Vec<ToolDefinition> {
        vec![
            ToolDefinition {
                name: "sqlite_query".into(),
                description: "Execute a SELECT query on a SQLite database".into(),
                parameters: serde_json::json!({
                    "type": "object",
                    "properties": {
                        "database": {
                            "type": "string",
                            "description": "Database name (default: 'main')",
                            "default": "main"
                        },
                        "query": {
                            "type": "string",
                            "description": "SQL SELECT query to execute"
                        },
                        "params": {
                            "type": "array",
                            "description": "Query parameters",
                            "items": {},
                            "default": []
                        }
                    },
                    "required": ["query"]
                }),
            },
            ToolDefinition {
                name: "sqlite_execute".into(),
                description: "Execute an INSERT, UPDATE, or DELETE command on a SQLite database"
                    .into(),
                parameters: serde_json::json!({
                    "type": "object",
                    "properties": {
                        "database": {
                            "type": "string",
                            "description": "Database name (default: 'main')",
                            "default": "main"
                        },
                        "query": {
                            "type": "string",
                            "description": "SQL command to execute"
                        },
                        "params": {
                            "type": "array",
                            "description": "Query parameters",
                            "items": {},
                            "default": []
                        }
                    },
                    "required": ["query"]
                }),
            },
            ToolDefinition {
                name: "sqlite_migrate".into(),
                description: "Run database migrations".into(),
                parameters: serde_json::json!({
                    "type": "object",
                    "properties": {
                        "database": {
                            "type": "string",
                            "description": "Database name (default: 'main')",
                            "default": "main"
                        },
                        "migration_dir": {
                            "type": "string",
                            "description": "Directory containing migration files"
                        }
                    }
                }),
            },
            ToolDefinition {
                name: "sqlite_backup".into(),
                description: "Create a backup of a SQLite database".into(),
                parameters: serde_json::json!({
                    "type": "object",
                    "properties": {
                        "database": {
                            "type": "string",
                            "description": "Database name (default: 'main')",
                            "default": "main"
                        },
                        "backup_path": {
                            "type": "string",
                            "description": "Path where the backup file will be created"
                        }
                    },
                    "required": ["backup_path"]
                }),
            },
        ]
    }

    async fn execute_tool(&self, tool_name: &str, params: Value) -> Result<Value> {
        if !self.connected {
            return Err(AdapterError::ExecutionFailed {
                tool_name: tool_name.into(),
                reason: "sqlite adapter is not connected".into(),
            });
        }

        let database = params
            .get("database")
            .and_then(|v| v.as_str())
            .unwrap_or("main");

        match tool_name {
            "sqlite_query" => {
                let query = params
                    .get("query")
                    .and_then(|v| v.as_str())
                    .ok_or_else(|| AdapterError::InvalidParams {
                        tool_name: "sqlite_query".into(),
                        reason: "missing required field `query`".into(),
                    })?;
                let query_params = params
                    .get("params")
                    .and_then(|v| v.as_array())
                    .cloned()
                    .unwrap_or_default();
                self.execute_query(database, query, query_params).await
            }
            "sqlite_execute" => {
                let query = params
                    .get("query")
                    .and_then(|v| v.as_str())
                    .ok_or_else(|| AdapterError::InvalidParams {
                        tool_name: "sqlite_execute".into(),
                        reason: "missing required field `query`".into(),
                    })?;
                let query_params = params
                    .get("params")
                    .and_then(|v| v.as_array())
                    .cloned()
                    .unwrap_or_default();
                self.execute_command(database, query, query_params).await
            }
            "sqlite_migrate" => {
                let migration_dir = params.get("migration_dir").and_then(|v| v.as_str());
                self.migrations
                    .run_migrations(database, migration_dir)
                    .await
                    .map_err(|e| AdapterError::ExecutionFailed {
                        tool_name: "sqlite_migrate".into(),
                        reason: e.to_string(),
                    })?;
                Ok(serde_json::json!({"status": "migrations_completed"}))
            }
            "sqlite_backup" => {
                let backup_path = params
                    .get("backup_path")
                    .and_then(|v| v.as_str())
                    .ok_or_else(|| AdapterError::InvalidParams {
                        tool_name: "sqlite_backup".into(),
                        reason: "missing required field `backup_path`".into(),
                    })?;
                self.pool
                    .backup_database(database, backup_path)
                    .await
                    .map_err(|e| AdapterError::ExecutionFailed {
                        tool_name: "sqlite_backup".into(),
                        reason: e.to_string(),
                    })?;
                Ok(serde_json::json!({"status": "backup_completed", "path": backup_path}))
            }
            _ => Err(AdapterError::ToolNotFound {
                adapter_id: "sqlite".into(),
                tool_name: tool_name.into(),
            }),
        }
    }

    fn required_auth(&self) -> Option<AuthRequirement> {
        None
    }
}
