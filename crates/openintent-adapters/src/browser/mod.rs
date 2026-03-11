//! Browser adapter — control a Chromium-based browser via the Chrome DevTools Protocol.
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

pub mod cdp;
pub mod tools;
pub mod types;

use async_trait::async_trait;
use serde_json::{Value, json};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::time::Duration;
use tracing::{info, warn};

use crate::error::{AdapterError, Result};
use crate::traits::{Adapter, AdapterType, AuthRequirement, HealthStatus, ToolDefinition};

use cdp::{HTTP_TIMEOUT_SECS, find_chrome_path, is_devtools_reachable, send_cdp_command_with_timeout, try_launch_chrome};
use tools::{
    CdpSender, tool_browser_click, tool_browser_evaluate,
    tool_browser_get_page_content, tool_browser_navigate, tool_browser_screenshot,
    tool_browser_type_text,
};

/// Default Chrome DevTools Protocol debug port.
const DEFAULT_DEBUG_PORT: u16 = 9222;

/// Browser service adapter using Chrome DevTools Protocol.
pub struct BrowserAdapter {
    /// Unique identifier for this adapter instance.
    id: String,
    /// Whether the adapter has been connected.
    pub(crate) connected: AtomicBool,
    /// Optional path to the Chrome/Chromium executable.
    chrome_path: Option<String>,
    /// The remote debugging port.
    pub(crate) debug_port: u16,
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
    pub(crate) fn devtools_base_url(&self) -> String {
        cdp::devtools_base_url(self.debug_port)
    }

    /// Allocate the next CDP message ID.
    pub(crate) fn next_id(&self) -> u64 {
        self.next_message_id.fetch_add(1, Ordering::Relaxed)
    }

    /// Check if the DevTools endpoint is reachable.
    async fn is_devtools_reachable_inner(&self) -> bool {
        is_devtools_reachable(&self.client, self.debug_port).await
    }

    /// Attempt to launch Chrome with remote debugging enabled.
    async fn try_launch_chrome_inner(&self) -> Result<()> {
        let chrome_path = find_chrome_path(self.chrome_path.as_deref())?;
        try_launch_chrome(&self.client, self.debug_port, &chrome_path).await
    }
}

#[async_trait]
impl CdpSender for BrowserAdapter {
    async fn send_cdp(&self, method: &str, params: Value, timeout_secs: u64) -> Result<Value> {
        let msg_id = self.next_id();
        send_cdp_command_with_timeout(
            &self.client,
            self.debug_port,
            msg_id,
            method,
            params,
            timeout_secs,
        )
        .await
    }
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

        if self.is_devtools_reachable_inner().await {
            info!("DevTools endpoint already reachable");
            self.connected.store(true, Ordering::Release);
            return Ok(());
        }

        // Try to launch Chrome.
        self.try_launch_chrome_inner().await?;
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
        if self.is_devtools_reachable_inner().await {
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
            "browser_navigate" => tool_browser_navigate(self, params).await,
            "browser_get_page_content" => tool_browser_get_page_content(self).await,
            "browser_screenshot" => tool_browser_screenshot(self, params).await,
            "browser_click" => tool_browser_click(self, params).await,
            "browser_type_text" => tool_browser_type_text(self, params).await,
            "browser_evaluate" => tool_browser_evaluate(self, params).await,
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
    use cdp::build_cdp_message;
    use tools::extract_runtime_value;

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
