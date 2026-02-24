//! Browser adapter -- control a Chromium-based browser via the Chrome DevTools Protocol.
//!
//! This adapter communicates with Chrome/Chromium over its remote debugging port
//! using the CDP (Chrome DevTools Protocol) over WebSocket.  It provides tools for
//! navigation, content extraction, screenshots, element interaction, and JavaScript
//! evaluation.
//!
//! # Architecture
//!
//! 1. Connect to Chrome's HTTP endpoint at `http://localhost:{port}/json/version`
//!    to verify the browser is reachable.
//! 2. For each tool execution, discover page targets via `GET /json`, connect to
//!    the first page target's WebSocket URL, send a CDP command, receive the
//!    response, and close the connection.
//!
//! The adapter can optionally launch Chrome with `--remote-debugging-port` if it
//! is not already running.

use async_trait::async_trait;

use futures::{SinkExt, StreamExt};
use serde_json::{Value, json};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::time::Duration;
use tokio_tungstenite::connect_async;
use tokio_tungstenite::tungstenite::Message;
use tracing::{debug, info, warn};

use crate::error::{AdapterError, Result};
use crate::traits::{Adapter, AdapterType, AuthRequirement, HealthStatus, ToolDefinition};

/// Default Chrome DevTools Protocol debug port.
const DEFAULT_DEBUG_PORT: u16 = 9222;

/// Timeout for CDP WebSocket operations in seconds.
const CDP_TIMEOUT_SECS: u64 = 30;

/// Timeout for HTTP requests to the DevTools endpoint in seconds.
const HTTP_TIMEOUT_SECS: u64 = 10;

/// Timeout waiting for Chrome to start up in seconds.
const CHROME_STARTUP_TIMEOUT_SECS: u64 = 10;

/// Maximum response body size from CDP in bytes (5 MB).
const MAX_CDP_RESPONSE_BYTES: usize = 5 * 1024 * 1024;

/// Browser service adapter using Chrome DevTools Protocol.
pub struct BrowserAdapter {
    /// Unique identifier for this adapter instance.
    id: String,
    /// Whether the adapter has been connected.
    connected: AtomicBool,
    /// Optional path to the Chrome/Chromium executable.
    chrome_path: Option<String>,
    /// The remote debugging port.
    debug_port: u16,
    /// Monotonically increasing CDP message ID.
    next_message_id: AtomicU64,
    /// HTTP client for DevTools REST endpoints.
    client: reqwest::Client,
}

// Explicit Send + Sync: all fields are atomic or Send+Sync.
// AtomicBool and AtomicU64 are Send + Sync, reqwest::Client is Send + Sync.
unsafe impl Send for BrowserAdapter {}
unsafe impl Sync for BrowserAdapter {}

impl BrowserAdapter {
    /// Create a new browser adapter with the default debug port (9222).
    pub fn new(id: impl Into<String>) -> Self {
        Self::with_port(id, DEFAULT_DEBUG_PORT)
    }

    /// Create a new browser adapter with a custom debug port.
    pub fn with_port(id: impl Into<String>, port: u16) -> Self {
        let client = reqwest::Client::builder()
            .timeout(Duration::from_secs(HTTP_TIMEOUT_SECS))
            .user_agent("OpenIntentOS/0.1")
            .build()
            .unwrap_or_default();

        Self {
            id: id.into(),
            connected: AtomicBool::new(false),
            chrome_path: None,
            debug_port: port,
            next_message_id: AtomicU64::new(1),
            client,
        }
    }

    /// Set a custom Chrome/Chromium executable path.
    pub fn with_chrome_path(mut self, path: impl Into<String>) -> Self {
        self.chrome_path = Some(path.into());
        self
    }

    /// Return the base URL for the DevTools HTTP endpoint.
    fn devtools_base_url(&self) -> String {
        format!("http://localhost:{}", self.debug_port)
    }

    /// Allocate the next CDP message ID.
    fn next_id(&self) -> u64 {
        self.next_message_id.fetch_add(1, Ordering::Relaxed)
    }

    /// Check if the DevTools endpoint is reachable.
    async fn is_devtools_reachable(&self) -> bool {
        let url = format!("{}/json/version", self.devtools_base_url());
        self.client.get(&url).send().await.is_ok()
    }

    /// Attempt to launch Chrome with remote debugging enabled.
    async fn try_launch_chrome(&self) -> Result<()> {
        let chrome_path = self.find_chrome_path()?;

        info!(
            chrome_path = %chrome_path,
            port = self.debug_port,
            "launching Chrome with remote debugging"
        );

        let mut cmd = tokio::process::Command::new(&chrome_path);
        cmd.arg(format!("--remote-debugging-port={}", self.debug_port))
            .arg("--no-first-run")
            .arg("--no-default-browser-check")
            .arg("--headless=new")
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null());

        cmd.spawn().map_err(|e| AdapterError::ExecutionFailed {
            tool_name: "connect".into(),
            reason: format!("failed to launch Chrome at `{chrome_path}`: {e}"),
        })?;

        // Wait for Chrome to become reachable.
        let deadline =
            tokio::time::Instant::now() + Duration::from_secs(CHROME_STARTUP_TIMEOUT_SECS);
        loop {
            if self.is_devtools_reachable().await {
                info!("Chrome DevTools endpoint is reachable");
                return Ok(());
            }
            if tokio::time::Instant::now() >= deadline {
                return Err(AdapterError::Timeout {
                    seconds: CHROME_STARTUP_TIMEOUT_SECS,
                    reason: "Chrome did not start in time".into(),
                });
            }
            tokio::time::sleep(Duration::from_millis(250)).await;
        }
    }

    /// Find the Chrome/Chromium executable path.
    fn find_chrome_path(&self) -> Result<String> {
        if let Some(ref path) = self.chrome_path {
            return Ok(path.clone());
        }

        // Platform-specific default paths.
        let candidates = if cfg!(target_os = "macos") {
            vec![
                "/Applications/Google Chrome.app/Contents/MacOS/Google Chrome",
                "/Applications/Chromium.app/Contents/MacOS/Chromium",
                "/Applications/Microsoft Edge.app/Contents/MacOS/Microsoft Edge",
            ]
        } else if cfg!(target_os = "linux") {
            vec![
                "google-chrome",
                "google-chrome-stable",
                "chromium",
                "chromium-browser",
            ]
        } else {
            vec![]
        };

        for candidate in &candidates {
            let path = std::path::Path::new(candidate);
            if path.exists() || which_exists(candidate) {
                return Ok((*candidate).to_string());
            }
        }

        Err(AdapterError::ExecutionFailed {
            tool_name: "connect".into(),
            reason: "could not find Chrome/Chromium executable; set chrome_path manually".into(),
        })
    }

    /// Get the list of page targets from the DevTools endpoint.
    async fn get_page_targets(&self) -> Result<Vec<Value>> {
        let url = format!("{}/json", self.devtools_base_url());
        let response =
            self.client
                .get(&url)
                .send()
                .await
                .map_err(|e| AdapterError::ExecutionFailed {
                    tool_name: "browser".into(),
                    reason: format!("failed to list DevTools targets: {e}"),
                })?;

        let targets: Vec<Value> =
            response
                .json()
                .await
                .map_err(|e| AdapterError::ExecutionFailed {
                    tool_name: "browser".into(),
                    reason: format!("failed to parse target list: {e}"),
                })?;

        // Filter to page targets only.
        let pages: Vec<Value> = targets
            .into_iter()
            .filter(|t| t.get("type").and_then(|v| v.as_str()) == Some("page"))
            .collect();

        if pages.is_empty() {
            return Err(AdapterError::ExecutionFailed {
                tool_name: "browser".into(),
                reason: "no page targets available in Chrome".into(),
            });
        }

        Ok(pages)
    }

    /// Get the WebSocket debugger URL for the first page target.
    async fn get_ws_url(&self) -> Result<String> {
        let pages = self.get_page_targets().await?;
        let first_page = &pages[0];

        first_page
            .get("webSocketDebuggerUrl")
            .and_then(|v| v.as_str())
            .map(|s| s.to_string())
            .ok_or_else(|| AdapterError::ExecutionFailed {
                tool_name: "browser".into(),
                reason: "page target has no webSocketDebuggerUrl".into(),
            })
    }

    /// Send a CDP command over WebSocket and return the result.
    ///
    /// Opens a new WebSocket connection, sends the command, waits for the
    /// matching response (by message ID), and closes the connection.
    async fn send_cdp_command(&self, method: &str, params: Value) -> Result<Value> {
        let ws_url = self.get_ws_url().await?;
        let msg_id = self.next_id();

        debug!(
            method = method,
            msg_id = msg_id,
            ws_url = %ws_url,
            "sending CDP command"
        );

        let cdp_message = json!({
            "id": msg_id,
            "method": method,
            "params": params,
        });

        // Connect to the WebSocket with a timeout.
        let (ws_stream, _response) = tokio::time::timeout(
            Duration::from_secs(CDP_TIMEOUT_SECS),
            connect_async(&ws_url),
        )
        .await
        .map_err(|_| AdapterError::Timeout {
            seconds: CDP_TIMEOUT_SECS,
            reason: format!("WebSocket connection to `{ws_url}` timed out"),
        })?
        .map_err(|e| AdapterError::ExecutionFailed {
            tool_name: "browser".into(),
            reason: format!("WebSocket connection failed: {e}"),
        })?;

        let (mut sink, mut stream) = ws_stream.split();

        // Send the CDP command.
        let msg_text = serde_json::to_string(&cdp_message).map_err(AdapterError::from)?;
        sink.send(Message::Text(msg_text.into()))
            .await
            .map_err(|e| AdapterError::ExecutionFailed {
                tool_name: "browser".into(),
                reason: format!("failed to send CDP message: {e}"),
            })?;

        // Wait for the matching response.
        let result = tokio::time::timeout(Duration::from_secs(CDP_TIMEOUT_SECS), async {
            while let Some(msg_result) = stream.next().await {
                let msg = msg_result.map_err(|e| AdapterError::ExecutionFailed {
                    tool_name: "browser".into(),
                    reason: format!("WebSocket receive error: {e}"),
                })?;

                match msg {
                    Message::Text(text) => {
                        if text.len() > MAX_CDP_RESPONSE_BYTES {
                            return Err(AdapterError::ExecutionFailed {
                                tool_name: "browser".into(),
                                reason: format!(
                                    "CDP response too large: {} bytes (max {})",
                                    text.len(),
                                    MAX_CDP_RESPONSE_BYTES
                                ),
                            });
                        }

                        let response: Value =
                            serde_json::from_str(&text).map_err(AdapterError::from)?;

                        // Check if this response matches our message ID.
                        if response.get("id").and_then(|v| v.as_u64()) == Some(msg_id) {
                            // Check for CDP errors.
                            if let Some(error) = response.get("error") {
                                let error_msg = error
                                    .get("message")
                                    .and_then(|v| v.as_str())
                                    .unwrap_or("unknown CDP error");
                                return Err(AdapterError::ExecutionFailed {
                                    tool_name: "browser".into(),
                                    reason: format!("CDP error: {error_msg}"),
                                });
                            }

                            return Ok(response.get("result").cloned().unwrap_or(json!({})));
                        }
                        // Not our message; continue reading.
                    }
                    Message::Close(_) => {
                        return Err(AdapterError::ExecutionFailed {
                            tool_name: "browser".into(),
                            reason: "WebSocket closed before receiving CDP response".into(),
                        });
                    }
                    // Ignore ping, pong, binary frames.
                    _ => {}
                }
            }

            Err(AdapterError::ExecutionFailed {
                tool_name: "browser".into(),
                reason: "WebSocket stream ended without CDP response".into(),
            })
        })
        .await
        .map_err(|_| AdapterError::Timeout {
            seconds: CDP_TIMEOUT_SECS,
            reason: format!("waiting for CDP response to `{method}`"),
        })?;

        // Attempt to close the WebSocket cleanly (best-effort).
        let _ = sink.send(Message::Close(None)).await;

        result
    }

    // -----------------------------------------------------------------------
    // Tool implementations
    // -----------------------------------------------------------------------

    /// Navigate the browser to a URL.
    async fn tool_browser_navigate(&self, params: Value) -> Result<Value> {
        let url_str = params.get("url").and_then(|v| v.as_str()).ok_or_else(|| {
            AdapterError::InvalidParams {
                tool_name: "browser_navigate".into(),
                reason: "missing required string field `url`".into(),
            }
        })?;

        // Validate URL.
        let _parsed = url::Url::parse(url_str).map_err(|e| AdapterError::InvalidParams {
            tool_name: "browser_navigate".into(),
            reason: format!("invalid URL `{url_str}`: {e}"),
        })?;

        debug!(url = url_str, "navigating browser");

        let result = self
            .send_cdp_command("Page.navigate", json!({ "url": url_str }))
            .await?;

        // Extract frame ID and loader ID from response.
        let frame_id = result
            .get("frameId")
            .and_then(|v| v.as_str())
            .unwrap_or("unknown");

        info!(url = url_str, frame_id = frame_id, "navigation complete");

        Ok(json!({
            "success": true,
            "url": url_str,
            "frame_id": frame_id,
        }))
    }

    /// Get the current page's text content.
    async fn tool_browser_get_page_content(&self, _params: Value) -> Result<Value> {
        debug!("getting page content");

        let expression = "document.body.innerText";
        let result = self
            .send_cdp_command(
                "Runtime.evaluate",
                json!({
                    "expression": expression,
                    "returnByValue": true,
                }),
            )
            .await?;

        let content = extract_runtime_value(&result)?;

        Ok(json!({
            "content": content,
            "length": content.len(),
        }))
    }

    /// Take a screenshot of the current page.
    async fn tool_browser_screenshot(&self, params: Value) -> Result<Value> {
        let format = params
            .get("format")
            .and_then(|v| v.as_str())
            .unwrap_or("png");

        // Validate format.
        if format != "png" && format != "jpeg" {
            return Err(AdapterError::InvalidParams {
                tool_name: "browser_screenshot".into(),
                reason: format!("unsupported format `{format}`; use \"png\" or \"jpeg\""),
            });
        }

        debug!(format = format, "taking screenshot");

        let result = self
            .send_cdp_command("Page.captureScreenshot", json!({ "format": format }))
            .await?;

        let data = result.get("data").and_then(|v| v.as_str()).unwrap_or("");

        Ok(json!({
            "format": format,
            "data": data,
            "encoding": "base64",
            "length": data.len(),
        }))
    }

    /// Click an element identified by CSS selector.
    async fn tool_browser_click(&self, params: Value) -> Result<Value> {
        let selector = params
            .get("selector")
            .and_then(|v| v.as_str())
            .ok_or_else(|| AdapterError::InvalidParams {
                tool_name: "browser_click".into(),
                reason: "missing required string field `selector`".into(),
            })?;

        debug!(selector = selector, "clicking element");

        // Use Runtime.evaluate to find and click the element.
        let js = format!(
            r#"(() => {{
                const el = document.querySelector({selector});
                if (!el) return JSON.stringify({{ error: "element not found", selector: {selector} }});
                el.click();
                return JSON.stringify({{ success: true, tag: el.tagName, selector: {selector} }});
            }})()"#,
            selector = serde_json::to_string(selector).map_err(AdapterError::from)?
        );

        let result = self
            .send_cdp_command(
                "Runtime.evaluate",
                json!({
                    "expression": js,
                    "returnByValue": true,
                }),
            )
            .await?;

        let value_str = extract_runtime_value(&result)?;

        // Parse the JSON returned by the script.
        let click_result: Value =
            serde_json::from_str(&value_str).unwrap_or_else(|_| json!({ "result": value_str }));

        if click_result.get("error").is_some() {
            return Err(AdapterError::ExecutionFailed {
                tool_name: "browser_click".into(),
                reason: format!("element not found for selector `{selector}`"),
            });
        }

        Ok(click_result)
    }

    /// Type text into the currently focused element.
    async fn tool_browser_type_text(&self, params: Value) -> Result<Value> {
        let text = params.get("text").and_then(|v| v.as_str()).ok_or_else(|| {
            AdapterError::InvalidParams {
                tool_name: "browser_type_text".into(),
                reason: "missing required string field `text`".into(),
            }
        })?;

        debug!(text_length = text.len(), "typing text into focused element");

        // Use Runtime.evaluate to set the value of the focused element and
        // dispatch an input event for frameworks that listen to events.
        let js = format!(
            r#"(() => {{
                const el = document.activeElement;
                if (!el || el === document.body) {{
                    return JSON.stringify({{ error: "no element focused" }});
                }}
                const text = {text};
                if ('value' in el) {{
                    el.value += text;
                    el.dispatchEvent(new Event('input', {{ bubbles: true }}));
                    el.dispatchEvent(new Event('change', {{ bubbles: true }}));
                }} else {{
                    el.textContent += text;
                    el.dispatchEvent(new Event('input', {{ bubbles: true }}));
                }}
                return JSON.stringify({{ success: true, tag: el.tagName, typed_length: text.length }});
            }})()"#,
            text = serde_json::to_string(text).map_err(AdapterError::from)?
        );

        let result = self
            .send_cdp_command(
                "Runtime.evaluate",
                json!({
                    "expression": js,
                    "returnByValue": true,
                }),
            )
            .await?;

        let value_str = extract_runtime_value(&result)?;
        let type_result: Value =
            serde_json::from_str(&value_str).unwrap_or_else(|_| json!({ "result": value_str }));

        if type_result.get("error").is_some() {
            return Err(AdapterError::ExecutionFailed {
                tool_name: "browser_type_text".into(),
                reason: "no element focused to receive text input".into(),
            });
        }

        Ok(type_result)
    }

    /// Evaluate arbitrary JavaScript in the page context.
    async fn tool_browser_evaluate(&self, params: Value) -> Result<Value> {
        let expression = params
            .get("expression")
            .and_then(|v| v.as_str())
            .ok_or_else(|| AdapterError::InvalidParams {
                tool_name: "browser_evaluate".into(),
                reason: "missing required string field `expression`".into(),
            })?;

        debug!(
            expression_length = expression.len(),
            "evaluating JavaScript"
        );

        let result = self
            .send_cdp_command(
                "Runtime.evaluate",
                json!({
                    "expression": expression,
                    "returnByValue": true,
                    "awaitPromise": true,
                }),
            )
            .await?;

        // Check for exception.
        if let Some(exception) = result.get("exceptionDetails") {
            let exception_text = exception
                .get("text")
                .and_then(|v| v.as_str())
                .unwrap_or("unknown exception");
            return Err(AdapterError::ExecutionFailed {
                tool_name: "browser_evaluate".into(),
                reason: format!("JavaScript exception: {exception_text}"),
            });
        }

        let value = result.get("result").cloned().unwrap_or(json!(null));

        Ok(json!({
            "result": value,
        }))
    }
}

/// Extract the string value from a `Runtime.evaluate` CDP response.
///
/// The CDP response shape is: `{ "result": { "type": "string", "value": "..." } }`.
fn extract_runtime_value(cdp_result: &Value) -> Result<String> {
    let result_obj = cdp_result
        .get("result")
        .ok_or_else(|| AdapterError::ExecutionFailed {
            tool_name: "browser".into(),
            reason: "CDP response missing `result` field".into(),
        })?;

    // Check for exceptions in the evaluation.
    if let Some(exception) = cdp_result.get("exceptionDetails") {
        let exception_text = exception
            .get("text")
            .and_then(|v| v.as_str())
            .unwrap_or("unknown exception");
        return Err(AdapterError::ExecutionFailed {
            tool_name: "browser".into(),
            reason: format!("JavaScript exception: {exception_text}"),
        });
    }

    // The value can be of different types: string, number, boolean, object.
    match result_obj.get("value") {
        Some(Value::String(s)) => Ok(s.clone()),
        Some(other) => Ok(other.to_string()),
        None => {
            // Some evaluations return undefined.
            let result_type = result_obj
                .get("type")
                .and_then(|v| v.as_str())
                .unwrap_or("undefined");
            Ok(result_type.to_string())
        }
    }
}

/// Check whether a command exists on the system PATH (non-blocking best-effort).
fn which_exists(name: &str) -> bool {
    std::process::Command::new("which")
        .arg(name)
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status()
        .map(|s| s.success())
        .unwrap_or(false)
}

/// Build a CDP message JSON object (useful for testing).
pub fn build_cdp_message(id: u64, method: &str, params: Value) -> Value {
    json!({
        "id": id,
        "method": method,
        "params": params,
    })
}

#[async_trait]
impl Adapter for BrowserAdapter {
    fn id(&self) -> &str {
        &self.id
    }

    fn adapter_type(&self) -> AdapterType {
        AdapterType::Productivity
    }

    async fn connect(&mut self) -> Result<()> {
        info!(
            id = %self.id,
            port = self.debug_port,
            "connecting browser adapter"
        );

        if self.is_devtools_reachable().await {
            info!("DevTools endpoint already reachable");
            self.connected.store(true, Ordering::Release);
            return Ok(());
        }

        // Try to launch Chrome.
        self.try_launch_chrome().await?;
        self.connected.store(true, Ordering::Release);
        info!(id = %self.id, "browser adapter connected");
        Ok(())
    }

    async fn disconnect(&mut self) -> Result<()> {
        info!(id = %self.id, "browser adapter disconnected");
        self.connected.store(false, Ordering::Release);
        Ok(())
    }

    async fn health_check(&self) -> Result<HealthStatus> {
        if !self.connected.load(Ordering::Acquire) {
            return Ok(HealthStatus::Unhealthy);
        }

        // Verify DevTools is still reachable.
        if self.is_devtools_reachable().await {
            Ok(HealthStatus::Healthy)
        } else {
            warn!(id = %self.id, "DevTools endpoint unreachable during health check");
            Ok(HealthStatus::Degraded)
        }
    }

    fn tools(&self) -> Vec<ToolDefinition> {
        vec![
            ToolDefinition {
                name: "browser_navigate".into(),
                description: "Navigate the browser to a URL".into(),
                parameters: json!({
                    "type": "object",
                    "properties": {
                        "url": {
                            "type": "string",
                            "description": "The URL to navigate to"
                        }
                    },
                    "required": ["url"]
                }),
            },
            ToolDefinition {
                name: "browser_get_page_content".into(),
                description: "Get the current page's text content (innerText of body)".into(),
                parameters: json!({
                    "type": "object",
                    "properties": {},
                    "required": []
                }),
            },
            ToolDefinition {
                name: "browser_screenshot".into(),
                description: "Take a screenshot of the current page (returns base64-encoded image)"
                    .into(),
                parameters: json!({
                    "type": "object",
                    "properties": {
                        "format": {
                            "type": "string",
                            "description": "Image format: \"png\" (default) or \"jpeg\"",
                            "enum": ["png", "jpeg"]
                        }
                    },
                    "required": []
                }),
            },
            ToolDefinition {
                name: "browser_click".into(),
                description: "Click an element identified by CSS selector".into(),
                parameters: json!({
                    "type": "object",
                    "properties": {
                        "selector": {
                            "type": "string",
                            "description": "CSS selector for the element to click"
                        }
                    },
                    "required": ["selector"]
                }),
            },
            ToolDefinition {
                name: "browser_type_text".into(),
                description: "Type text into the currently focused element".into(),
                parameters: json!({
                    "type": "object",
                    "properties": {
                        "text": {
                            "type": "string",
                            "description": "The text to type"
                        }
                    },
                    "required": ["text"]
                }),
            },
            ToolDefinition {
                name: "browser_evaluate".into(),
                description: "Evaluate a JavaScript expression in the page context".into(),
                parameters: json!({
                    "type": "object",
                    "properties": {
                        "expression": {
                            "type": "string",
                            "description": "The JavaScript expression to evaluate"
                        }
                    },
                    "required": ["expression"]
                }),
            },
        ]
    }

    async fn execute_tool(&self, name: &str, params: Value) -> Result<Value> {
        if !self.connected.load(Ordering::Acquire) {
            return Err(AdapterError::ExecutionFailed {
                tool_name: name.to_string(),
                reason: format!("adapter `{}` is not connected", self.id),
            });
        }

        match name {
            "browser_navigate" => self.tool_browser_navigate(params).await,
            "browser_get_page_content" => self.tool_browser_get_page_content(params).await,
            "browser_screenshot" => self.tool_browser_screenshot(params).await,
            "browser_click" => self.tool_browser_click(params).await,
            "browser_type_text" => self.tool_browser_type_text(params).await,
            "browser_evaluate" => self.tool_browser_evaluate(params).await,
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
    fn browser_adapter_new_default_port() {
        let adapter = BrowserAdapter::new("test-browser");
        assert_eq!(adapter.id(), "test-browser");
        assert_eq!(adapter.debug_port, DEFAULT_DEBUG_PORT);
        assert!(!adapter.connected.load(Ordering::Relaxed));
        assert!(adapter.chrome_path.is_none());
    }

    #[test]
    fn browser_adapter_with_custom_port() {
        let adapter = BrowserAdapter::with_port("test-browser", 9333);
        assert_eq!(adapter.debug_port, 9333);
    }

    #[test]
    fn browser_adapter_with_chrome_path() {
        let adapter = BrowserAdapter::new("test-browser").with_chrome_path("/usr/bin/chromium");
        assert_eq!(adapter.chrome_path.as_deref(), Some("/usr/bin/chromium"));
    }

    #[test]
    fn browser_adapter_type_is_productivity() {
        let adapter = BrowserAdapter::new("test-browser");
        assert_eq!(adapter.adapter_type(), AdapterType::Productivity);
    }

    #[test]
    fn browser_adapter_no_auth_required() {
        let adapter = BrowserAdapter::new("test-browser");
        assert!(adapter.required_auth().is_none());
    }

    #[test]
    fn browser_adapter_tools_count() {
        let adapter = BrowserAdapter::new("test-browser");
        let tools = adapter.tools();
        assert_eq!(tools.len(), 6);
    }

    #[test]
    fn browser_adapter_tool_names() {
        let adapter = BrowserAdapter::new("test-browser");
        let tools = adapter.tools();
        let names: Vec<&str> = tools.iter().map(|t| t.name.as_str()).collect();
        assert!(names.contains(&"browser_navigate"));
        assert!(names.contains(&"browser_get_page_content"));
        assert!(names.contains(&"browser_screenshot"));
        assert!(names.contains(&"browser_click"));
        assert!(names.contains(&"browser_type_text"));
        assert!(names.contains(&"browser_evaluate"));
    }

    #[test]
    fn browser_adapter_tool_parameters_have_required_fields() {
        let adapter = BrowserAdapter::new("test-browser");
        let tools = adapter.tools();

        // browser_navigate requires "url".
        let nav = tools.iter().find(|t| t.name == "browser_navigate");
        assert!(nav.is_some());
        let nav = nav.expect("should exist in tests");
        let required = nav.parameters.get("required").and_then(|v| v.as_array());
        assert!(required.is_some());
        assert!(
            required
                .expect("should exist in tests")
                .contains(&json!("url"))
        );

        // browser_get_page_content has no required params.
        let content = tools
            .iter()
            .find(|t| t.name == "browser_get_page_content")
            .expect("should exist in tests");
        let required = content
            .parameters
            .get("required")
            .and_then(|v| v.as_array())
            .expect("should exist in tests");
        assert!(required.is_empty());

        // browser_click requires "selector".
        let click = tools
            .iter()
            .find(|t| t.name == "browser_click")
            .expect("should exist in tests");
        let required = click
            .parameters
            .get("required")
            .and_then(|v| v.as_array())
            .expect("should exist in tests");
        assert!(required.contains(&json!("selector")));

        // browser_type_text requires "text".
        let type_text = tools
            .iter()
            .find(|t| t.name == "browser_type_text")
            .expect("should exist in tests");
        let required = type_text
            .parameters
            .get("required")
            .and_then(|v| v.as_array())
            .expect("should exist in tests");
        assert!(required.contains(&json!("text")));

        // browser_evaluate requires "expression".
        let evaluate = tools
            .iter()
            .find(|t| t.name == "browser_evaluate")
            .expect("should exist in tests");
        let required = evaluate
            .parameters
            .get("required")
            .and_then(|v| v.as_array())
            .expect("should exist in tests");
        assert!(required.contains(&json!("expression")));
    }

    #[tokio::test]
    async fn browser_adapter_health_when_not_connected() {
        let adapter = BrowserAdapter::new("test-browser");
        let status = adapter.health_check().await;
        assert!(status.is_ok());
        assert_eq!(
            status.expect("should be ok in tests"),
            HealthStatus::Unhealthy
        );
    }

    #[tokio::test]
    async fn browser_adapter_rejects_tool_when_not_connected() {
        let adapter = BrowserAdapter::new("test-browser");
        let result = adapter
            .execute_tool("browser_navigate", json!({"url": "https://example.com"}))
            .await;
        assert!(result.is_err());
        let err = result.unwrap_err();
        let err_str = err.to_string();
        assert!(err_str.contains("not connected"));
    }

    #[tokio::test]
    async fn browser_adapter_rejects_unknown_tool() {
        let adapter = BrowserAdapter::new("test-browser");
        // Manually set connected to test tool dispatch.
        adapter.connected.store(true, Ordering::Release);

        let result = adapter.execute_tool("nonexistent_tool", json!({})).await;
        assert!(result.is_err());
        match result.unwrap_err() {
            AdapterError::ToolNotFound {
                adapter_id,
                tool_name,
            } => {
                assert_eq!(adapter_id, "test-browser");
                assert_eq!(tool_name, "nonexistent_tool");
            }
            other => panic!("expected ToolNotFound, got: {other:?}"),
        }
    }

    #[tokio::test]
    async fn browser_adapter_connect_disconnect() {
        let mut adapter = BrowserAdapter::new("test-browser");
        assert!(!adapter.connected.load(Ordering::Relaxed));

        adapter
            .disconnect()
            .await
            .expect("disconnect should succeed in tests");
        assert!(!adapter.connected.load(Ordering::Relaxed));
    }

    #[test]
    fn cdp_message_construction() {
        let msg = build_cdp_message(1, "Page.navigate", json!({"url": "https://example.com"}));
        assert_eq!(msg.get("id").and_then(|v| v.as_u64()), Some(1));
        assert_eq!(
            msg.get("method").and_then(|v| v.as_str()),
            Some("Page.navigate")
        );
        assert_eq!(
            msg.get("params")
                .and_then(|v| v.get("url"))
                .and_then(|v| v.as_str()),
            Some("https://example.com")
        );
    }

    #[test]
    fn cdp_message_runtime_evaluate() {
        let msg = build_cdp_message(
            42,
            "Runtime.evaluate",
            json!({
                "expression": "document.title",
                "returnByValue": true,
            }),
        );
        assert_eq!(msg.get("id").and_then(|v| v.as_u64()), Some(42));
        assert_eq!(
            msg.get("method").and_then(|v| v.as_str()),
            Some("Runtime.evaluate")
        );
        assert_eq!(
            msg.get("params")
                .and_then(|v| v.get("expression"))
                .and_then(|v| v.as_str()),
            Some("document.title")
        );
        assert_eq!(
            msg.get("params")
                .and_then(|v| v.get("returnByValue"))
                .and_then(|v| v.as_bool()),
            Some(true)
        );
    }

    #[test]
    fn cdp_message_screenshot() {
        let msg = build_cdp_message(5, "Page.captureScreenshot", json!({"format": "png"}));
        assert_eq!(msg.get("id").and_then(|v| v.as_u64()), Some(5));
        assert_eq!(
            msg.get("method").and_then(|v| v.as_str()),
            Some("Page.captureScreenshot")
        );
        assert_eq!(
            msg.get("params")
                .and_then(|v| v.get("format"))
                .and_then(|v| v.as_str()),
            Some("png")
        );
    }

    #[test]
    fn extract_runtime_value_string() {
        let cdp_response = json!({
            "result": {
                "type": "string",
                "value": "Hello, World!"
            }
        });
        let value = extract_runtime_value(&cdp_response);
        assert!(value.is_ok());
        assert_eq!(value.expect("should be ok in tests"), "Hello, World!");
    }

    #[test]
    fn extract_runtime_value_number() {
        let cdp_response = json!({
            "result": {
                "type": "number",
                "value": 42
            }
        });
        let value = extract_runtime_value(&cdp_response);
        assert!(value.is_ok());
        assert_eq!(value.expect("should be ok in tests"), "42");
    }

    #[test]
    fn extract_runtime_value_undefined() {
        let cdp_response = json!({
            "result": {
                "type": "undefined"
            }
        });
        let value = extract_runtime_value(&cdp_response);
        assert!(value.is_ok());
        assert_eq!(value.expect("should be ok in tests"), "undefined");
    }

    #[test]
    fn extract_runtime_value_exception() {
        let cdp_response = json!({
            "result": {
                "type": "object",
                "subtype": "error"
            },
            "exceptionDetails": {
                "text": "ReferenceError: foo is not defined"
            }
        });
        let value = extract_runtime_value(&cdp_response);
        assert!(value.is_err());
        let err_str = value.unwrap_err().to_string();
        assert!(err_str.contains("JavaScript exception"));
    }

    #[test]
    fn extract_runtime_value_missing_result() {
        let cdp_response = json!({});
        let value = extract_runtime_value(&cdp_response);
        assert!(value.is_err());
    }

    #[tokio::test]
    async fn browser_adapter_navigate_validates_url() {
        let adapter = BrowserAdapter::new("test-browser");
        adapter.connected.store(true, Ordering::Release);

        // Invalid URL should fail with InvalidParams, not a network error.
        let result = adapter
            .execute_tool("browser_navigate", json!({"url": "not a valid url"}))
            .await;
        assert!(result.is_err());
        match result.unwrap_err() {
            AdapterError::InvalidParams { tool_name, reason } => {
                assert_eq!(tool_name, "browser_navigate");
                assert!(reason.contains("invalid URL"));
            }
            other => panic!("expected InvalidParams, got: {other:?}"),
        }
    }

    #[tokio::test]
    async fn browser_adapter_navigate_requires_url_param() {
        let adapter = BrowserAdapter::new("test-browser");
        adapter.connected.store(true, Ordering::Release);

        let result = adapter.execute_tool("browser_navigate", json!({})).await;
        assert!(result.is_err());
        match result.unwrap_err() {
            AdapterError::InvalidParams { tool_name, .. } => {
                assert_eq!(tool_name, "browser_navigate");
            }
            other => panic!("expected InvalidParams, got: {other:?}"),
        }
    }

    #[tokio::test]
    async fn browser_adapter_click_requires_selector_param() {
        let adapter = BrowserAdapter::new("test-browser");
        adapter.connected.store(true, Ordering::Release);

        let result = adapter.execute_tool("browser_click", json!({})).await;
        assert!(result.is_err());
        match result.unwrap_err() {
            AdapterError::InvalidParams { tool_name, reason } => {
                assert_eq!(tool_name, "browser_click");
                assert!(reason.contains("selector"));
            }
            other => panic!("expected InvalidParams, got: {other:?}"),
        }
    }

    #[tokio::test]
    async fn browser_adapter_type_text_requires_text_param() {
        let adapter = BrowserAdapter::new("test-browser");
        adapter.connected.store(true, Ordering::Release);

        let result = adapter.execute_tool("browser_type_text", json!({})).await;
        assert!(result.is_err());
        match result.unwrap_err() {
            AdapterError::InvalidParams { tool_name, reason } => {
                assert_eq!(tool_name, "browser_type_text");
                assert!(reason.contains("text"));
            }
            other => panic!("expected InvalidParams, got: {other:?}"),
        }
    }

    #[tokio::test]
    async fn browser_adapter_evaluate_requires_expression_param() {
        let adapter = BrowserAdapter::new("test-browser");
        adapter.connected.store(true, Ordering::Release);

        let result = adapter.execute_tool("browser_evaluate", json!({})).await;
        assert!(result.is_err());
        match result.unwrap_err() {
            AdapterError::InvalidParams { tool_name, reason } => {
                assert_eq!(tool_name, "browser_evaluate");
                assert!(reason.contains("expression"));
            }
            other => panic!("expected InvalidParams, got: {other:?}"),
        }
    }

    #[tokio::test]
    async fn browser_adapter_screenshot_rejects_invalid_format() {
        let adapter = BrowserAdapter::new("test-browser");
        adapter.connected.store(true, Ordering::Release);

        let result = adapter
            .execute_tool("browser_screenshot", json!({"format": "bmp"}))
            .await;
        assert!(result.is_err());
        match result.unwrap_err() {
            AdapterError::InvalidParams { tool_name, reason } => {
                assert_eq!(tool_name, "browser_screenshot");
                assert!(reason.contains("bmp"));
            }
            other => panic!("expected InvalidParams, got: {other:?}"),
        }
    }

    #[test]
    fn devtools_base_url_default_port() {
        let adapter = BrowserAdapter::new("test");
        assert_eq!(adapter.devtools_base_url(), "http://localhost:9222");
    }

    #[test]
    fn devtools_base_url_custom_port() {
        let adapter = BrowserAdapter::with_port("test", 9333);
        assert_eq!(adapter.devtools_base_url(), "http://localhost:9333");
    }

    #[test]
    fn next_id_increments() {
        let adapter = BrowserAdapter::new("test");
        let id1 = adapter.next_id();
        let id2 = adapter.next_id();
        let id3 = adapter.next_id();
        assert_eq!(id1 + 1, id2);
        assert_eq!(id2 + 1, id3);
    }
}
