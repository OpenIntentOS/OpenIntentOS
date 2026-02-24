//! Generic HTTP request adapter -- make arbitrary HTTP requests with full control.
//!
//! This adapter supports all common HTTP methods (GET, POST, PUT, PATCH,
//! DELETE, HEAD) with configurable headers, body, and timeout.  It returns
//! the full response including status code, headers, body, and elapsed time.

use async_trait::async_trait;
use serde_json::{Value, json};
use std::collections::HashMap;
use std::time::Instant;
use tracing::{debug, info};

use crate::error::{AdapterError, Result};
use crate::traits::{Adapter, AdapterType, AuthRequirement, HealthStatus, ToolDefinition};

/// Default request timeout in seconds.
const DEFAULT_TIMEOUT_SECS: u64 = 30;

/// Maximum response body size in bytes (1 MB).
const MAX_BODY_BYTES: usize = 1_024 * 1_024;

/// Generic HTTP request service adapter.
pub struct HttpRequestAdapter {
    /// Unique identifier for this adapter instance.
    id: String,
    /// Whether the adapter has been connected.
    connected: bool,
    /// HTTP client for making requests.
    client: reqwest::Client,
}

impl HttpRequestAdapter {
    /// Create a new HTTP request adapter.
    pub fn new(id: impl Into<String>) -> Self {
        let client = reqwest::Client::builder()
            .user_agent("OpenIntentOS/0.1")
            .build()
            .unwrap_or_default();

        Self {
            id: id.into(),
            connected: false,
            client,
        }
    }

    /// Execute an HTTP request and return the full response.
    async fn tool_http_request(&self, params: Value) -> Result<Value> {
        let method_str = params
            .get("method")
            .and_then(|v| v.as_str())
            .ok_or_else(|| AdapterError::InvalidParams {
                tool_name: "http_request".into(),
                reason: "missing required string field `method`".into(),
            })?;

        let url_str = params.get("url").and_then(|v| v.as_str()).ok_or_else(|| {
            AdapterError::InvalidParams {
                tool_name: "http_request".into(),
                reason: "missing required string field `url`".into(),
            }
        })?;

        let timeout_secs = params
            .get("timeout_seconds")
            .and_then(|v| v.as_u64())
            .unwrap_or(DEFAULT_TIMEOUT_SECS);

        // Parse and validate method.
        let method = parse_method(method_str).ok_or_else(|| AdapterError::InvalidParams {
            tool_name: "http_request".into(),
            reason: format!(
                "unsupported HTTP method `{method_str}`. Supported: GET, POST, PUT, PATCH, DELETE, HEAD"
            ),
        })?;

        // Validate URL.
        let _parsed_url = url::Url::parse(url_str).map_err(|e| AdapterError::InvalidParams {
            tool_name: "http_request".into(),
            reason: format!("invalid URL `{url_str}`: {e}"),
        })?;

        debug!(
            method = method_str,
            url = url_str,
            timeout_secs = timeout_secs,
            "executing HTTP request"
        );

        // Build the request.
        let mut request_builder = self
            .client
            .request(method, url_str)
            .timeout(std::time::Duration::from_secs(timeout_secs));

        // Add custom headers.
        if let Some(headers) = params.get("headers").and_then(|v| v.as_object()) {
            for (key, value) in headers {
                if let Some(val_str) = value.as_str() {
                    let header_name = reqwest::header::HeaderName::from_bytes(key.as_bytes())
                        .map_err(|e| AdapterError::InvalidParams {
                            tool_name: "http_request".into(),
                            reason: format!("invalid header name `{key}`: {e}"),
                        })?;
                    let header_value =
                        reqwest::header::HeaderValue::from_str(val_str).map_err(|e| {
                            AdapterError::InvalidParams {
                                tool_name: "http_request".into(),
                                reason: format!("invalid header value for `{key}`: {e}"),
                            }
                        })?;
                    request_builder = request_builder.header(header_name, header_value);
                }
            }
        }

        // Add body if provided.
        if let Some(body) = params.get("body").and_then(|v| v.as_str()) {
            request_builder = request_builder.body(body.to_string());
        }

        // Send the request and measure elapsed time.
        let start = Instant::now();
        let response = request_builder.send().await.map_err(|e| {
            if e.is_timeout() {
                AdapterError::Timeout {
                    seconds: timeout_secs,
                    reason: format!("HTTP request to `{url_str}` timed out"),
                }
            } else {
                AdapterError::ExecutionFailed {
                    tool_name: "http_request".into(),
                    reason: format!("HTTP request failed: {e}"),
                }
            }
        })?;
        let elapsed_ms = start.elapsed().as_millis() as u64;

        let status = response.status().as_u16();

        // Collect response headers.
        let response_headers: HashMap<String, String> = response
            .headers()
            .iter()
            .map(|(k, v)| {
                (
                    k.as_str().to_string(),
                    v.to_str().unwrap_or("<binary>").to_string(),
                )
            })
            .collect();

        // Read the response body with size limit.
        let body_bytes = response
            .bytes()
            .await
            .map_err(|e| AdapterError::ExecutionFailed {
                tool_name: "http_request".into(),
                reason: format!("failed to read response body: {e}"),
            })?;

        let body = if body_bytes.len() > MAX_BODY_BYTES {
            let truncated = String::from_utf8_lossy(&body_bytes[..MAX_BODY_BYTES]);
            format!("{truncated}\n... [body truncated at 1 MB]")
        } else {
            String::from_utf8_lossy(&body_bytes).into_owned()
        };

        debug!(
            method = method_str,
            url = url_str,
            status = status,
            elapsed_ms = elapsed_ms,
            body_length = body_bytes.len(),
            "HTTP request completed"
        );

        Ok(json!({
            "status": status,
            "headers": response_headers,
            "body": body,
            "elapsed_ms": elapsed_ms,
        }))
    }
}

/// Parse an HTTP method string into a `reqwest::Method`.
/// Returns `None` if the method is not supported.
fn parse_method(method: &str) -> Option<reqwest::Method> {
    match method.to_uppercase().as_str() {
        "GET" => Some(reqwest::Method::GET),
        "POST" => Some(reqwest::Method::POST),
        "PUT" => Some(reqwest::Method::PUT),
        "PATCH" => Some(reqwest::Method::PATCH),
        "DELETE" => Some(reqwest::Method::DELETE),
        "HEAD" => Some(reqwest::Method::HEAD),
        _ => None,
    }
}

#[async_trait]
impl Adapter for HttpRequestAdapter {
    fn id(&self) -> &str {
        &self.id
    }

    fn adapter_type(&self) -> AdapterType {
        AdapterType::DevTools
    }

    async fn connect(&mut self) -> Result<()> {
        info!(id = %self.id, "HTTP request adapter connected");
        self.connected = true;
        Ok(())
    }

    async fn disconnect(&mut self) -> Result<()> {
        info!(id = %self.id, "HTTP request adapter disconnected");
        self.connected = false;
        Ok(())
    }

    async fn health_check(&self) -> Result<HealthStatus> {
        if !self.connected {
            return Ok(HealthStatus::Unhealthy);
        }
        Ok(HealthStatus::Healthy)
    }

    fn tools(&self) -> Vec<ToolDefinition> {
        vec![ToolDefinition {
            name: "http_request".into(),
            description:
                "Make an arbitrary HTTP request with full control over method, headers, and body"
                    .into(),
            parameters: json!({
                "type": "object",
                "properties": {
                    "method": {
                        "type": "string",
                        "description": "HTTP method: GET, POST, PUT, PATCH, DELETE, or HEAD",
                        "enum": ["GET", "POST", "PUT", "PATCH", "DELETE", "HEAD"]
                    },
                    "url": {
                        "type": "string",
                        "description": "The URL to send the request to"
                    },
                    "headers": {
                        "type": "object",
                        "description": "Optional headers as key-value pairs",
                        "additionalProperties": { "type": "string" }
                    },
                    "body": {
                        "type": "string",
                        "description": "Optional request body"
                    },
                    "timeout_seconds": {
                        "type": "integer",
                        "description": "Request timeout in seconds (default: 30)"
                    }
                },
                "required": ["method", "url"]
            }),
        }]
    }

    async fn execute_tool(&self, name: &str, params: Value) -> Result<Value> {
        if !self.connected {
            return Err(AdapterError::ExecutionFailed {
                tool_name: name.to_string(),
                reason: format!("adapter `{}` is not connected", self.id),
            });
        }
        match name {
            "http_request" => self.tool_http_request(params).await,
            _ => Err(AdapterError::ToolNotFound {
                adapter_id: self.id.clone(),
                tool_name: name.to_string(),
            }),
        }
    }

    fn required_auth(&self) -> Option<AuthRequirement> {
        None
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn http_request_adapter_tools_list() {
        let adapter = HttpRequestAdapter::new("hr-test");
        let tools = adapter.tools();
        assert_eq!(tools.len(), 1);
        assert_eq!(tools[0].name, "http_request");
    }

    #[tokio::test]
    async fn http_request_adapter_connect_disconnect() {
        let mut adapter = HttpRequestAdapter::new("hr-test");
        assert!(!adapter.connected);

        adapter.connect().await.unwrap();
        assert!(adapter.connected);

        adapter.disconnect().await.unwrap();
        assert!(!adapter.connected);
    }

    #[tokio::test]
    async fn http_request_adapter_health_when_disconnected() {
        let adapter = HttpRequestAdapter::new("hr-test");
        let status = adapter.health_check().await.unwrap();
        assert_eq!(status, HealthStatus::Unhealthy);
    }

    #[tokio::test]
    async fn http_request_adapter_health_when_connected() {
        let mut adapter = HttpRequestAdapter::new("hr-test");
        adapter.connect().await.unwrap();
        let status = adapter.health_check().await.unwrap();
        assert_eq!(status, HealthStatus::Healthy);
    }

    #[tokio::test]
    async fn http_request_adapter_rejects_when_not_connected() {
        let adapter = HttpRequestAdapter::new("hr-test");
        let result = adapter
            .execute_tool(
                "http_request",
                json!({"method": "GET", "url": "https://example.com"}),
            )
            .await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn http_request_adapter_rejects_unknown_tool() {
        let mut adapter = HttpRequestAdapter::new("hr-test");
        adapter.connect().await.unwrap();
        let result = adapter.execute_tool("nonexistent", json!({})).await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn http_request_adapter_rejects_invalid_method() {
        let mut adapter = HttpRequestAdapter::new("hr-test");
        adapter.connect().await.unwrap();
        let result = adapter
            .execute_tool(
                "http_request",
                json!({"method": "FOOBAR", "url": "https://example.com"}),
            )
            .await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn http_request_adapter_rejects_invalid_url() {
        let mut adapter = HttpRequestAdapter::new("hr-test");
        adapter.connect().await.unwrap();
        let result = adapter
            .execute_tool("http_request", json!({"method": "GET", "url": "not a url"}))
            .await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn http_request_adapter_rejects_missing_method() {
        let mut adapter = HttpRequestAdapter::new("hr-test");
        adapter.connect().await.unwrap();
        let result = adapter
            .execute_tool("http_request", json!({"url": "https://example.com"}))
            .await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn http_request_adapter_rejects_missing_url() {
        let mut adapter = HttpRequestAdapter::new("hr-test");
        adapter.connect().await.unwrap();
        let result = adapter
            .execute_tool("http_request", json!({"method": "GET"}))
            .await;
        assert!(result.is_err());
    }

    #[test]
    fn parse_method_supported_methods() {
        assert_eq!(parse_method("GET"), Some(reqwest::Method::GET));
        assert_eq!(parse_method("post"), Some(reqwest::Method::POST));
        assert_eq!(parse_method("Put"), Some(reqwest::Method::PUT));
        assert_eq!(parse_method("PATCH"), Some(reqwest::Method::PATCH));
        assert_eq!(parse_method("DELETE"), Some(reqwest::Method::DELETE));
        assert_eq!(parse_method("HEAD"), Some(reqwest::Method::HEAD));
    }

    #[test]
    fn parse_method_unsupported_returns_none() {
        assert_eq!(parse_method("FOOBAR"), None);
        assert_eq!(parse_method("OPTIONS"), None);
        assert_eq!(parse_method(""), None);
    }
}
