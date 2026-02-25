//! Query builder and execution utilities for SQLite adapter.

use std::collections::HashMap;
use serde_json::Value;
use sqlx::{Row, SqlitePool};
use tracing::{debug, error};

use crate::sqlite::error::SqliteError;

/// Query builder for constructing SQL queries dynamically.
#[derive(Debug, Clone)]
pub struct QueryBuilder {
    query: String,
    params: Vec<Value>,
}

impl QueryBuilder {
    /// Create a new query builder with a base SQL query.
    pub fn new(query: impl Into<String>) -> Self {
        Self {
            query: query.into(),
            params: Vec::new(),
        }
    }

    /// Add a parameter to the query.
    pub fn param(mut self, value: Value) -> Self {
        self.params.push(value);
        self
    }

    /// Add multiple parameters to the query.
    pub fn params(mut self, values: Vec<Value>) -> Self {
        self.params.extend(values);
        self
    }

    /// Replace placeholders in the query with actual values.
    pub fn build(self) -> (String, Vec<Value>) {
        (self.query, self.params)
    }

    /// Execute the query and return results.
    pub async fn execute(self, pool: &SqlitePool) -> Result<QueryResult, SqliteError> {
        let (query, params) = self.build();
        
        debug!(query = %query, param_count = params.len(), "executing SQL query");

        // Convert JSON values to SQLite-compatible types
        let mut sqlx_params = Vec::new();
        for param in &params {
            match param {
                Value::String(s) => sqlx_params.push(s.clone()),
                Value::Number(n) if n.is_i64() => sqlx_params.push(n.as_i64().unwrap().to_string()),
                Value::Number(n) if n.is_f64() => sqlx_params.push(n.as_f64().unwrap().to_string()),
                Value::Bool(b) => sqlx_params.push(b.to_string()),
                Value::Null => sqlx_params.push("NULL".to_string()),
                _ => sqlx_params.push(param.to_string()),
            }
        }

        // Execute query based on type
        if query.trim().to_uppercase().starts_with("SELECT") {
            self.execute_select(pool, &query, &sqlx_params).await
        } else {
            self.execute_modify(pool, &query, &sqlx_params).await
        }
    }

    /// Execute a SELECT query.
    async fn execute_select(
        &self,
        pool: &SqlitePool,
        query: &str,
        params: &[String],
    ) -> Result<QueryResult, SqliteError> {
        let mut query_builder = sqlx::query(query);
        
        for param in params {
            query_builder = query_builder.bind(param);
        }

        let rows = query_builder
            .fetch_all(pool)
            .await
            .map_err(|e| {
                error!(error = %e, "failed to execute SELECT query");
                SqliteError::QueryExecution(e.to_string())
            })?;

        let mut results = Vec::new();
        for row in rows {
            let mut row_data = HashMap::new();
            
            // Get column names and values
            for (i, column) in row.columns().iter().enumerate() {
                let column_name = column.name().to_string();
                
                // Try to extract value based on SQLite type
                let value = if let Ok(val) = row.try_get::<String, _>(i) {
                    Value::String(val)
                } else if let Ok(val) = row.try_get::<i64, _>(i) {
                    Value::Number(val.into())
                } else if let Ok(val) = row.try_get::<f64, _>(i) {
                    Value::Number(serde_json::Number::from_f64(val).unwrap_or_else(|| 0.into()))
                } else if let Ok(val) = row.try_get::<bool, _>(i) {
                    Value::Bool(val)
                } else {
                    Value::Null
                };
                
                row_data.insert(column_name, value);
            }
            
            results.push(Value::Object(row_data.into()));
        }

        Ok(QueryResult::Select { 
            rows: results,
            count: results.len(),
        })
    }

    /// Execute an INSERT, UPDATE, or DELETE query.
    async fn execute_modify(
        &self,
        pool: &SqlitePool,
        query: &str,
        params: &[String],
    ) -> Result<QueryResult, SqliteError> {
        let mut query_builder = sqlx::query(query);
        
        for param in params {
            query_builder = query_builder.bind(param);
        }

        let result = query_builder
            .execute(pool)
            .await
            .map_err(|e| {
                error!(error = %e, "failed to execute modify query");
                SqliteError::QueryExecution(e.to_string())
            })?;

        Ok(QueryResult::Modify {
            rows_affected: result.rows_affected(),
            last_insert_id: result.last_insert_rowid(),
        })
    }
}

/// Result of a SQL query execution.
#[derive(Debug, Clone)]
pub enum QueryResult {
    /// Result from a SELECT query.
    Select {
        rows: Vec<Value>,
        count: usize,
    },
    /// Result from INSERT, UPDATE, or DELETE.
    Modify {
        rows_affected: u64,
        last_insert_id: i64,
    },
}

impl QueryResult {
    /// Convert the result to a JSON value for tool responses.
    pub fn to_json(&self) -> Value {
        match self {
            QueryResult::Select { rows, count } => {
                serde_json::json!({
                    "type": "select",
                    "rows": rows,
                    "count": count
                })
            }
            QueryResult::Modify { rows_affected, last_insert_id } => {
                serde_json::json!({
                    "type": "modify",
                    "rows_affected": rows_affected,
                    "last_insert_id": last_insert_id
                })
            }
        }
    }

    /// Get the number of rows returned or affected.
    pub fn row_count(&self) -> usize {
        match self {
            QueryResult::Select { count, .. } => *count,
            QueryResult::Modify { rows_affected, .. } => *rows_affected as usize,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn query_builder_basic() {
        let builder = QueryBuilder::new("SELECT * FROM users WHERE id = ?")
            .param(Value::Number(42.into()));
        
        let (query, params) = builder.build();
        assert_eq!(query, "SELECT * FROM users WHERE id = ?");
        assert_eq!(params.len(), 1);
        assert_eq!(params[0], Value::Number(42.into()));
    }

    #[test]
    fn query_builder_multiple_params() {
        let builder = QueryBuilder::new("INSERT INTO users (name, email, active) VALUES (?, ?, ?)")
            .param(Value::String("John".to_string()))
            .param(Value::String("john@example.com".to_string()))
            .param(Value::Bool(true));
        
        let (query, params) = builder.build();
        assert_eq!(params.len(), 3);
        assert_eq!(params[0], Value::String("John".to_string()));
        assert_eq!(params[1], Value::String("john@example.com".to_string()));
        assert_eq!(params[2], Value::Bool(true));
    }

    #[test]
    fn query_result_to_json() {
        let result = QueryResult::Select {
            rows: vec![
                serde_json::json!({"id": 1, "name": "John"}),
                serde_json::json!({"id": 2, "name": "Jane"}),
            ],
            count: 2,
        };
        
        let json = result.to_json();
        assert_eq!(json["type"], "select");
        assert_eq!(json["count"], 2);
        assert!(json["rows"].is_array());
    }
}