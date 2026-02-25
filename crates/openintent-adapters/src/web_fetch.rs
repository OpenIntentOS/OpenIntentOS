//! Web fetch adapter -- fetch any URL and return clean, readable content.
//!
//! Features:
//!   - **Readability extraction** via `readability` crate (like Mozilla Readability)
//!   - **html2text fallback** for pages Readability cannot parse
//!   - **SSRF protection** -- blocks requests to private/internal networks
//!   - **In-memory LRU cache** (15 min TTL, 100 entries) via moka
//!   - Real browser User-Agent to avoid being blocked
//!   - Strips `<script>`, `<style>`, `<nav>`, `<noscript>` before extraction
//!   - Collapses excessive whitespace for cleaner output
//!   - Smart truncation at paragraph boundaries
//!   - Automatic retry on transient failures

use std::io::Cursor;
use std::net::IpAddr;
use std::time::Duration;

use async_trait::async_trait;
use moka::future::Cache;
use serde_json::{Value, json};
use tracing::{debug, info, warn};

use crate::error::{AdapterError, Result};
use crate::traits::{Adapter, AdapterType, AuthRequirement, HealthStatus, ToolDefinition};

// ═══════════════════════════════════════════════════════════════════════
//  Constants
// ═══════════════════════════════════════════════════════════════════════

/// Default maximum content length in characters.
const DEFAULT_MAX_LENGTH: usize = 80_000;

/// Default request timeout in seconds.
const DEFAULT_TIMEOUT_SECS: u64 = 30;

/// Maximum number of retries for transient failures.
const MAX_RETRIES: u32 = 2;

/// Maximum HTML size (in bytes) to feed into Readability.
const READABILITY_MAX_HTML_BYTES: usize = 2_000_000;

/// Cache TTL in minutes.
const CACHE_TTL_MINUTES: u64 = 15;
/// Maximum cached entries.
const CACHE_MAX_ENTRIES: u64 = 100;

/// Realistic browser User-Agent to avoid being blocked.
const BROWSER_USER_AGENT: &str =
    "Mozilla/5.0 (Macintosh; Intel Mac OS X 10_15_7) \
     AppleWebKit/537.36 (KHTML, like Gecko) Chrome/131.0.0.0 Safari/537.36";

// ═══════════════════════════════════════════════════════════════════════
//  Adapter
// ═══════════════════════════════════════════════════════════════════════

/// Web fetch service adapter with Readability extraction, SSRF guard, and caching.
pub struct WebFetchAdapter {
    id: String,
    connected: bool,
    client: reqwest::Client,
    cache: Cache<String, Value>,
}

impl WebFetchAdapter {
    /// Create a new web fetch adapter.
    pub fn new(id: impl Into<String>) -> Self {
        let client = reqwest::Client::builder()
            .user_agent(BROWSER_USER_AGENT)
            .timeout(Duration::from_secs(DEFAULT_TIMEOUT_SECS))
            .redirect(reqwest::redirect::Policy::limited(10))
            .build()
            .unwrap_or_default();

        let cache = Cache::builder()
            .max_capacity(CACHE_MAX_ENTRIES)
            .time_to_live(Duration::from_secs(CACHE_TTL_MINUTES * 60))
            .build();

        Self {
            id: id.into(),
            connected: false,
            client,
            cache,
        }
    }

    /// Fetch a URL and return its content with cache-first, retry, and SSRF guard.
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
        let parsed_url = url::Url::parse(url_str).map_err(|e| AdapterError::InvalidParams {
            tool_name: "web_fetch".into(),
            reason: format!("invalid URL `{url_str}`: {e}"),
        })?;

        // SSRF guard: block requests to private/internal networks.
        check_ssrf(&parsed_url).await?;

        // Check cache first.
        let cache_key = url_str.to_string();
        if let Some(cached) = self.cache.get(&cache_key).await {
            debug!(url = url_str, "returning cached fetch result");
            return Ok(cached);
        }

        debug!(url = url_str, max_length, "fetching URL");

        // Retry loop for transient failures.
        let mut last_error = None;
        for attempt in 0..=MAX_RETRIES {
            if attempt > 0 {
                let delay = Duration::from_millis(500 * u64::from(attempt));
                tokio::time::sleep(delay).await;
                debug!(url = url_str, attempt, "retrying fetch");
            }

            match self.do_fetch(url_str, max_length).await {
                Ok(result) => {
                    self.cache.insert(cache_key, result.clone()).await;
                    return Ok(result);
                }
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
            .header(
                "Accept",
                "text/markdown, text/html;q=0.9, application/xhtml+xml;q=0.8, */*;q=0.1",
            )
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

        // Extract content based on type.
        let (content, extractor) = if content_type.contains("text/markdown") {
            // Cloudflare Markdown for Agents or similar.
            (raw_body.clone(), "markdown-native")
        } else if content_type.contains("text/html") {
            extract_content_from_html(&raw_body, url_str)
        } else if content_type.contains("application/json") {
            let formatted = match serde_json::from_str::<Value>(&raw_body) {
                Ok(parsed) => serde_json::to_string_pretty(&parsed).unwrap_or(raw_body),
                Err(_) => raw_body,
            };
            (formatted, "json")
        } else {
            (raw_body, "raw")
        };

        let (final_content, original_length) = smart_truncate(&content, max_length);

        debug!(
            url = url_str,
            content_type = content_type.as_str(),
            extractor,
            original_length,
            final_length = final_content.len(),
            "fetch completed"
        );

        Ok(json!({
            "url": url_str,
            "content_type": content_type,
            "extractor": extractor,
            "content": final_content,
            "length": original_length,
        }))
    }
}

// ═══════════════════════════════════════════════════════════════════════
//  SSRF protection
// ═══════════════════════════════════════════════════════════════════════

/// Check if a URL targets a private/internal network and block it.
async fn check_ssrf(url: &url::Url) -> Result<()> {
    // Only allow http/https.
    match url.scheme() {
        "http" | "https" => {}
        scheme => {
            return Err(AdapterError::ExecutionFailed {
                tool_name: "web_fetch".into(),
                reason: format!("SSRF blocked: unsupported scheme `{scheme}`"),
            });
        }
    }

    let host = match url.host_str() {
        Some(h) => h,
        None => {
            return Err(AdapterError::ExecutionFailed {
                tool_name: "web_fetch".into(),
                reason: "SSRF blocked: no host in URL".into(),
            });
        }
    };

    // Check if the host is directly an IP address.
    if let Ok(ip) = host.parse::<IpAddr>() {
        if is_private_ip(ip) {
            return Err(AdapterError::ExecutionFailed {
                tool_name: "web_fetch".into(),
                reason: format!("SSRF blocked: {host} is a private IP address"),
            });
        }
        return Ok(());
    }

    // Resolve hostname and check all resulting IPs.
    let port = url.port_or_known_default().unwrap_or(443);
    let addr_str = format!("{host}:{port}");
    let addrs: Vec<std::net::SocketAddr> = tokio::net::lookup_host(&addr_str)
        .await
        .map_err(|e| AdapterError::ExecutionFailed {
            tool_name: "web_fetch".into(),
            reason: format!("DNS resolution failed for `{host}`: {e}"),
        })?
        .collect();

    for addr in &addrs {
        if is_private_ip(addr.ip()) {
            return Err(AdapterError::ExecutionFailed {
                tool_name: "web_fetch".into(),
                reason: format!("SSRF blocked: {host} resolves to private IP {}", addr.ip()),
            });
        }
    }

    Ok(())
}

/// Check if an IP address is private, loopback, link-local, or otherwise internal.
fn is_private_ip(ip: IpAddr) -> bool {
    match ip {
        IpAddr::V4(v4) => {
            v4.is_loopback()
                || v4.is_private()
                || v4.is_link_local()
                || v4.is_unspecified()
                || v4.is_broadcast()
                // 100.64.0.0/10 (CGNAT / Shared Address Space)
                || (v4.octets()[0] == 100 && (v4.octets()[1] & 0xC0) == 64)
                // 192.0.0.0/24 (IETF Protocol Assignments)
                || (v4.octets()[0] == 192 && v4.octets()[1] == 0 && v4.octets()[2] == 0)
        }
        IpAddr::V6(v6) => v6.is_loopback() || v6.is_unspecified(),
    }
}

// ═══════════════════════════════════════════════════════════════════════
//  HTML content extraction (Readability -> html2text -> regex fallback)
// ═══════════════════════════════════════════════════════════════════════

/// Extract readable content from HTML using a multi-stage pipeline.
/// Returns `(content, extractor_name)`.
///
/// Pipeline: Readability -> html2text -> regex fallback.
pub fn extract_content_from_html(html: &str, url_str: &str) -> (String, &'static str) {
    // Guard: skip Readability for very large HTML.
    if html.len() <= READABILITY_MAX_HTML_BYTES {
        // Stage 1: Try Readability (highest quality -- like Mozilla Readability).
        if let Some(text) = try_readability(html, url_str) {
            if text.len() > 100 {
                return (text, "readability");
            }
        }
    }

    // Stage 2: Try html2text (handles more edge cases than regex).
    if let Some(text) = try_html2text(html) {
        if text.len() > 50 {
            return (text, "html2text");
        }
    }

    // Stage 3: Regex-based fallback (always works).
    let text = extract_text_regex(html);
    (text, "regex")
}

/// Try extracting article content via the `readability` crate.
fn try_readability(html: &str, url_str: &str) -> Option<String> {
    let parsed_url = url::Url::parse(url_str).ok()?;
    let mut cursor = Cursor::new(html.as_bytes());

    match readability::extractor::extract(&mut cursor, &parsed_url) {
        Ok(product) => {
            let text = product.text.trim().to_string();
            if text.is_empty() { None } else { Some(text) }
        }
        Err(e) => {
            debug!("readability extraction failed: {e}");
            None
        }
    }
}

/// Try converting HTML to text via the `html2text` crate.
fn try_html2text(html: &str) -> Option<String> {
    let text = html2text::from_read(html.as_bytes(), 120).ok()?;
    let trimmed = text.trim().to_string();
    if trimmed.is_empty() { None } else { Some(trimmed) }
}

/// Regex-based fallback: strip scripts/styles/nav, remove tags, collapse whitespace.
fn extract_text_regex(html: &str) -> String {
    let cleaned = remove_tag_blocks(html, "script");
    let cleaned = remove_tag_blocks(&cleaned, "style");
    let cleaned = remove_tag_blocks(&cleaned, "nav");
    let cleaned = remove_tag_blocks(&cleaned, "noscript");

    let text = strip_html_tags(&cleaned);
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
            result.push_str(&html[cursor..abs_start]);
            if let Some(end) = lower[abs_start..].find(&close_pattern) {
                cursor = abs_start + end + close_pattern.len();
            } else {
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

/// Collapse runs of whitespace into single spaces and limit consecutive newlines.
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
pub fn smart_truncate(content: &str, max_length: usize) -> (String, usize) {
    let original_length = content.len();
    if original_length <= max_length {
        return (content.to_string(), original_length);
    }

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

// ═══════════════════════════════════════════════════════════════════════
//  Adapter trait implementation
// ═══════════════════════════════════════════════════════════════════════

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
            description: "Fetch a URL and return its content as clean text. \
                          Uses Readability extraction for article content, \
                          html2text as fallback. Results are cached for 15 minutes. \
                          Blocks requests to private/internal networks (SSRF protection)."
                .into(),
            parameters: json!({
                "type": "object",
                "properties": {
                    "url": {
                        "type": "string",
                        "description": "The URL to fetch"
                    },
                    "max_length": {
                        "type": "integer",
                        "description": "Maximum content length in characters (default: 80000)"
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

// ═══════════════════════════════════════════════════════════════════════
//  Tests
// ═══════════════════════════════════════════════════════════════════════

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

    // ─────────────────────────────────────────────────────────────────
    //  SSRF tests
    // ─────────────────────────────────────────────────────────────────

    #[test]
    fn is_private_ip_loopback() {
        assert!(is_private_ip("127.0.0.1".parse().unwrap()));
        assert!(is_private_ip("::1".parse().unwrap()));
    }

    #[test]
    fn is_private_ip_rfc1918() {
        assert!(is_private_ip("10.0.0.1".parse().unwrap()));
        assert!(is_private_ip("172.16.0.1".parse().unwrap()));
        assert!(is_private_ip("192.168.1.1".parse().unwrap()));
    }

    #[test]
    fn is_private_ip_link_local() {
        assert!(is_private_ip("169.254.1.1".parse().unwrap()));
    }

    #[test]
    fn is_private_ip_public() {
        assert!(!is_private_ip("8.8.8.8".parse().unwrap()));
        assert!(!is_private_ip("1.1.1.1".parse().unwrap()));
        assert!(!is_private_ip("203.0.113.1".parse().unwrap()));
    }

    #[tokio::test]
    async fn ssrf_blocks_localhost() {
        let url = url::Url::parse("http://127.0.0.1/admin").unwrap();
        assert!(check_ssrf(&url).await.is_err());
    }

    #[tokio::test]
    async fn ssrf_blocks_private_ip() {
        let url = url::Url::parse("http://192.168.1.1/config").unwrap();
        assert!(check_ssrf(&url).await.is_err());
    }

    #[tokio::test]
    async fn ssrf_blocks_file_scheme() {
        let url = url::Url::parse("file:///etc/passwd").unwrap();
        assert!(check_ssrf(&url).await.is_err());
    }

    #[tokio::test]
    async fn ssrf_allows_public_url() {
        let url = url::Url::parse("https://example.com").unwrap();
        // This should succeed (DNS for example.com resolves to public IP).
        assert!(check_ssrf(&url).await.is_ok());
    }

    // ─────────────────────────────────────────────────────────────────
    //  Content extraction tests
    // ─────────────────────────────────────────────────────────────────

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
    fn extract_text_regex_full_pipeline() {
        let html = r#"
        <html>
        <head><style>body{}</style><script>var x=1;</script></head>
        <body>
        <nav>Menu Item</nav>
        <p>Main content here.</p>
        <p>Second paragraph.</p>
        </body>
        </html>"#;
        let text = extract_text_regex(html);
        assert!(text.contains("Main content"));
        assert!(text.contains("Second paragraph"));
        assert!(!text.contains("var x=1"));
        assert!(!text.contains("body{}"));
    }

    #[test]
    fn readability_extracts_article() {
        let html = r#"<!DOCTYPE html>
        <html><head><title>Test Article</title></head>
        <body>
        <nav><a href="/">Home</a><a href="/about">About</a></nav>
        <article>
        <h1>Test Article Title</h1>
        <p>This is the main article content. It contains several sentences
        to ensure that the readability algorithm identifies it as the primary
        content block. The article discusses important topics that are
        relevant to the reader.</p>
        <p>Here is a second paragraph with additional details. The content
        continues with more information that helps establish this as the
        main body of the page rather than navigation or sidebar content.</p>
        </article>
        <footer>Copyright 2025</footer>
        </body></html>"#;
        let (content, extractor) = extract_content_from_html(html, "https://example.com/article");
        assert!(
            content.contains("main article content"),
            "content should contain article text, got: {content}"
        );
        // Should use readability or html2text, not regex.
        assert!(extractor == "readability" || extractor == "html2text");
    }

    #[test]
    fn html2text_works_as_fallback() {
        let html = "<h1>Title</h1><p>Paragraph one.</p><p>Paragraph two.</p>";
        let result = try_html2text(html);
        assert!(result.is_some());
        let text = result.unwrap();
        assert!(text.contains("Title"));
        assert!(text.contains("Paragraph one"));
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
        assert!(content.contains("First paragraph."));
    }

    #[tokio::test]
    async fn cache_stores_and_retrieves() {
        let adapter = WebFetchAdapter::new("wf-test");
        let key = "https://example.com".to_string();
        let val = json!({"url": "https://example.com", "content": "cached"});
        adapter.cache.insert(key.clone(), val.clone()).await;
        assert_eq!(adapter.cache.get(&key).await, Some(val));
    }
}
