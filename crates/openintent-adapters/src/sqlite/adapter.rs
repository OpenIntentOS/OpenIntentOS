use serde_json::Value;
use std::collections::HashMap;
use std::path::PathBuf;
use async_trait::async_trait;
use sqlx::{Pool, Sqlite, SqlitePool, Row};
use tracing::{debug, error, info};

use crate::runtime::ToolAdapter;
use crate::sqlite::{SqliteConnectionPool, MigrationManager, SqliteError};

/// SQLite database adapter for OpenIntentOS
pub struct SqliteAdapter {
    pool: SqliteConnectionPool,
    migrations: MigrationManager,
    databases: HashMap<String, SqlitePool>,
}

impl SqliteAdapter {
    /// Create a new SQLite adapter
    pub async fn new(default_db_path: Option<PathBuf>) -> Result<Self, SqliteError> {
        let pool = SqliteConnectionPool::new(default_db_path).await?;
        let migrations = MigrationManager::new();
        
        Ok(Self {
            pool,
            migrations,
            databases: HashMap::new(),
        })
    }

    /// Get or create a database connection
    pub async fn get_database(&mut self, db_name: &str) -> Result<&SqlitePool, SqliteError> {
        if !self.databases.contains_key(db_name) {
            let pool = self.pool.get_or_create_database(db_name).await?;
            self.databases.insert(db_name.to_string(), pool);
        }
        
        Ok(self.databases.get(db_name).unwrap())
    }

    /// Execute a SQL query and return results
    async fn execute_query(&mut self, db_name: &str, query: &str, params: Vec<Value>) -> Result<Value, SqliteError> {
        let pool = self.get_database(db_name).await?;
        
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
                _ => return Err(SqliteError::InvalidParameter(format!("Unsupported parameter type: {}", param))),
            }
        }

        let rows = sql_query.fetch_all(pool).await
            .map_err(|e| SqliteError::QueryExecution(e.to_string()))?;

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
    async fn execute_command(&mut self, db_name: &str, query: &str, params: Vec<Value>) -> Result<Value, SqliteError> {
        let pool = self.get_database(db_name).await?;
        
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
                _ => return Err(SqliteError::InvalidParameter(format!("Unsupported parameter type: {}", param))),
            }
        }

        let result = sql_query.execute(pool).await
            .map_err(|e| SqliteError::QueryExecution(e.to_string()))?;

        Ok(serde_json::json!({
            "rows_affected": result.rows_affected(),
            "last_insert_rowid": result.last_insert_rowid()
        }))
    }
}

#[async_trait]
impl ToolAdapter for SqliteAdapter {
    fn name(&self) -> &'static str {
        "sqlite"
    }

    fn tools(&self) -> Vec<ToolDefinition> {
        vec![
            ToolDefinition {
                name: "sqlite_query".to_string(),
                description: "Execute a SQL SELECT query and return results".to_string(),
                parameters: serde_json::json!({
                    "type": "object",
                    "properties": {
                        "database": {
                            "type": "string",
                            "description": "Database name (default: 'main')"
                        },
                        "query": {
                            "type": "string",
                            "description": "SQL SELECT query to execute"
                        },
                        "params": {
                            "type": "array",
                            "description": "Query parameters",
                            "items": {}
                        }
                    },
                    "required": ["query"]
                }),
            },
            ToolDefinition {
                name: "sqlite_execute".to_string(),
                description: "Execute a SQL command (INSERT, UPDATE, DELETE, CREATE, etc.)".to_string(),
                parameters: serde_json::json!({
                    "type": "object",
                    "properties": {
                        "database": {
                            "type": "string",
                            "description": "Database name (default: 'main')"
                        },
                        "query": {
                            "type": "string",
                            "description": "SQL command to execute"
                        },
                        "params": {
                            "type": "array",
                            "description": "Query parameters",
                            "items": {}
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
                            "description": "Database name (default: 'main')"
                        },
                        "migration_dir": {
                            "type": "string",
                            "description": "Directory containing migration files"
                        }
                    },
                    "required": []
                }),
            },
            ToolDefinition {
                name: "sqlite_backup".to_string(),
                description: "Create a backup of the database".to_string(),
                parameters: serde_json::json!({
                    "type": "object",
                    "properties": {
                        "database": {
                            "type": "string",
                            "description": "Database name (default: 'main')"
                        },
                        "backup_path": {
                            "type": "string",
                            "description": "Path for the backup file"
                        }
                    },
                    "required": ["backup_path"]
                }),
            },
        ]
    }

    async fn execute_tool(&mut self, name: &str, params: Value) -> ToolResult {
        let database = params.get("database")
            .and_then(|v| v.as_str())
            .unwrap_or("main");

        match name {
            "sqlite_query" => {
                let query = params.get("query")
                    .and_then(|v| v.as_str())
                    .ok_or_else(|| AdapterError::InvalidInput("Missing 'query' parameter".to_string()))?;

                let query_params = params.get("params")
                    .and_then(|v| v.as_array())
                    .cloned()
                    .unwrap_or_default();

                match self.execute_query(database, query, query_params).await {
                    Ok(result) => Ok(result),
                    Err(e) => Err(AdapterError::ExecutionFailed(e.to_string())),
                }
            }
            "sqlite_execute" => {
                let query = params.get("query")
                    .and_then(|v| v.as_str())
                    .ok_or_else(|| AdapterError::InvalidInput("Missing 'query' parameter".to_string()))?;

                let query_params = params.get("params")
                    .and_then(|v| v.as_array())
                    .cloned()
                    .unwrap_or_default();

                match self.execute_command(database, query, query_params).await {
                    Ok(result) => Ok(result),
                    Err(e) => Err(AdapterError::ExecutionFailed(e.to_string())),
                }
            }
            "sqlite_migrate" => {
                let migration_dir = params.get("migration_dir")
                    .and_then(|v| v.as_str());

                match self.migrations.run_migrations(database, migration_dir).await {
                    Ok(_) => Ok(serde_json::json!({"status": "migrations_completed"})),
                    Err(e) => Err(AdapterError::ExecutionFailed(e.to_string())),
                }
            }
            "sqlite_backup" => {
                let backup_path = params.get("backup_path")
                    .and_then(|v| v.as_str())
                    .ok_or_else(|| AdapterError::InvalidInput("Missing 'backup_path' parameter".to_string()))?;

                match self.pool.backup_database(database, backup_path).await {
                    Ok(_) => Ok(serde_json::json!({"status": "backup_completed", "path": backup_path})),
                    Err(e) => Err(AdapterError::ExecutionFailed(e.to_string())),
                }
            }
            _ => Err(AdapterError::UnknownTool(name.to_string())),
        }
    }
}