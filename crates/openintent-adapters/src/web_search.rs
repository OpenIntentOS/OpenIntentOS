//! Web search adapter -- search the web via DuckDuckGo HTML interface.
//!
//! This adapter performs web searches using the DuckDuckGo HTML endpoint,
//! which requires no API key.  Results are parsed from the HTML response
//! using simple string matching to extract titles, URLs, and snippets.

use async_trait::async_trait;
use serde_json::{Value, json};
use tracing::{debug, info, warn};

use crate::error::{AdapterError, Result};
use crate::traits::{Adapter, AdapterType, AuthRequirement, HealthStatus, ToolDefinition};

/// Default maximum number of search results to return.
const DEFAULT_MAX_RESULTS: usize = 5;

/// DuckDuckGo HTML search endpoint.
const DUCKDUCKGO_HTML_URL: &str = "https://html.duckduckgo.com/html/";

/// Web search service adapter.
pub struct WebSearchAdapter {
    /// Unique identifier for this adapter instance.
    id: String,
    /// Whether the adapter has been connected.
    connected: bool,
    /// HTTP client for making requests.
    client: reqwest::Client,
}

impl WebSearchAdapter {
    /// Create a new web search adapter.
    pub fn new(id: impl Into<String>) -> Self {
        let client = reqwest::Client::builder()
            .user_agent("OpenIntentOS/0.1")
            .timeout(std::time::Duration::from_secs(30))
            .build()
            .unwrap_or_default();

        Self {
            id: id.into(),
            connected: false,
            client,
        }
    }

    /// Execute a web search and return structured results.
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

        debug!(
            query = query,
            max_results = max_results,
            "performing web search"
        );

        let response = self
            .client
            .get(DUCKDUCKGO_HTML_URL)
            .query(&[("q", query)])
            .send()
            .await
            .map_err(|e| AdapterError::ExecutionFailed {
                tool_name: "web_search".into(),
                reason: format!("HTTP request failed: {e}"),
            })?;

        if !response.status().is_success() {
            return Err(AdapterError::ExecutionFailed {
                tool_name: "web_search".into(),
                reason: format!("DuckDuckGo returned status {}", response.status()),
            });
        }

        let html = response
            .text()
            .await
            .map_err(|e| AdapterError::ExecutionFailed {
                tool_name: "web_search".into(),
                reason: format!("failed to read response body: {e}"),
            })?;

        let results = parse_duckduckgo_results(&html, max_results);

        debug!(result_count = results.len(), "search completed");

        Ok(json!({
            "results": results,
        }))
    }
}

/// Parse DuckDuckGo HTML search results.
///
/// Extracts titles, URLs, and snippets from the HTML response by looking
/// for elements with class `result__a` (title/link) and `result__snippet`
/// (description text).
fn parse_duckduckgo_results(html: &str, max_results: usize) -> Vec<Value> {
    let mut results = Vec::new();

    // DuckDuckGo HTML results contain links with class="result__a"
    // and snippets with class="result__snippet".
    // We iterate through result__a occurrences and pair them with snippets.

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

        // Extract the URL from the href attribute before the title marker.
        // The pattern is: <a ... href="URL" ... class="result__a">Title</a>
        let before_marker = &html[..title_pos];
        let url = extract_href_before(before_marker).unwrap_or_default();

        // Extract the title text after the marker.
        // Find the closing > of the opening tag, then extract text until </a>.
        let after_marker = &html[title_pos + title_marker.len()..];
        let title = extract_tag_text(after_marker, "</a>");

        // Extract the snippet from the corresponding snippet position.
        let snippet = if i < snippet_positions.len() {
            let after_snippet = &html[snippet_positions[i] + snippet_marker.len()..];
            let raw = extract_tag_text(after_snippet, "</");
            strip_html_tags(&raw)
        } else {
            String::new()
        };

        if !title.is_empty() || !url.is_empty() {
            results.push(json!({
                "title": strip_html_tags(&title),
                "url": url,
                "snippet": snippet.trim(),
            }));
        }
    }

    results
}

/// Extract the href value from the last `href="..."` before the given position.
fn extract_href_before(html_before: &str) -> Option<String> {
    let href_marker = "href=\"";
    let last_href = html_before.rfind(href_marker)?;
    let start = last_href + href_marker.len();
    let remaining = &html_before[start..];
    let end = remaining.find('"')?;
    Some(remaining[..end].to_string())
}

/// Extract text content after finding the closing `>` of the current tag,
/// up to the specified end marker.
fn extract_tag_text(html_after_marker: &str, end_marker: &str) -> String {
    // Find the closing > of the opening tag.
    let closing_bracket = match html_after_marker.find('>') {
        Some(pos) => pos,
        None => return String::new(),
    };

    let content = &html_after_marker[closing_bracket + 1..];

    // Find the end marker.
    let end = match content.find(end_marker) {
        Some(pos) => pos,
        None => content.len(),
    };

    content[..end].to_string()
}

/// Strip HTML tags from a string, removing `<...>` sequences and decoding
/// common HTML entities.
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
impl Adapter for WebSearchAdapter {
    fn id(&self) -> &str {
        &self.id
    }

    fn adapter_type(&self) -> AdapterType {
        AdapterType::Productivity
    }

    async fn connect(&mut self) -> Result<()> {
        info!(id = %self.id, "web search adapter connected");
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
        // Verify we can reach DuckDuckGo.
        match self.client.head(DUCKDUCKGO_HTML_URL).send().await {
            Ok(resp) if resp.status().is_success() || resp.status().is_redirection() => {
                Ok(HealthStatus::Healthy)
            }
            Ok(resp) => {
                warn!(status = %resp.status(), "DuckDuckGo health check returned non-success");
                Ok(HealthStatus::Degraded)
            }
            Err(e) => {
                warn!(error = %e, "DuckDuckGo health check failed");
                Ok(HealthStatus::Unhealthy)
            }
        }
    }

    fn tools(&self) -> Vec<ToolDefinition> {
        vec![ToolDefinition {
            name: "web_search".into(),
            description: "Search the web using DuckDuckGo and return titles, URLs, and snippets"
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
                        "description": "Maximum number of results to return (default: 5)"
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

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

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
            .execute_tool("web_search", json!({"query": "rust lang"}))
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
}
