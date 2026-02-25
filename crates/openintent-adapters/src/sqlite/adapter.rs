use serde_json::Value;
use std::collections::HashMap;
use std::path::PathBuf;
use async_trait::async_trait;
use sqlx::{Pool, Sqlite, SqlitePool, Row};
use tracing::{debug, error, info};

use crate::traits::{Adapter, ToolDefinition, AdapterType, HealthStatus};
use crate::sqlite::{SqliteConnectionPool, MigrationManager, SqliteError};

/// SQLite database adapter for OpenIntentOS
pub struct SqliteAdapter {
    pool: SqliteConnectionPool,
    migrations: MigrationManager,
    databases: HashMap<String, SqlitePool>,
}

impl SqliteAdapter {
    /// Create a new SQLite adapter
    pub async fn new(default_db_path: Option<PathBuf>) -> Result<Self> {
        let pool = SqliteConnectionPool::new(default_db_path).await
            .map_err(|e| AdapterError::ConnectionFailed(e.to_string()))?;
        let migrations = MigrationManager::new();
        
        Ok(Self {
            pool,
            migrations,
            databases: HashMap::new(),
        })
    }

    /// Get or create a database connection
    async fn get_database(&mut self, db_name: &str) -> Result<&SqlitePool> {
        if !self.databases.contains_key(db_name) {
            let pool = self.pool.get_or_create_database(db_name).await
                .map_err(|e| AdapterError::ConnectionFailed(e.to_string()))?;
            self.databases.insert(db_name.to_string(), pool);
        }
        
        Ok(self.databases.get(db_name).unwrap())
    }

    /// Execute a SQL query and return results
    async fn execute_query(&self, db_name: &str, query: &str, params: Vec<Value>) -> Result<Value> {
        let pool = if db_name == "main" {
            self.pool.get_main_pool()
        } else {
            return Err(AdapterError::InvalidInput(format!("Database '{}' not supported yet", db_name)));
        };
        
        // Convert JSON values to SQL parameters
        let mut sql_query = sqlx::query(query);
        for param in params {
            match param {
                Value::String(s) => sql_query = sql_query.bind(s),
                Value::Number(n) => {
                    if let Some(i) = n.as_i64() {
                        sql_query = sql_query.bind(i);
                    } else if let Some(f) = n.as_f64() {
                        sql_query = sql_query.bind(f);
                    }
                }
                Value::Bool(b) => sql_query = sql_query.bind(b),
                Value::Null => sql_query = sql_query.bind(Option::<String>::None),
                _ => return Err(AdapterError::InvalidInput(format!("Unsupported parameter type: {}", param))),
            }
        }

        let rows = sql_query.fetch_all(pool).await
            .map_err(|e| AdapterError::ExecutionFailed(e.to_string()))?;

        // Convert rows to JSON
        let mut results = Vec::new();
        for row in rows {
            let mut row_data = serde_json::Map::new();
            
            for (i, column) in row.columns().iter().enumerate() {
                let column_name = column.name();
                
                // Try to extract value based on column type
                let value = if let Ok(val) = row.try_get::<String, _>(i) {
                    Value::String(val)
                } else if let Ok(val) = row.try_get::<i64, _>(i) {
                    Value::Number(serde_json::Number::from(val))
                } else if let Ok(val) = row.try_get::<f64, _>(i) {
                    Value::Number(serde_json::Number::from_f64(val).unwrap_or(serde_json::Number::from(0)))
                } else if let Ok(val) = row.try_get::<bool, _>(i) {
                    Value::Bool(val)
                } else {
                    Value::Null
                };
                
                row_data.insert(column_name.to_string(), value);
            }
            
            results.push(Value::Object(row_data));
        }

        Ok(Value::Array(results))
    }

    /// Execute a SQL command (INSERT, UPDATE, DELETE)
    async fn execute_command(&self, db_name: &str, query: &str, params: Vec<Value>) -> Result<Value> {
        let pool = if db_name == "main" {
            self.pool.get_main_pool()
        } else {
            return Err(AdapterError::InvalidInput(format!("Database '{}' not supported yet", db_name)));
        };
        
        let mut sql_query = sqlx::query(query);
        for param in params {
            match param {
                Value::String(s) => sql_query = sql_query.bind(s),
                Value::Number(n) => {
                    if let Some(i) = n.as_i64() {
                        sql_query = sql_query.bind(i);
                    } else if let Some(f) = n.as_f64() {
                        sql_query = sql_query.bind(f);
                    }
                }
                Value::Bool(b) => sql_query = sql_query.bind(b),
                Value::Null => sql_query = sql_query.bind(Option::<String>::None),
                _ => return Err(AdapterError::InvalidInput(format!("Unsupported parameter type: {}", param))),
            }
        }

        let result = sql_query.execute(pool).await
            .map_err(|e| AdapterError::ExecutionFailed(e.to_string()))?;

        Ok(serde_json::json!({
            "rows_affected": result.rows_affected(),
            "last_insert_rowid": result.last_insert_rowid()
        }))
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
        // Already connected during new()
        Ok(())
    }

    async fn disconnect(&mut self) -> Result<()> {
        // SQLite connections are automatically closed when dropped
        Ok(())
    }

    async fn health_check(&self) -> Result<HealthStatus> {
        // Try a simple query to check database health
        match self.pool.get_main_pool().execute("SELECT 1").await {
            Ok(_) => Ok(HealthStatus::Healthy),
            Err(_) => Ok(HealthStatus::Unhealthy),
        }
    }

    fn tools(&self) -> Vec<ToolDefinition> {
        vec![
            ToolDefinition {
                name: "sqlite_query".to_string(),
                description: "Execute a SELECT query on a SQLite database".to_string(),
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
                name: "sqlite_execute".to_string(),
                description: "Execute an INSERT, UPDATE, or DELETE command on a SQLite database".to_string(),
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
                name: "sqlite_migrate".to_string(),
                description: "Run database migrations".to_string(),
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
                name: "sqlite_backup".to_string(),
                description: "Create a backup of a SQLite database".to_string(),
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
        let database = params.get("database")
            .and_then(|v| v.as_str())
            .unwrap_or("main");

        match tool_name {
            "sqlite_query" => {
                let query = params.get("query")
                    .and_then(|v| v.as_str())
                    .ok_or_else(|| AdapterError::InvalidInput("Missing 'query' parameter".to_string()))?;

                let query_params = params.get("params")
                    .and_then(|v| v.as_array())
                    .cloned()
                    .unwrap_or_default();

                self.execute_query(database, query, query_params).await
                    .map_err(|e| AdapterError::ExecutionFailed(e.to_string()))
            }
            "sqlite_execute" => {
                let query = params.get("query")
                    .and_then(|v| v.as_str())
                    .ok_or_else(|| AdapterError::InvalidInput("Missing 'query' parameter".to_string()))?;

                let query_params = params.get("params")
                    .and_then(|v| v.as_array())
                    .cloned()
                    .unwrap_or_default();

                self.execute_command(database, query, query_params).await
                    .map_err(|e| AdapterError::ExecutionFailed(e.to_string()))
            }
            "sqlite_migrate" => {
                let migration_dir = params.get("migration_dir")
                    .and_then(|v| v.as_str());

                self.migrations.run_migrations(database, migration_dir).await
                    .map_err(|e| AdapterError::ExecutionFailed(e.to_string()))?;
                
                Ok(serde_json::json!({"status": "migrations_completed"}))
            }
            "sqlite_backup" => {
                let backup_path = params.get("backup_path")
                    .and_then(|v| v.as_str())
                    .ok_or_else(|| AdapterError::InvalidInput("Missing 'backup_path' parameter".to_string()))?;

                self.pool.backup_database(database, backup_path).await
                    .map_err(|e| AdapterError::ExecutionFailed(e.to_string()))?;
                
                Ok(serde_json::json!({"status": "backup_completed", "path": backup_path}))
            }
            _ => Err(AdapterError::UnknownTool(tool_name.to_string())),
        }
    }

    fn required_auth(&self) -> Option<crate::traits::AuthRequirement> {
        None
    }
}