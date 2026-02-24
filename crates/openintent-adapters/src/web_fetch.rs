//! Web fetch adapter -- fetch any URL and return its content as text.
//!
//! Features:
//!   - Real browser User-Agent to avoid being blocked
//!   - Strips `<script>`, `<style>`, `<nav>`, `<header>`, `<footer>` before
//!     extracting text from HTML
//!   - Collapses excessive whitespace for cleaner output
//!   - Smart truncation at paragraph boundaries
//!   - Automatic retry on transient failures

use async_trait::async_trait;
use serde_json::{Value, json};
use tracing::{debug, info, warn};

use crate::error::{AdapterError, Result};
use crate::traits::{Adapter, AdapterType, AuthRequirement, HealthStatus, ToolDefinition};

/// Default maximum content length in characters.
const DEFAULT_MAX_LENGTH: usize = 80_000;

/// Default request timeout in seconds.
const DEFAULT_TIMEOUT_SECS: u64 = 30;

/// Maximum number of retries for transient failures.
const MAX_RETRIES: u32 = 2;

/// Realistic browser User-Agent to avoid being blocked.
const BROWSER_USER_AGENT: &str =
    "Mozilla/5.0 (Macintosh; Intel Mac OS X 10_15_7) \
     AppleWebKit/537.36 (KHTML, like Gecko) Chrome/131.0.0.0 Safari/537.36";

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
            .user_agent(BROWSER_USER_AGENT)
            .timeout(std::time::Duration::from_secs(DEFAULT_TIMEOUT_SECS))
            .redirect(reqwest::redirect::Policy::limited(10))
            .build()
            .unwrap_or_default();

        Self {
            id: id.into(),
            connected: false,
            client,
        }
    }

    /// Fetch a URL and return its content with automatic retry.
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

        debug!(url = url_str, max_length, "fetching URL");

        // Retry loop for transient failures.
        let mut last_error = None;
        for attempt in 0..=MAX_RETRIES {
            if attempt > 0 {
                let delay = std::time::Duration::from_millis(500 * u64::from(attempt));
                tokio::time::sleep(delay).await;
                debug!(url = url_str, attempt, "retrying fetch");
            }

            match self.do_fetch(url_str, max_length).await {
                Ok(result) => return Ok(result),
                Err(e) => {
                    warn!(url = url_str, attempt, error = %e, "fetch attempt failed");
                    last_error = Some(e);
                }
            }
        }

        Err(last_error.unwrap_or_else(|| AdapterError::ExecutionFailed {
            tool_name: "web_fetch".into(),
            reason: "all retry attempts exhausted".into(),
        }))
    }

    /// Perform a single fetch attempt.
    async fn do_fetch(&self, url_str: &str, max_length: usize) -> Result<Value> {
        let response = self
            .client
            .get(url_str)
            .header("Accept", "text/html,application/xhtml+xml,application/xml;q=0.9,*/*;q=0.8")
            .header("Accept-Language", "en-US,en;q=0.9,zh-CN;q=0.8,zh;q=0.7")
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
            extract_text_from_html(&raw_body)
        } else if content_type.contains("application/json") {
            match serde_json::from_str::<Value>(&raw_body) {
                Ok(parsed) => serde_json::to_string_pretty(&parsed).unwrap_or(raw_body),
                Err(_) => raw_body,
            }
        } else {
            raw_body
        };

        // Smart truncation.
        let (final_content, original_length) = smart_truncate(&content, max_length);

        debug!(
            url = url_str,
            content_type = content_type.as_str(),
            original_length,
            final_length = final_content.len(),
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

// ═══════════════════════════════════════════════════════════════════════
//  HTML content extraction
// ═══════════════════════════════════════════════════════════════════════

/// Extract readable text from HTML by removing scripts, styles, nav,
/// header, footer, and then stripping remaining tags.
fn extract_text_from_html(html: &str) -> String {
    // 1. Remove <script>...</script>, <style>...</style>, and noise tags.
    let cleaned = remove_tag_blocks(html, "script");
    let cleaned = remove_tag_blocks(&cleaned, "style");
    let cleaned = remove_tag_blocks(&cleaned, "nav");
    let cleaned = remove_tag_blocks(&cleaned, "noscript");

    // 2. Strip remaining HTML tags and decode entities.
    let text = strip_html_tags(&cleaned);

    // 3. Collapse excessive whitespace.
    collapse_whitespace(&text)
}

/// Remove all occurrences of `<tag ...>...</tag>` (case-insensitive).
fn remove_tag_blocks(html: &str, tag: &str) -> String {
    let open_pattern = format!("<{}", tag);
    let close_pattern = format!("</{}>", tag);
    let mut result = String::with_capacity(html.len());
    let lower = html.to_ascii_lowercase();
    let mut cursor = 0;

    while cursor < html.len() {
        if let Some(start) = lower[cursor..].find(&open_pattern) {
            let abs_start = cursor + start;
            // Copy everything before the tag.
            result.push_str(&html[cursor..abs_start]);
            // Find the closing tag.
            if let Some(end) = lower[abs_start..].find(&close_pattern) {
                cursor = abs_start + end + close_pattern.len();
            } else {
                // No closing tag found, skip to end.
                cursor = html.len();
            }
        } else {
            result.push_str(&html[cursor..]);
            break;
        }
    }
    result
}

/// Strip HTML tags from a string and decode common HTML entities.
fn strip_html_tags(input: &str) -> String {
    let mut result = String::with_capacity(input.len());
    let mut inside_tag = false;

    for ch in input.chars() {
        match ch {
            '<' => inside_tag = true,
            '>' => {
                inside_tag = false;
                // Insert a space after closing tags to prevent word merging.
                result.push(' ');
            }
            _ if !inside_tag => result.push(ch),
            _ => {}
        }
    }

    result
        .replace("&amp;", "&")
        .replace("&lt;", "<")
        .replace("&gt;", ">")
        .replace("&quot;", "\"")
        .replace("&#39;", "'")
        .replace("&apos;", "'")
        .replace("&nbsp;", " ")
}

/// Collapse runs of whitespace into single spaces, and multiple newlines
/// into double newlines (paragraph breaks).
fn collapse_whitespace(text: &str) -> String {
    let mut result = String::with_capacity(text.len() / 2);
    let mut prev_newline_count = 0u32;
    let mut prev_was_space = false;

    for ch in text.chars() {
        if ch == '\n' {
            prev_newline_count += 1;
            prev_was_space = false;
            if prev_newline_count <= 2 {
                result.push('\n');
            }
        } else if ch.is_whitespace() {
            prev_newline_count = 0;
            if !prev_was_space {
                result.push(' ');
                prev_was_space = true;
            }
        } else {
            prev_newline_count = 0;
            prev_was_space = false;
            result.push(ch);
        }
    }

    result.trim().to_string()
}

// ═══════════════════════════════════════════════════════════════════════
//  Smart truncation
// ═══════════════════════════════════════════════════════════════════════

/// Truncate content intelligently at a paragraph or sentence boundary.
/// Returns `(content, original_length)`.
fn smart_truncate(content: &str, max_length: usize) -> (String, usize) {
    let original_length = content.len();
    if original_length <= max_length {
        return (content.to_string(), original_length);
    }

    // Try to truncate at the last paragraph break before max_length.
    let search_region = &content[..max_length];
    let truncation_point = search_region
        .rfind("\n\n")
        .or_else(|| search_region.rfind('\n'))
        .or_else(|| search_region.rfind(". "))
        .unwrap_or(max_length);

    let mut truncated = content[..truncation_point].to_string();
    truncated.push_str("\n\n... [content truncated]");
    (truncated, original_length)
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
        assert!(strip_html_tags("<p>Hello</p>").contains("Hello"));
        assert!(strip_html_tags("<div><span>nested</span></div>").contains("nested"));
        assert_eq!(strip_html_tags("plain text"), "plain text");
    }

    #[test]
    fn strip_html_tags_decodes_entities() {
        assert!(strip_html_tags("a &amp; b").contains("a & b"));
    }

    #[test]
    fn remove_tag_blocks_strips_scripts() {
        let html = "<p>Hello</p><script>alert('xss')</script><p>World</p>";
        let result = remove_tag_blocks(html, "script");
        assert!(!result.contains("alert"));
        assert!(result.contains("Hello"));
        assert!(result.contains("World"));
    }

    #[test]
    fn remove_tag_blocks_strips_styles() {
        let html = "<style type=\"text/css\">body { color: red; }</style><p>Content</p>";
        let result = remove_tag_blocks(html, "style");
        assert!(!result.contains("color: red"));
        assert!(result.contains("Content"));
    }

    #[test]
    fn extract_text_from_html_full_pipeline() {
        let html = r#"
        <html>
        <head><style>body{}</style><script>var x=1;</script></head>
        <body>
        <nav>Menu Item</nav>
        <p>Main content here.</p>
        <p>Second paragraph.</p>
        </body>
        </html>"#;
        let text = extract_text_from_html(html);
        assert!(text.contains("Main content"));
        assert!(text.contains("Second paragraph"));
        assert!(!text.contains("var x=1"));
        assert!(!text.contains("body{}"));
    }

    #[test]
    fn collapse_whitespace_reduces_spaces() {
        let input = "hello    world\n\n\n\n\nfoo";
        let result = collapse_whitespace(input);
        assert!(result.contains("hello world"));
        assert!(!result.contains("\n\n\n"));
    }

    #[test]
    fn smart_truncate_short_text() {
        let (content, len) = smart_truncate("short", 100);
        assert_eq!(content, "short");
        assert_eq!(len, 5);
    }

    #[test]
    fn smart_truncate_at_paragraph_boundary() {
        let text = "First paragraph.\n\nSecond paragraph.\n\nThird paragraph is longer.";
        let (content, len) = smart_truncate(text, 40);
        assert_eq!(len, text.len());
        assert!(content.contains("[content truncated]"));
        // Should truncate at paragraph boundary.
        assert!(content.contains("First paragraph."));
    }
}
