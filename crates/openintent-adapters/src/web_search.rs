//! Web search adapter -- multi-engine web search with caching and fallback.
//!
//! Tools:
//!   - `web_search` -- search only, returns titles/URLs/snippets
//!   - `web_research` -- search + auto-fetch top results with content extraction
//!
//! Search priority:
//!   1. Brave Search API (if `BRAVE_API_KEY` is set) -- best structured results
//!   2. Perplexity Sonar (if `PERPLEXITY_API_KEY` is set) -- AI-synthesized
//!   3. DuckDuckGo HTML scraping (no key needed) -- universal fallback
//!
//! Features:
//!   - In-memory LRU cache (15 min TTL, 100 entries) via moka
//!   - `web_research` automatically fetches and extracts content from top URLs
//!   - Real browser User-Agent to avoid blocking
//!   - DDG tracking URL cleaning and percent-decoding
//!   - Automatic engine fallback on failure

use std::time::Duration;

use async_trait::async_trait;
use moka::future::Cache;
use serde_json::{Value, json};
use tracing::{debug, info, warn};

use crate::web_fetch;

use crate::error::{AdapterError, Result};
use crate::traits::{Adapter, AdapterType, AuthRequirement, HealthStatus, ToolDefinition};

// ═══════════════════════════════════════════════════════════════════════
//  Constants
// ═══════════════════════════════════════════════════════════════════════

const DEFAULT_MAX_RESULTS: usize = 10;
const DUCKDUCKGO_HTML_URL: &str = "https://html.duckduckgo.com/html/";
const BRAVE_SEARCH_URL: &str = "https://api.search.brave.com/res/v1/web/search";
const PERPLEXITY_API_URL: &str = "https://api.perplexity.ai/chat/completions";

/// Cache TTL in minutes.
const CACHE_TTL_MINUTES: u64 = 15;
/// Maximum cached entries.
const CACHE_MAX_ENTRIES: u64 = 100;

const BROWSER_USER_AGENT: &str =
    "Mozilla/5.0 (Macintosh; Intel Mac OS X 10_15_7) \
     AppleWebKit/537.36 (KHTML, like Gecko) Chrome/131.0.0.0 Safari/537.36";

// ═══════════════════════════════════════════════════════════════════════
//  Adapter
// ═══════════════════════════════════════════════════════════════════════

/// Web search service adapter with multi-engine support and caching.
pub struct WebSearchAdapter {
    id: String,
    connected: bool,
    client: reqwest::Client,
    brave_api_key: Option<String>,
    perplexity_api_key: Option<String>,
    /// In-memory LRU cache for search results.
    cache: Cache<String, Value>,
}

impl WebSearchAdapter {
    /// Create a new web search adapter.
    pub fn new(id: impl Into<String>) -> Self {
        let client = reqwest::Client::builder()
            .user_agent(BROWSER_USER_AGENT)
            .timeout(Duration::from_secs(30))
            .build()
            .unwrap_or_default();

        let brave_api_key = env_non_empty("BRAVE_API_KEY");
        let perplexity_api_key =
            env_non_empty("PERPLEXITY_API_KEY").or_else(|| env_non_empty("OPENROUTER_API_KEY"));

        let cache = Cache::builder()
            .max_capacity(CACHE_MAX_ENTRIES)
            .time_to_live(Duration::from_secs(CACHE_TTL_MINUTES * 60))
            .build();

        Self {
            id: id.into(),
            connected: false,
            client,
            brave_api_key,
            perplexity_api_key,
            cache,
        }
    }

    /// Execute a web search with cache-first strategy.
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

        // Check cache first.
        let cache_key = format!("{}:{}", query.to_lowercase().trim(), max_results);
        if let Some(cached) = self.cache.get(&cache_key).await {
            debug!(query, "returning cached search results");
            return Ok(cached);
        }

        debug!(query, max_results, "performing web search");

        // Try engines in priority order.
        let result = self.search_with_fallback(query, max_results).await?;

        // Store in cache.
        self.cache.insert(cache_key, result.clone()).await;

        Ok(result)
    }

    /// Try engines in priority order with automatic fallback.
    async fn search_with_fallback(&self, query: &str, max_results: usize) -> Result<Value> {
        // 1. Brave Search API (best structured results).
        if let Some(ref api_key) = self.brave_api_key {
            match self.search_brave(query, max_results, api_key).await {
                Ok(results) if !results.is_empty() => {
                    debug!(count = results.len(), engine = "brave", "search completed");
                    return Ok(json!({ "engine": "brave", "results": results }));
                }
                Ok(_) => debug!("Brave returned no results, trying next engine"),
                Err(e) => warn!(error = %e, "Brave Search failed, trying next engine"),
            }
        }

        // 2. Perplexity Sonar (AI-synthesized answer with citations).
        if let Some(ref api_key) = self.perplexity_api_key {
            match self.search_perplexity(query, api_key).await {
                Ok(result) => {
                    debug!(engine = "perplexity", "search completed");
                    return Ok(result);
                }
                Err(e) => warn!(error = %e, "Perplexity failed, falling back to DuckDuckGo"),
            }
        }

        // 3. DuckDuckGo HTML scraping (universal fallback, no key needed).
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
    //  Perplexity Sonar (AI search with citations)
    // ───────────────────────────────────────────────────────────────────

    async fn search_perplexity(&self, query: &str, api_key: &str) -> Result<Value> {
        let body = json!({
            "model": "sonar",
            "messages": [{"role": "user", "content": query}],
            "return_citations": true,
        });

        let response = self
            .client
            .post(PERPLEXITY_API_URL)
            .header("Authorization", format!("Bearer {api_key}"))
            .header("Content-Type", "application/json")
            .json(&body)
            .send()
            .await
            .map_err(|e| AdapterError::ExecutionFailed {
                tool_name: "web_search".into(),
                reason: format!("Perplexity request failed: {e}"),
            })?;

        if !response.status().is_success() {
            return Err(AdapterError::ExecutionFailed {
                tool_name: "web_search".into(),
                reason: format!("Perplexity returned status {}", response.status()),
            });
        }

        let data: Value = response.json().await.map_err(|e| {
            AdapterError::ExecutionFailed {
                tool_name: "web_search".into(),
                reason: format!("failed to parse Perplexity response: {e}"),
            }
        })?;

        // Extract the synthesized answer.
        let answer = data
            .pointer("/choices/0/message/content")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_string();

        // Extract citations as structured results.
        let mut results = Vec::new();
        if let Some(citations) = data.get("citations").and_then(|v| v.as_array()) {
            for citation in citations {
                if let Some(url) = citation.as_str() {
                    results.push(json!({
                        "title": "",
                        "url": url,
                        "snippet": "",
                    }));
                }
            }
        }

        Ok(json!({
            "engine": "perplexity",
            "answer": answer,
            "results": results,
        }))
    }

    // ───────────────────────────────────────────────────────────────────
    //  DuckDuckGo HTML scraping
    // ───────────────────────────────────────────────────────────────────

    async fn search_duckduckgo(&self, query: &str, max_results: usize) -> Result<Vec<Value>> {
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

    // ───────────────────────────────────────────────────────────────────
    //  web_research: compound search + fetch + extract
    // ───────────────────────────────────────────────────────────────────

    /// Deep research: search, then automatically fetch and extract content
    /// from the top result pages. Returns both search results and page content.
    async fn tool_web_research(&self, params: Value) -> Result<Value> {
        let query = params
            .get("query")
            .and_then(|v| v.as_str())
            .ok_or_else(|| AdapterError::InvalidParams {
                tool_name: "web_research".into(),
                reason: "missing required string field `query`".into(),
            })?;

        let max_pages = params
            .get("max_pages")
            .and_then(|v| v.as_u64())
            .map(|v| v as usize)
            .unwrap_or(5)
            .min(8);

        let max_content_per_page = params
            .get("max_content_per_page")
            .and_then(|v| v.as_u64())
            .map(|v| v as usize)
            .unwrap_or(15_000);

        debug!(query, max_pages, "starting deep research");

        // Check cache.
        let cache_key = format!("research:{}:{}", query.to_lowercase().trim(), max_pages);
        if let Some(cached) = self.cache.get(&cache_key).await {
            debug!(query, "returning cached research results");
            return Ok(cached);
        }

        // Step 1: Search for results.
        let search_result = self.search_with_fallback(query, 10).await?;
        let search_results = search_result
            .get("results")
            .and_then(|v| v.as_array())
            .cloned()
            .unwrap_or_default();

        // Step 2: Fetch top N URLs and extract content.
        let mut fetched_pages = Vec::new();
        for result in search_results.iter().take(max_pages) {
            let url = match result.get("url").and_then(|v| v.as_str()) {
                Some(u) if !u.is_empty() => u,
                _ => continue,
            };
            let title = result
                .get("title")
                .and_then(|v| v.as_str())
                .unwrap_or("");

            match self.fetch_and_extract(url, max_content_per_page).await {
                Ok(content) => {
                    fetched_pages.push(json!({
                        "url": url,
                        "title": title,
                        "content": content,
                    }));
                }
                Err(e) => {
                    debug!(url, error = %e, "failed to fetch page during research");
                }
            }
        }

        info!(
            query,
            search_count = search_results.len(),
            fetched_count = fetched_pages.len(),
            "deep research completed"
        );

        let result = json!({
            "query": query,
            "search_results": search_results,
            "fetched_pages": fetched_pages,
            "pages_fetched": fetched_pages.len(),
        });

        // Cache the research result.
        self.cache.insert(cache_key, result.clone()).await;

        Ok(result)
    }

    /// Fetch a single URL and extract its content for research.
    async fn fetch_and_extract(
        &self,
        url_str: &str,
        max_content: usize,
    ) -> Result<String> {
        let response = self
            .client
            .get(url_str)
            .header(
                "Accept",
                "text/markdown, text/html;q=0.9, */*;q=0.1",
            )
            .header("Accept-Language", "en-US,en;q=0.9,zh-CN;q=0.8,zh;q=0.7")
            .timeout(Duration::from_secs(15))
            .send()
            .await
            .map_err(|e| AdapterError::ExecutionFailed {
                tool_name: "web_research".into(),
                reason: format!("fetch failed: {e}"),
            })?;

        if !response.status().is_success() {
            return Err(AdapterError::ExecutionFailed {
                tool_name: "web_research".into(),
                reason: format!("status {}", response.status()),
            });
        }

        let content_type = response
            .headers()
            .get(reqwest::header::CONTENT_TYPE)
            .and_then(|v| v.to_str().ok())
            .unwrap_or("text/plain")
            .to_string();

        // Skip binary/non-text content types entirely.
        if is_binary_content_type(&content_type) {
            return Err(AdapterError::ExecutionFailed {
                tool_name: "web_research".into(),
                reason: format!("skipping binary content type: {content_type}"),
            });
        }

        let body = response.text().await.map_err(|e| {
            AdapterError::ExecutionFailed {
                tool_name: "web_research".into(),
                reason: format!("body read failed: {e}"),
            }
        })?;

        // Skip content that looks like binary/compressed data (gzip magic, PDF, etc.).
        if looks_like_binary(&body) {
            return Err(AdapterError::ExecutionFailed {
                tool_name: "web_research".into(),
                reason: "skipping binary/compressed content".into(),
            });
        }

        let content = if content_type.contains("text/html") {
            let (text, _extractor) = web_fetch::extract_content_from_html(&body, url_str);
            text
        } else {
            body
        };

        let (truncated, _) = web_fetch::smart_truncate(&content, max_content);
        Ok(truncated)
    }
}

/// Read a non-empty environment variable.
fn env_non_empty(name: &str) -> Option<String> {
    std::env::var(name).ok().filter(|v| !v.is_empty())
}

/// Check if a Content-Type header indicates binary/non-text content.
fn is_binary_content_type(ct: &str) -> bool {
    let ct_lower = ct.to_ascii_lowercase();
    ct_lower.starts_with("image/")
        || ct_lower.starts_with("audio/")
        || ct_lower.starts_with("video/")
        || ct_lower.contains("octet-stream")
        || ct_lower.contains("application/pdf")
        || ct_lower.contains("application/zip")
        || ct_lower.contains("application/gzip")
        || ct_lower.contains("application/x-gzip")
}

/// Heuristic: check if the body starts with known binary signatures.
fn looks_like_binary(body: &str) -> bool {
    if body.len() < 4 {
        return false;
    }
    let bytes = body.as_bytes();
    // Gzip magic: 0x1f 0x8b
    if bytes[0] == 0x1f && bytes[1] == 0x8b {
        return true;
    }
    // PDF magic: %PDF
    if bytes.starts_with(b"%PDF") {
        return true;
    }
    // PK (ZIP/DOCX/XLSX): 0x50 0x4b 0x03 0x04
    if bytes.starts_with(&[0x50, 0x4b, 0x03, 0x04]) {
        return true;
    }
    // Check for high ratio of non-printable characters in first 512 bytes.
    // Use byte-level checks to avoid char-boundary issues.
    let check_len = body.len().min(512);
    let non_text = bytes[..check_len]
        .iter()
        .filter(|&&b| {
            // Non-printable ASCII (excluding whitespace/newline) and high bytes
            // that aren't part of valid UTF-8 multi-byte sequences.
            b < 0x20 && b != b'\n' && b != b'\r' && b != b'\t'
        })
        .count();
    // If more than 10% of the first 512 bytes are control chars, treat as binary.
    non_text * 10 > check_len
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
        let engines: Vec<&str> = [
            self.brave_api_key.as_ref().map(|_| "brave"),
            self.perplexity_api_key.as_ref().map(|_| "perplexity"),
            Some("duckduckgo"),
        ]
        .into_iter()
        .flatten()
        .collect();
        let engine_str = engines.join("+");
        info!(id = %self.id, engines = %engine_str, "web search adapter connected");
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
        vec![
            ToolDefinition {
                name: "web_search".into(),
                description: "Search the web and return titles, URLs, and snippets. \
                              Uses Brave Search, Perplexity Sonar, or DuckDuckGo \
                              depending on available API keys. Results are cached \
                              for 15 minutes. Returns up to 10 results by default."
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
            },
            ToolDefinition {
                name: "web_research".into(),
                description: "Deep web research: searches the web, then automatically \
                              fetches and reads the top result pages to extract their \
                              full content. Use this instead of web_search when you need \
                              comprehensive information, detailed analysis, or when search \
                              snippets alone are insufficient. Returns both search results \
                              and extracted page content."
                    .into(),
                parameters: json!({
                    "type": "object",
                    "properties": {
                        "query": {
                            "type": "string",
                            "description": "The research query"
                        },
                        "max_pages": {
                            "type": "integer",
                            "description": "Maximum pages to fetch and read (default: 5, max: 8)"
                        },
                        "max_content_per_page": {
                            "type": "integer",
                            "description": "Maximum characters per page (default: 15000)"
                        }
                    },
                    "required": ["query"]
                }),
            },
        ]
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
            "web_research" => self.tool_web_research(params).await,
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
        assert_eq!(tools.len(), 2);
        assert_eq!(tools[0].name, "web_search");
        assert_eq!(tools[1].name, "web_research");
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

    #[tokio::test]
    async fn web_search_cache_returns_same_result() {
        let adapter = WebSearchAdapter::new("ws-test");
        let key = "test:10".to_string();
        let val = json!({"engine": "test", "results": []});
        adapter.cache.insert(key.clone(), val.clone()).await;
        let cached = adapter.cache.get(&key).await;
        assert_eq!(cached, Some(val));
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
