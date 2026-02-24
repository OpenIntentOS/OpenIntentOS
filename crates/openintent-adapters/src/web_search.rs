//! Web search adapter -- multi-engine web search with automatic fallback.
//!
//! Search priority:
//!   1. Brave Search API (if `BRAVE_API_KEY` is set) -- best quality
//!   2. DuckDuckGo HTML scraping (no key needed) -- fallback
//!
//! Both backends return unified JSON results with title, URL, and snippet.

use async_trait::async_trait;
use serde_json::{Value, json};
use tracing::{debug, info, warn};

use crate::error::{AdapterError, Result};
use crate::traits::{Adapter, AdapterType, AuthRequirement, HealthStatus, ToolDefinition};

// ═══════════════════════════════════════════════════════════════════════
//  Constants
// ═══════════════════════════════════════════════════════════════════════

/// Default maximum number of search results to return.
const DEFAULT_MAX_RESULTS: usize = 10;

/// DuckDuckGo HTML search endpoint.
const DUCKDUCKGO_HTML_URL: &str = "https://html.duckduckgo.com/html/";

/// Brave Search API endpoint.
const BRAVE_SEARCH_URL: &str = "https://api.search.brave.com/res/v1/web/search";

/// Realistic browser User-Agent to avoid being blocked.
const BROWSER_USER_AGENT: &str =
    "Mozilla/5.0 (Macintosh; Intel Mac OS X 10_15_7) \
     AppleWebKit/537.36 (KHTML, like Gecko) Chrome/131.0.0.0 Safari/537.36";

// ═══════════════════════════════════════════════════════════════════════
//  Adapter
// ═══════════════════════════════════════════════════════════════════════

/// Web search service adapter with multi-engine support.
pub struct WebSearchAdapter {
    id: String,
    connected: bool,
    client: reqwest::Client,
    /// Brave Search API key (if available).
    brave_api_key: Option<String>,
}

impl WebSearchAdapter {
    /// Create a new web search adapter.
    pub fn new(id: impl Into<String>) -> Self {
        let client = reqwest::Client::builder()
            .user_agent(BROWSER_USER_AGENT)
            .timeout(std::time::Duration::from_secs(30))
            .build()
            .unwrap_or_default();

        let brave_api_key = std::env::var("BRAVE_API_KEY")
            .ok()
            .filter(|k| !k.is_empty());

        Self {
            id: id.into(),
            connected: false,
            client,
            brave_api_key,
        }
    }

    /// Execute a web search, trying Brave first then falling back to DDG.
    async fn tool_web_search(&self, params: Value) -> Result<Value> {
        let query = params
            .get("query")
            .and_then(|v| v.as_str())
            .ok_or_else(|| AdapterError::InvalidParams {
                tool_name: "web_search".into(),
                reason: "missing required string field `query`".into(),
            })?;

        let max_results = params
            .get("max_results")
            .and_then(|v| v.as_u64())
            .map(|v| v as usize)
            .unwrap_or(DEFAULT_MAX_RESULTS);

        debug!(query, max_results, "performing web search");

        // Try Brave Search API first (higher quality).
        if let Some(ref api_key) = self.brave_api_key {
            match self.search_brave(query, max_results, api_key).await {
                Ok(results) if !results.is_empty() => {
                    debug!(count = results.len(), engine = "brave", "search completed");
                    return Ok(json!({ "engine": "brave", "results": results }));
                }
                Ok(_) => debug!("Brave returned no results, falling back to DuckDuckGo"),
                Err(e) => warn!(error = %e, "Brave Search failed, falling back"),
            }
        }

        // Fallback: DuckDuckGo HTML scraping.
        let results = self.search_duckduckgo(query, max_results).await?;
        debug!(count = results.len(), engine = "duckduckgo", "search completed");

        Ok(json!({ "engine": "duckduckgo", "results": results }))
    }

    // ───────────────────────────────────────────────────────────────────
    //  Brave Search API
    // ───────────────────────────────────────────────────────────────────

    async fn search_brave(
        &self,
        query: &str,
        max_results: usize,
        api_key: &str,
    ) -> Result<Vec<Value>> {
        let response = self
            .client
            .get(BRAVE_SEARCH_URL)
            .header("X-Subscription-Token", api_key)
            .header("Accept", "application/json")
            .query(&[("q", query), ("count", &max_results.to_string())])
            .send()
            .await
            .map_err(|e| AdapterError::ExecutionFailed {
                tool_name: "web_search".into(),
                reason: format!("Brave Search request failed: {e}"),
            })?;

        if !response.status().is_success() {
            return Err(AdapterError::ExecutionFailed {
                tool_name: "web_search".into(),
                reason: format!("Brave Search returned status {}", response.status()),
            });
        }

        let body: Value = response.json().await.map_err(|e| {
            AdapterError::ExecutionFailed {
                tool_name: "web_search".into(),
                reason: format!("failed to parse Brave response: {e}"),
            }
        })?;

        let mut results = Vec::new();
        if let Some(web_results) = body.pointer("/web/results").and_then(|v| v.as_array()) {
            for item in web_results.iter().take(max_results) {
                let title = item.get("title").and_then(|v| v.as_str()).unwrap_or("");
                let url = item.get("url").and_then(|v| v.as_str()).unwrap_or("");
                let snippet = item
                    .get("description")
                    .and_then(|v| v.as_str())
                    .unwrap_or("");

                if !title.is_empty() || !url.is_empty() {
                    results.push(json!({
                        "title": strip_html_tags(title),
                        "url": url,
                        "snippet": strip_html_tags(snippet),
                    }));
                }
            }
        }
        Ok(results)
    }

    // ───────────────────────────────────────────────────────────────────
    //  DuckDuckGo HTML scraping
    // ───────────────────────────────────────────────────────────────────

    async fn search_duckduckgo(&self, query: &str, max_results: usize) -> Result<Vec<Value>> {
        // POST with form data is more reliable than GET for DDG.
        let response = self
            .client
            .post(DUCKDUCKGO_HTML_URL)
            .form(&[("q", query), ("kl", ""), ("df", "")])
            .send()
            .await
            .map_err(|e| AdapterError::ExecutionFailed {
                tool_name: "web_search".into(),
                reason: format!("DuckDuckGo request failed: {e}"),
            })?;

        if !response.status().is_success() {
            return Err(AdapterError::ExecutionFailed {
                tool_name: "web_search".into(),
                reason: format!("DuckDuckGo returned status {}", response.status()),
            });
        }

        let html = response.text().await.map_err(|e| {
            AdapterError::ExecutionFailed {
                tool_name: "web_search".into(),
                reason: format!("failed to read DuckDuckGo response: {e}"),
            }
        })?;

        Ok(parse_duckduckgo_results(&html, max_results))
    }
}

// ═══════════════════════════════════════════════════════════════════════
//  DuckDuckGo HTML parsing
// ═══════════════════════════════════════════════════════════════════════

fn parse_duckduckgo_results(html: &str, max_results: usize) -> Vec<Value> {
    let mut results = Vec::new();

    let title_marker = "class=\"result__a\"";
    let snippet_marker = "class=\"result__snippet\"";

    let mut title_positions: Vec<usize> = Vec::new();
    let mut search_from = 0;
    while let Some(pos) = html[search_from..].find(title_marker) {
        title_positions.push(search_from + pos);
        search_from = search_from + pos + title_marker.len();
    }

    let mut snippet_positions: Vec<usize> = Vec::new();
    search_from = 0;
    while let Some(pos) = html[search_from..].find(snippet_marker) {
        snippet_positions.push(search_from + pos);
        search_from = search_from + pos + snippet_marker.len();
    }

    for (i, &title_pos) in title_positions.iter().enumerate() {
        if results.len() >= max_results {
            break;
        }

        let before_marker = &html[..title_pos];
        let url = extract_href_before(before_marker).unwrap_or_default();
        let clean_url = clean_ddg_url(&url);

        let after_marker = &html[title_pos + title_marker.len()..];
        let title = extract_tag_text(after_marker, "</a>");

        let snippet = if i < snippet_positions.len() {
            let after_snippet = &html[snippet_positions[i] + snippet_marker.len()..];
            let raw = extract_tag_text(after_snippet, "</");
            strip_html_tags(&raw)
        } else {
            String::new()
        };

        if !title.is_empty() || !clean_url.is_empty() {
            results.push(json!({
                "title": strip_html_tags(&title),
                "url": clean_url,
                "snippet": snippet.trim(),
            }));
        }
    }

    results
}

/// Clean DuckDuckGo tracking URLs to extract the actual destination URL.
fn clean_ddg_url(url: &str) -> String {
    if url.contains("duckduckgo.com/l/") {
        if let Some(uddg_start) = url.find("uddg=") {
            let encoded = &url[uddg_start + 5..];
            let encoded = encoded.split('&').next().unwrap_or(encoded);
            return url_decode(encoded);
        }
    }
    if url.starts_with("//") {
        return format!("https:{url}");
    }
    url.to_string()
}

/// Simple URL percent-decoding.
fn url_decode(input: &str) -> String {
    let mut result = String::with_capacity(input.len());
    let mut chars = input.chars();
    while let Some(ch) = chars.next() {
        if ch == '%' {
            let hex: String = chars.by_ref().take(2).collect();
            if let Ok(byte) = u8::from_str_radix(&hex, 16) {
                result.push(byte as char);
            } else {
                result.push('%');
                result.push_str(&hex);
            }
        } else if ch == '+' {
            result.push(' ');
        } else {
            result.push(ch);
        }
    }
    result
}

fn extract_href_before(html_before: &str) -> Option<String> {
    let href_marker = "href=\"";
    let last_href = html_before.rfind(href_marker)?;
    let start = last_href + href_marker.len();
    let remaining = &html_before[start..];
    let end = remaining.find('"')?;
    Some(remaining[..end].to_string())
}

fn extract_tag_text(html_after_marker: &str, end_marker: &str) -> String {
    let closing_bracket = match html_after_marker.find('>') {
        Some(pos) => pos,
        None => return String::new(),
    };
    let content = &html_after_marker[closing_bracket + 1..];
    let end = match content.find(end_marker) {
        Some(pos) => pos,
        None => content.len(),
    };
    content[..end].to_string()
}

/// Strip HTML tags from a string and decode common HTML entities.
pub fn strip_html_tags(input: &str) -> String {
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

    result
        .replace("&amp;", "&")
        .replace("&lt;", "<")
        .replace("&gt;", ">")
        .replace("&quot;", "\"")
        .replace("&#39;", "'")
        .replace("&apos;", "'")
        .replace("&nbsp;", " ")
}

// ═══════════════════════════════════════════════════════════════════════
//  Adapter trait implementation
// ═══════════════════════════════════════════════════════════════════════

#[async_trait]
impl Adapter for WebSearchAdapter {
    fn id(&self) -> &str {
        &self.id
    }

    fn adapter_type(&self) -> AdapterType {
        AdapterType::Productivity
    }

    async fn connect(&mut self) -> Result<()> {
        let engine = if self.brave_api_key.is_some() {
            "brave+duckduckgo"
        } else {
            "duckduckgo"
        };
        info!(id = %self.id, engine, "web search adapter connected");
        self.connected = true;
        Ok(())
    }

    async fn disconnect(&mut self) -> Result<()> {
        info!(id = %self.id, "web search adapter disconnected");
        self.connected = false;
        Ok(())
    }

    async fn health_check(&self) -> Result<HealthStatus> {
        if !self.connected {
            return Ok(HealthStatus::Unhealthy);
        }
        match self.client.head(DUCKDUCKGO_HTML_URL).send().await {
            Ok(resp) if resp.status().is_success() || resp.status().is_redirection() => {
                Ok(HealthStatus::Healthy)
            }
            Ok(resp) => {
                warn!(status = %resp.status(), "search health check non-success");
                Ok(HealthStatus::Degraded)
            }
            Err(e) => {
                warn!(error = %e, "search health check failed");
                Ok(HealthStatus::Unhealthy)
            }
        }
    }

    fn tools(&self) -> Vec<ToolDefinition> {
        vec![ToolDefinition {
            name: "web_search".into(),
            description: "Search the web and return titles, URLs, and snippets. \
                          Uses Brave Search (if BRAVE_API_KEY is set) with \
                          DuckDuckGo as fallback. Returns up to 10 results by default."
                .into(),
            parameters: json!({
                "type": "object",
                "properties": {
                    "query": {
                        "type": "string",
                        "description": "The search query"
                    },
                    "max_results": {
                        "type": "integer",
                        "description": "Maximum number of results (default: 10)"
                    }
                },
                "required": ["query"]
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
            "web_search" => self.tool_web_search(params).await,
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
    fn web_search_adapter_tools_list() {
        let adapter = WebSearchAdapter::new("ws-test");
        let tools = adapter.tools();
        assert_eq!(tools.len(), 1);
        assert_eq!(tools[0].name, "web_search");
    }

    #[tokio::test]
    async fn web_search_adapter_connect_disconnect() {
        let mut adapter = WebSearchAdapter::new("ws-test");
        assert!(!adapter.connected);
        adapter.connect().await.unwrap();
        assert!(adapter.connected);
        adapter.disconnect().await.unwrap();
        assert!(!adapter.connected);
    }

    #[tokio::test]
    async fn web_search_adapter_health_when_disconnected() {
        let adapter = WebSearchAdapter::new("ws-test");
        let status = adapter.health_check().await.unwrap();
        assert_eq!(status, HealthStatus::Unhealthy);
    }

    #[tokio::test]
    async fn web_search_adapter_rejects_when_not_connected() {
        let adapter = WebSearchAdapter::new("ws-test");
        let result = adapter
            .execute_tool("web_search", json!({"query": "test"}))
            .await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn web_search_adapter_rejects_unknown_tool() {
        let mut adapter = WebSearchAdapter::new("ws-test");
        adapter.connect().await.unwrap();
        let result = adapter.execute_tool("nonexistent", json!({})).await;
        assert!(result.is_err());
    }

    #[test]
    fn strip_html_tags_removes_tags() {
        assert_eq!(strip_html_tags("<b>hello</b> world"), "hello world");
        assert_eq!(strip_html_tags("<a href=\"x\">link</a>"), "link");
        assert_eq!(strip_html_tags("no tags here"), "no tags here");
        assert_eq!(strip_html_tags(""), "");
    }

    #[test]
    fn strip_html_tags_decodes_entities() {
        assert_eq!(strip_html_tags("a &amp; b"), "a & b");
        assert_eq!(strip_html_tags("&lt;tag&gt;"), "<tag>");
        assert_eq!(strip_html_tags("&quot;quoted&quot;"), "\"quoted\"");
    }

    #[test]
    fn parse_duckduckgo_results_extracts_data() {
        let html = r#"
        <div class="result">
            <a rel="nofollow" href="https://example.com" class="result__a">Example Title</a>
            <span class="result__snippet">This is a snippet about Example.</span>
        </div>
        <div class="result">
            <a rel="nofollow" href="https://other.com" class="result__a">Other Result</a>
            <span class="result__snippet">Another snippet here.</span>
        </div>
        "#;

        let results = parse_duckduckgo_results(html, 10);
        assert_eq!(results.len(), 2);
        assert_eq!(results[0]["title"], "Example Title");
        assert_eq!(results[0]["url"], "https://example.com");
        assert_eq!(results[0]["snippet"], "This is a snippet about Example.");
        assert_eq!(results[1]["title"], "Other Result");
        assert_eq!(results[1]["url"], "https://other.com");
    }

    #[test]
    fn parse_duckduckgo_results_respects_max_results() {
        let html = r#"
        <a href="https://a.com" class="result__a">A</a>
        <span class="result__snippet">Snippet A</span>
        <a href="https://b.com" class="result__a">B</a>
        <span class="result__snippet">Snippet B</span>
        <a href="https://c.com" class="result__a">C</a>
        <span class="result__snippet">Snippet C</span>
        "#;
        let results = parse_duckduckgo_results(html, 2);
        assert_eq!(results.len(), 2);
    }

    #[test]
    fn parse_duckduckgo_results_handles_empty_html() {
        let results = parse_duckduckgo_results("", 5);
        assert!(results.is_empty());
    }

    #[test]
    fn extract_href_before_finds_url() {
        let html = r#"<a rel="nofollow" href="https://example.com" class="result__a""#;
        let marker = "class=\"result__a\"";
        let before = &html[..html.find(marker).unwrap()];
        let url = extract_href_before(before);
        assert_eq!(url, Some("https://example.com".to_string()));
    }

    #[test]
    fn clean_ddg_url_extracts_actual_url() {
        let ddg = "//duckduckgo.com/l/?uddg=https%3A%2F%2Fexample.com%2Fpage&rut=abc";
        assert_eq!(clean_ddg_url(ddg), "https://example.com/page");
    }

    #[test]
    fn clean_ddg_url_passes_through_normal_urls() {
        assert_eq!(clean_ddg_url("https://example.com"), "https://example.com");
    }

    #[test]
    fn clean_ddg_url_adds_protocol() {
        assert_eq!(clean_ddg_url("//example.com/p"), "https://example.com/p");
    }

    #[test]
    fn url_decode_handles_percent_encoding() {
        assert_eq!(url_decode("hello%20world"), "hello world");
        assert_eq!(url_decode("https%3A%2F%2Fexample.com"), "https://example.com");
        assert_eq!(url_decode("a+b"), "a b");
    }
}
