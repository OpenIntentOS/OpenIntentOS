//! Chrome DevTools Protocol communication helpers.
//!
//! Provides WebSocket-based CDP command sending, target discovery, and
//! Chrome process management utilities used by the BrowserAdapter.

use std::time::Duration;

use futures::{SinkExt, StreamExt};
use serde_json::{Value, json};
use tokio_tungstenite::connect_async;
use tokio_tungstenite::tungstenite::Message;
use tracing::{debug, info};

use crate::error::{AdapterError, Result};

/// Timeout for CDP WebSocket operations in seconds.
pub const CDP_TIMEOUT_SECS: u64 = 30;

/// Timeout for HTTP requests to the DevTools endpoint in seconds.
pub const HTTP_TIMEOUT_SECS: u64 = 10;

/// Timeout waiting for Chrome to start up in seconds.
pub const CHROME_STARTUP_TIMEOUT_SECS: u64 = 10;

/// Maximum response body size from CDP in bytes (5 MB).
pub const MAX_CDP_RESPONSE_BYTES: usize = 5 * 1024 * 1024;

/// Return the base URL for the DevTools HTTP endpoint.
pub fn devtools_base_url(debug_port: u16) -> String {
    format!("http://localhost:{debug_port}")
}

/// Check if the DevTools endpoint is reachable.
pub async fn is_devtools_reachable(client: &reqwest::Client, debug_port: u16) -> bool {
    let url = format!("{}/json/version", devtools_base_url(debug_port));
    client.get(&url).send().await.is_ok()
}

/// Get the list of page targets from the DevTools endpoint.
pub async fn get_page_targets(client: &reqwest::Client, debug_port: u16) -> Result<Vec<Value>> {
    let url = format!("{}/json", devtools_base_url(debug_port));
    let response = client
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
pub async fn get_ws_url(client: &reqwest::Client, debug_port: u16) -> Result<String> {
    let pages = get_page_targets(client, debug_port).await?;
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
pub async fn send_cdp_command_with_timeout(
    client: &reqwest::Client,
    debug_port: u16,
    msg_id: u64,
    method: &str,
    params: Value,
    timeout_secs: u64,
) -> Result<Value> {
    let ws_url = get_ws_url(client, debug_port).await?;

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
    let result = tokio::time::timeout(Duration::from_secs(timeout_secs), async {
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
        seconds: timeout_secs,
        reason: format!("waiting for CDP response to `{method}`"),
    })?;

    // Attempt to close the WebSocket cleanly (best-effort).
    let _ = sink.send(Message::Close(None)).await;

    result
}

/// Attempt to launch Chrome with remote debugging enabled.
pub async fn try_launch_chrome(
    client: &reqwest::Client,
    debug_port: u16,
    chrome_path: &str,
) -> Result<()> {
    info!(
        chrome_path = %chrome_path,
        port = debug_port,
        "launching Chrome with remote debugging"
    );

    let mut cmd = tokio::process::Command::new(chrome_path);
    cmd.arg(format!("--remote-debugging-port={debug_port}"))
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
        if is_devtools_reachable(client, debug_port).await {
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
pub fn find_chrome_path(custom_path: Option<&str>) -> Result<String> {
    if let Some(path) = custom_path {
        return Ok(path.to_string());
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

/// Check whether a command exists on the system PATH (non-blocking best-effort).
pub fn which_exists(name: &str) -> bool {
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
