//! Tool implementations for the browser adapter.
//!
//! Each function corresponds to one LLM-callable tool.  All functions take
//! a reference to the BrowserAdapter so they can call CDP commands via the
//! shared client and atomic state.

use serde_json::{Value, json};
use tracing::debug;

use crate::error::{AdapterError, Result};

use super::cdp::CDP_TIMEOUT_SECS;

/// Navigate the browser to a URL.
pub async fn tool_browser_navigate(
    send: &impl CdpSender,
    params: Value,
) -> Result<Value> {
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

    let result = send
        .send_cdp("Page.navigate", json!({ "url": url_str }), CDP_TIMEOUT_SECS)
        .await?;

    let frame_id = result
        .get("frameId")
        .and_then(|v| v.as_str())
        .unwrap_or("unknown");

    Ok(json!({
        "success": true,
        "url": url_str,
        "frame_id": frame_id,
    }))
}

/// Get the current page's text content.
pub async fn tool_browser_get_page_content(send: &impl CdpSender) -> Result<Value> {
    debug!("getting page content");

    let expression = "document.body.innerText";
    let result = send
        .send_cdp(
            "Runtime.evaluate",
            json!({
                "expression": expression,
                "returnByValue": true,
            }),
            CDP_TIMEOUT_SECS,
        )
        .await?;

    let content = extract_runtime_value(&result)?;

    Ok(json!({
        "content": content,
        "length": content.len(),
    }))
}

/// Take a screenshot of the current page.
pub async fn tool_browser_screenshot(send: &impl CdpSender, params: Value) -> Result<Value> {
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

    let result = send
        .send_cdp(
            "Page.captureScreenshot",
            json!({ "format": format }),
            CDP_TIMEOUT_SECS,
        )
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
pub async fn tool_browser_click(send: &impl CdpSender, params: Value) -> Result<Value> {
    let selector = params
        .get("selector")
        .and_then(|v| v.as_str())
        .ok_or_else(|| AdapterError::InvalidParams {
            tool_name: "browser_click".into(),
            reason: "missing required string field `selector`".into(),
        })?;

    debug!(selector = selector, "clicking element");

    let js = format!(
        r#"(() => {{
            const el = document.querySelector({selector});
            if (!el) return JSON.stringify({{ error: "element not found", selector: {selector} }});
            el.click();
            return JSON.stringify({{ success: true, tag: el.tagName, selector: {selector} }});
        }})()"#,
        selector = serde_json::to_string(selector).map_err(AdapterError::from)?
    );

    let result = send
        .send_cdp(
            "Runtime.evaluate",
            json!({
                "expression": js,
                "returnByValue": true,
            }),
            CDP_TIMEOUT_SECS,
        )
        .await?;

    let value_str = extract_runtime_value(&result)?;

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
pub async fn tool_browser_type_text(send: &impl CdpSender, params: Value) -> Result<Value> {
    let text = params.get("text").and_then(|v| v.as_str()).ok_or_else(|| {
        AdapterError::InvalidParams {
            tool_name: "browser_type_text".into(),
            reason: "missing required string field `text`".into(),
        }
    })?;

    debug!(text_length = text.len(), "typing text into focused element");

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

    let result = send
        .send_cdp(
            "Runtime.evaluate",
            json!({
                "expression": js,
                "returnByValue": true,
            }),
            CDP_TIMEOUT_SECS,
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
pub async fn tool_browser_evaluate(send: &impl CdpSender, params: Value) -> Result<Value> {
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

    let result = send
        .send_cdp(
            "Runtime.evaluate",
            json!({
                "expression": expression,
                "returnByValue": true,
                "awaitPromise": true,
            }),
            120,
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

/// Extract the string value from a `Runtime.evaluate` CDP response.
///
/// The CDP response shape is: `{ "result": { "type": "string", "value": "..." } }`.
pub fn extract_runtime_value(cdp_result: &Value) -> Result<String> {
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

// ---------------------------------------------------------------------------
// CdpSender trait — abstraction over the adapter for testability
// ---------------------------------------------------------------------------

/// Trait that abstracts CDP command dispatch for tool functions.
#[async_trait::async_trait]
pub trait CdpSender: Send + Sync {
    async fn send_cdp(&self, method: &str, params: Value, timeout_secs: u64) -> Result<Value>;
}
