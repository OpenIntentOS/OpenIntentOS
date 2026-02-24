//! Web fetch adapter -- fetch any URL and return its content as text.
//!
//! This adapter retrieves web pages, JSON APIs, or any other URL-accessible
//! resource and returns the content as structured text.  HTML content is
//! automatically stripped of tags to provide clean plain text.

use async_trait::async_trait;
use serde_json::{Value, json};
use tracing::{debug, info};

use crate::error::{AdapterError, Result};
use crate::traits::{Adapter, AdapterType, AuthRequirement, HealthStatus, ToolDefinition};

/// Default maximum content length in characters.
const DEFAULT_MAX_LENGTH: usize = 50_000;

/// Default request timeout in seconds.
const DEFAULT_TIMEOUT_SECS: u64 = 30;

/// Web fetch service adapter.
pub struct WebFetchAdapter {
    /// Unique identifier for this adapter instance.
    id: String,
    /// Whether the adapter has been connected.
    connected: bool,
    /// HTTP client for making requests.
    client: reqwest::Client,
}

impl WebFetchAdapter {
    /// Create a new web fetch adapter.
    pub fn new(id: impl Into<String>) -> Self {
        let client = reqwest::Client::builder()
            .user_agent("OpenIntentOS/0.1")
            .timeout(std::time::Duration::from_secs(DEFAULT_TIMEOUT_SECS))
            .build()
            .unwrap_or_default();

        Self {
            id: id.into(),
            connected: false,
            client,
        }
    }

    /// Fetch a URL and return its content.
    async fn tool_web_fetch(&self, params: Value) -> Result<Value> {
        let url_str = params.get("url").and_then(|v| v.as_str()).ok_or_else(|| {
            AdapterError::InvalidParams {
                tool_name: "web_fetch".into(),
                reason: "missing required string field `url`".into(),
            }
        })?;

        let max_length = params
            .get("max_length")
            .and_then(|v| v.as_u64())
            .map(|v| v as usize)
            .unwrap_or(DEFAULT_MAX_LENGTH);

        // Validate URL.
        let _parsed_url = url::Url::parse(url_str).map_err(|e| AdapterError::InvalidParams {
            tool_name: "web_fetch".into(),
            reason: format!("invalid URL `{url_str}`: {e}"),
        })?;

        debug!(url = url_str, max_length = max_length, "fetching URL");

        let response =
            self.client
                .get(url_str)
                .send()
                .await
                .map_err(|e| AdapterError::ExecutionFailed {
                    tool_name: "web_fetch".into(),
                    reason: format!("HTTP request failed: {e}"),
                })?;

        if !response.status().is_success() {
            return Err(AdapterError::ExecutionFailed {
                tool_name: "web_fetch".into(),
                reason: format!("server returned status {}", response.status()),
            });
        }

        // Determine content type from the response headers.
        let content_type = response
            .headers()
            .get(reqwest::header::CONTENT_TYPE)
            .and_then(|v| v.to_str().ok())
            .unwrap_or("text/plain")
            .to_string();

        let raw_body = response
            .text()
            .await
            .map_err(|e| AdapterError::ExecutionFailed {
                tool_name: "web_fetch".into(),
                reason: format!("failed to read response body: {e}"),
            })?;

        // Process content based on type.
        let content = if content_type.contains("text/html") {
            strip_html_tags(&raw_body)
        } else if content_type.contains("application/json") {
            // Try to pretty-print JSON.
            match serde_json::from_str::<Value>(&raw_body) {
                Ok(parsed) => serde_json::to_string_pretty(&parsed).unwrap_or(raw_body),
                Err(_) => raw_body,
            }
        } else {
            raw_body
        };

        // Truncate to max_length.
        let (final_content, original_length) = truncate_content(&content, max_length);

        debug!(
            url = url_str,
            content_type = content_type.as_str(),
            length = original_length,
            "fetch completed"
        );

        Ok(json!({
            "url": url_str,
            "content_type": content_type,
            "content": final_content,
            "length": original_length,
        }))
    }
}

/// Truncate content to `max_length` characters.
/// Returns `(content, original_length)`.
fn truncate_content(content: &str, max_length: usize) -> (String, usize) {
    let original_length = content.len();
    if original_length <= max_length {
        (content.to_string(), original_length)
    } else {
        let mut truncated = content[..max_length].to_string();
        truncated.push_str("\n... [content truncated]");
        (truncated, original_length)
    }
}

/// Strip HTML tags from a string, removing `<...>` sequences and decoding
/// common HTML entities.
fn strip_html_tags(input: &str) -> String {
    let mut result = String::with_capacity(input.len());
    let mut inside_tag = false;

    for ch in input.chars() {
        match ch {
            '<' => inside_tag = true,
            '>' => inside_tag = false,
            _ if !inside_tag => result.push(ch),
            _ => {}
        }
    }

    // Decode common HTML entities.
    result
        .replace("&amp;", "&")
        .replace("&lt;", "<")
        .replace("&gt;", ">")
        .replace("&quot;", "\"")
        .replace("&#39;", "'")
        .replace("&apos;", "'")
        .replace("&nbsp;", " ")
}

#[async_trait]
impl Adapter for WebFetchAdapter {
    fn id(&self) -> &str {
        &self.id
    }

    fn adapter_type(&self) -> AdapterType {
        AdapterType::Productivity
    }

    async fn connect(&mut self) -> Result<()> {
        info!(id = %self.id, "web fetch adapter connected");
        self.connected = true;
        Ok(())
    }

    async fn disconnect(&mut self) -> Result<()> {
        info!(id = %self.id, "web fetch adapter disconnected");
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
            name: "web_fetch".into(),
            description:
                "Fetch a URL and return its content as text (HTML is automatically stripped)".into(),
            parameters: json!({
                "type": "object",
                "properties": {
                    "url": {
                        "type": "string",
                        "description": "The URL to fetch"
                    },
                    "max_length": {
                        "type": "integer",
                        "description": "Maximum content length in characters (default: 50000)"
                    }
                },
                "required": ["url"]
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
            "web_fetch" => self.tool_web_fetch(params).await,
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
    fn web_fetch_adapter_tools_list() {
        let adapter = WebFetchAdapter::new("wf-test");
        let tools = adapter.tools();
        assert_eq!(tools.len(), 1);
        assert_eq!(tools[0].name, "web_fetch");
    }

    #[tokio::test]
    async fn web_fetch_adapter_connect_disconnect() {
        let mut adapter = WebFetchAdapter::new("wf-test");
        assert!(!adapter.connected);

        adapter.connect().await.unwrap();
        assert!(adapter.connected);

        adapter.disconnect().await.unwrap();
        assert!(!adapter.connected);
    }

    #[tokio::test]
    async fn web_fetch_adapter_health_when_disconnected() {
        let adapter = WebFetchAdapter::new("wf-test");
        let status = adapter.health_check().await.unwrap();
        assert_eq!(status, HealthStatus::Unhealthy);
    }

    #[tokio::test]
    async fn web_fetch_adapter_health_when_connected() {
        let mut adapter = WebFetchAdapter::new("wf-test");
        adapter.connect().await.unwrap();
        let status = adapter.health_check().await.unwrap();
        assert_eq!(status, HealthStatus::Healthy);
    }

    #[tokio::test]
    async fn web_fetch_adapter_rejects_when_not_connected() {
        let adapter = WebFetchAdapter::new("wf-test");
        let result = adapter
            .execute_tool("web_fetch", json!({"url": "https://example.com"}))
            .await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn web_fetch_adapter_rejects_unknown_tool() {
        let mut adapter = WebFetchAdapter::new("wf-test");
        adapter.connect().await.unwrap();
        let result = adapter.execute_tool("nonexistent", json!({})).await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn web_fetch_adapter_rejects_invalid_url() {
        let mut adapter = WebFetchAdapter::new("wf-test");
        adapter.connect().await.unwrap();
        let result = adapter
            .execute_tool("web_fetch", json!({"url": "not a valid url"}))
            .await;
        assert!(result.is_err());
    }

    #[test]
    fn strip_html_tags_removes_tags() {
        assert_eq!(strip_html_tags("<p>Hello</p>"), "Hello");
        assert_eq!(strip_html_tags("<div><span>nested</span></div>"), "nested");
        assert_eq!(strip_html_tags("plain text"), "plain text");
    }

    #[test]
    fn strip_html_tags_decodes_entities() {
        assert_eq!(strip_html_tags("a &amp; b"), "a & b");
        assert_eq!(strip_html_tags("1 &lt; 2 &gt; 0"), "1 < 2 > 0");
    }

    #[test]
    fn truncate_content_short_text() {
        let (content, len) = truncate_content("short", 100);
        assert_eq!(content, "short");
        assert_eq!(len, 5);
    }

    #[test]
    fn truncate_content_long_text() {
        let long_text = "x".repeat(200);
        let (content, len) = truncate_content(&long_text, 50);
        assert_eq!(len, 200);
        assert!(content.contains("[content truncated]"));
        assert!(content.len() < 100); // 50 chars + truncation message
    }
}
