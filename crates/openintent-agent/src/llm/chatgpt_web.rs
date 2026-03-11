//! ChatGPT Pro web session client.
//!
//! Uses the ChatGPT backend-api (`chatgpt.com/backend-api/conversation`) with
//! session-token-based authentication, allowing ChatGPT Pro subscribers to use
//! their subscription programmatically without needing an OpenAI API key.

use futures::StreamExt;
use reqwest::header::{HeaderMap, HeaderValue, AUTHORIZATION, CONTENT_TYPE};
use serde_json::{Value, json};
use std::sync::Arc;
use tokio::sync::RwLock;

use crate::error::{AgentError, Result};
use crate::llm::streaming_chatgpt_web::ChatGptWebStreamAccumulator;
use crate::llm::types::{
    ChatRequest, LlmResponse, Role, ToolDefinition, Usage,
};

/// Default ChatGPT web base URL.
pub const BASE_URL: &str = "https://chatgpt.com";

/// Browser-like User-Agent header.
const USER_AGENT: &str = "Mozilla/5.0 (Macintosh; Intel Mac OS X 10_15_7) \
    AppleWebKit/537.36 (KHTML, like Gecko) Chrome/133.0.0.0 Safari/537.36";

// ---------------------------------------------------------------------------
// Auth
// ---------------------------------------------------------------------------

/// Manages ChatGPT web session authentication.
///
/// Exchanges a long-lived session token (from browser cookies) for a
/// short-lived access token (JWT) and automatically refreshes it when it
/// nears expiry.
#[derive(Debug)]
pub struct ChatGptWebAuth {
    session_token: String,
    base_url: String,
    state: RwLock<AuthState>,
    http: reqwest::Client,
}

#[derive(Debug, Clone)]
struct AuthState {
    access_token: String,
    device_id: String,
    expires_at: chrono::DateTime<chrono::Utc>,
}

impl ChatGptWebAuth {
    /// Create a new auth manager with the given session token.
    pub fn new(session_token: String, base_url: String) -> Self {
        let device_id = uuid::Uuid::new_v4().to_string();
        Self {
            session_token,
            base_url,
            state: RwLock::new(AuthState {
                access_token: String::new(),
                device_id,
                expires_at: chrono::Utc::now(),
            }),
            http: reqwest::Client::builder()
                .timeout(std::time::Duration::from_secs(30))
                .build()
                .unwrap_or_default(),
        }
    }

    /// Get a valid access token, refreshing if needed.
    pub async fn get_access_token(&self) -> Result<String> {
        let state = self.state.read().await;
        let buffer = chrono::Duration::minutes(5);
        if !state.access_token.is_empty()
            && state.expires_at > chrono::Utc::now() + buffer
        {
            return Ok(state.access_token.clone());
        }
        drop(state);
        self.refresh().await
    }

    /// Get the device ID (persisted for the session lifetime).
    pub async fn device_id(&self) -> String {
        self.state.read().await.device_id.clone()
    }

    /// Refresh the access token by exchanging the session token.
    async fn refresh(&self) -> Result<String> {
        tracing::debug!("refreshing ChatGPT web access token");

        // If the stored token is already a JWT access token (starts with "eyJ"),
        // use it directly instead of trying to exchange via the session endpoint
        // (which is blocked by Cloudflare for non-browser requests).
        if self.session_token.starts_with("eyJ") {
            let expires_at = decode_jwt_expiry(&self.session_token)
                .unwrap_or_else(|| chrono::Utc::now() + chrono::Duration::hours(1));

            if expires_at <= chrono::Utc::now() {
                return Err(AgentError::LlmRequestFailed {
                    reason: "ChatGPT session token has expired — send /chatgpt in Telegram to refresh.".into(),
                });
            }

            tracing::info!(expires = %expires_at, "using stored JWT directly as access token");
            let mut state = self.state.write().await;
            state.access_token = self.session_token.clone();
            state.expires_at = expires_at;
            return Ok(self.session_token.clone());
        }

        let url = format!("{}/api/auth/session", self.base_url);
        let cookie = format!(
            "__Secure-next-auth.session-token={}",
            self.session_token
        );

        let resp = self
            .http
            .get(&url)
            .header("Cookie", &cookie)
            .header("User-Agent", USER_AGENT)
            .send()
            .await
            .map_err(|e| AgentError::LlmRequestFailed {
                reason: format!("ChatGPT auth refresh failed: {e}"),
            })?;

        let status = resp.status();
        let body = resp.text().await.map_err(|e| AgentError::LlmRequestFailed {
            reason: format!("ChatGPT auth response read failed: {e}"),
        })?;

        if !status.is_success() {
            return Err(AgentError::LlmRequestFailed {
                reason: format!(
                    "ChatGPT auth returned {status}: {body}. \
                     Session token may be expired — re-copy from browser."
                ),
            });
        }

        let v: Value = serde_json::from_str(&body).map_err(|e| {
            AgentError::LlmParseFailed {
                reason: format!("ChatGPT auth response parse failed: {e}"),
            }
        })?;

        let access_token = v["accessToken"]
            .as_str()
            .ok_or_else(|| AgentError::LlmParseFailed {
                reason: "missing `accessToken` in ChatGPT auth response".into(),
            })?
            .to_owned();

        // Parse expiry — the response contains an ISO 8601 string.
        let expires_at = v["expires"]
            .as_str()
            .and_then(|s| chrono::DateTime::parse_from_rfc3339(s).ok())
            .map(|dt| dt.with_timezone(&chrono::Utc))
            .unwrap_or_else(|| chrono::Utc::now() + chrono::Duration::hours(1));

        let mut state = self.state.write().await;
        state.access_token = access_token.clone();
        state.expires_at = expires_at;

        tracing::info!(
            expires = %expires_at,
            "ChatGPT web access token refreshed"
        );

        Ok(access_token)
    }
}

/// Decode the `exp` claim from a JWT without signature verification.
/// Returns the expiry as a UTC datetime, or `None` if decoding fails.
fn decode_jwt_expiry(jwt: &str) -> Option<chrono::DateTime<chrono::Utc>> {
    use base64::Engine;
    let payload = jwt.split('.').nth(1)?;
    let engine = base64::engine::general_purpose::URL_SAFE_NO_PAD;
    let bytes = engine.decode(payload).ok()?;
    let v: Value = serde_json::from_slice(&bytes).ok()?;
    let exp = v["exp"].as_i64()?;
    chrono::DateTime::from_timestamp(exp, 0)
}

// ---------------------------------------------------------------------------
// Request building
// ---------------------------------------------------------------------------

/// Build the JSON body for the ChatGPT backend-api conversation endpoint.
pub fn build_request_body(request: &ChatRequest, model: &str) -> Value {
    let parent_id = uuid::Uuid::new_v4().to_string();

    // Convert messages to ChatGPT web format.
    let mut web_messages: Vec<Value> = Vec::new();

    // Collect system prompt.
    let mut system_parts: Vec<String> = Vec::new();
    for msg in &request.messages {
        if msg.role == Role::System {
            system_parts.push(msg.content.clone());
        }
    }

    // If tools are provided, inject tool descriptions into system prompt so
    // the model can return structured tool calls as JSON.
    if !request.tools.is_empty() {
        system_parts.push(build_tool_prompt(&request.tools));
    }

    // Build user/assistant messages (skip system — handled above).
    let mut last_parent_id = parent_id.clone();
    for msg in &request.messages {
        match msg.role {
            Role::System => continue,
            Role::User | Role::Tool => {
                let msg_id = uuid::Uuid::new_v4().to_string();
                let content = if msg.role == Role::Tool {
                    format!("[Tool Result for {}]: {}", msg.tool_call_id.as_deref().unwrap_or("unknown"), msg.content)
                } else {
                    msg.content.clone()
                };
                web_messages.push(json!({
                    "id": msg_id,
                    "author": {"role": "user"},
                    "content": {"content_type": "text", "parts": [content]},
                    "metadata": {}
                }));
                last_parent_id = msg_id;
            }
            Role::Assistant => {
                let msg_id = uuid::Uuid::new_v4().to_string();
                let content = if msg.tool_calls.is_empty() {
                    msg.content.clone()
                } else {
                    // Serialize tool calls as JSON text.
                    let calls: Vec<Value> = msg.tool_calls.iter().map(|tc| {
                        json!({"id": tc.id, "name": tc.name, "arguments": tc.arguments})
                    }).collect();
                    serde_json::to_string(&calls).unwrap_or_default()
                };
                web_messages.push(json!({
                    "id": msg_id,
                    "author": {"role": "assistant"},
                    "content": {"content_type": "text", "parts": [content]},
                    "metadata": {}
                }));
                last_parent_id = msg_id;
            }
        }
    }

    let mut body = json!({
        "action": "next",
        "messages": web_messages,
        "model": model,
        "parent_message_id": last_parent_id,
        "conversation_id": null,
        "timezone_offset_min": 0,
        "history_and_training_disabled": true,
    });

    if !system_parts.is_empty() {
        body["system_message"] = json!({
            "content": {"content_type": "text", "parts": [system_parts.join("\n\n")]},
            "metadata": {}
        });
    }

    body
}

/// Build a text prompt describing available tools so the LLM can return
/// structured tool calls.
fn build_tool_prompt(tools: &[ToolDefinition]) -> String {
    let mut prompt = String::from(
        "You have access to the following tools. When you need to use a tool, \
         respond with EXACTLY one JSON object on its own line in this format:\n\
         {\"tool_call\": {\"id\": \"call_<random>\", \"name\": \"<tool_name>\", \
         \"arguments\": {<args>}}}\n\n\
         Available tools:\n",
    );

    for tool in tools {
        prompt.push_str(&format!(
            "- **{}**: {}\n  Parameters: {}\n",
            tool.name,
            tool.description,
            serde_json::to_string(&tool.input_schema).unwrap_or_default()
        ));
    }

    prompt.push_str(
        "\nIMPORTANT: When calling a tool, output ONLY the JSON object. \
         Do not add any text before or after it.",
    );

    prompt
}

// ---------------------------------------------------------------------------
// Sending requests
// ---------------------------------------------------------------------------

/// Send a conversation request to the ChatGPT backend-api.
pub async fn send_request(
    http: &reqwest::Client,
    auth: &ChatGptWebAuth,
    base_url: &str,
    body: &Value,
) -> Result<reqwest::Response> {
    let url = format!("{base_url}/backend-api/conversation");
    let access_token = auth.get_access_token().await?;
    let device_id = auth.device_id().await;

    let mut headers = HeaderMap::new();
    headers.insert(
        AUTHORIZATION,
        HeaderValue::from_str(&format!("Bearer {access_token}")).map_err(|e| {
            AgentError::LlmRequestFailed {
                reason: format!("invalid authorization header: {e}"),
            }
        })?,
    );
    headers.insert(CONTENT_TYPE, HeaderValue::from_static("application/json"));
    headers.insert(
        "Accept",
        HeaderValue::from_static("text/event-stream"),
    );
    headers.insert(
        "User-Agent",
        HeaderValue::from_static(USER_AGENT),
    );
    headers.insert(
        "Oai-Device-Id",
        HeaderValue::from_str(&device_id).unwrap_or_else(|_| {
            HeaderValue::from_static("unknown")
        }),
    );
    headers.insert(
        "Oai-Language",
        HeaderValue::from_static("en-US"),
    );
    headers.insert(
        "Origin",
        HeaderValue::from_static("https://chatgpt.com"),
    );
    headers.insert(
        "Referer",
        HeaderValue::from_static("https://chatgpt.com/"),
    );

    tracing::debug!(
        url = %url,
        model = %body["model"],
        provider = "chatgpt-web",
        "sending ChatGPT web request"
    );

    http.post(&url)
        .headers(headers)
        .json(body)
        .send()
        .await
        .map_err(|e| AgentError::LlmRequestFailed {
            reason: format!("ChatGPT web request failed: {e}"),
        })
}

// ---------------------------------------------------------------------------
// Streaming
// ---------------------------------------------------------------------------

/// Consume a ChatGPT web SSE stream and aggregate into a final response.
pub async fn consume_stream<F>(
    resp: reqwest::Response,
    on_text: &mut F,
    has_tools: bool,
) -> Result<(LlmResponse, Usage)>
where
    F: FnMut(&str),
{
    let mut accumulator = ChatGptWebStreamAccumulator::new(has_tools);

    let mut byte_stream = resp.bytes_stream();
    let mut line_buffer = String::new();

    while let Some(chunk_result) = byte_stream.next().await {
        let chunk = chunk_result.map_err(|e| AgentError::LlmStreamError {
            reason: format!("ChatGPT web stream read error: {e}"),
        })?;

        let text = std::str::from_utf8(&chunk).map_err(|e| {
            AgentError::LlmStreamError {
                reason: format!("invalid UTF-8 in ChatGPT web stream: {e}"),
            }
        })?;

        line_buffer.push_str(text);

        while let Some(newline_pos) = line_buffer.find('\n') {
            let line = line_buffer[..newline_pos].to_owned();
            line_buffer = line_buffer[newline_pos + 1..].to_owned();

            if let Some(delta_text) = accumulator.feed_line(&line)? {
                on_text(&delta_text);
            }

            if accumulator.is_done() {
                return accumulator.into_response();
            }
        }
    }

    accumulator.into_response()
}

// ---------------------------------------------------------------------------
// Browser proxy (fetch via Chrome to bypass Cloudflare)
// ---------------------------------------------------------------------------

/// Send a ChatGPT conversation request by executing `fetch()` inside Chrome
/// via the browser adapter's `browser_evaluate` tool. This bypasses Cloudflare
/// TLS fingerprinting since the request comes from a real browser.
///
/// Returns the raw SSE text (all `data:` lines concatenated).
pub async fn fetch_via_browser(
    browser: &Arc<dyn crate::runtime::ToolAdapter>,
    auth: &ChatGptWebAuth,
    base_url: &str,
    body: &Value,
) -> Result<String> {
    let url = format!("{base_url}/backend-api/conversation");
    let access_token = auth.get_access_token().await?;
    let device_id = auth.device_id().await;
    let body_json = serde_json::to_string(body).map_err(|e| AgentError::LlmRequestFailed {
        reason: format!("failed to serialize request body: {e}"),
    })?;

    // JavaScript that runs inside Chrome: makes a fetch, reads the SSE stream,
    // and returns all lines joined.
    let js = format!(
        r#"(async () => {{
  try {{
    const resp = await fetch({url}, {{
      method: 'POST',
      headers: {{
        'Authorization': 'Bearer {token}',
        'Content-Type': 'application/json',
        'Accept': 'text/event-stream',
        'Oai-Device-Id': '{device_id}',
        'Oai-Language': 'en-US'
      }},
      body: {body}
    }});
    if (!resp.ok) {{
      const t = await resp.text();
      return JSON.stringify({{error: resp.status + ': ' + t}});
    }}
    const reader = resp.body.getReader();
    const decoder = new TextDecoder();
    let result = '';
    while (true) {{
      const {{done, value}} = await reader.read();
      if (done) break;
      result += decoder.decode(value, {{stream: true}});
    }}
    return result;
  }} catch(e) {{
    return JSON.stringify({{error: e.message}});
  }}
}})()"#,
        url = serde_json::to_string(&url).unwrap_or_default(),
        token = access_token,
        device_id = device_id,
        body = body_json,
    );

    tracing::debug!(provider = "chatgpt-web", "sending request via browser proxy");

    let result = browser
        .execute(
            "browser_evaluate",
            json!({ "expression": js }),
        )
        .await
        .map_err(|e| AgentError::LlmRequestFailed {
            reason: format!("browser evaluate failed: {e}"),
        })?;

    // browser_evaluate returns a JSON string; extract the result.
    tracing::debug!(
        result_len = result.len(),
        result_preview = %if result.len() > 200 { &result[..200] } else { &result },
        "browser evaluate raw result"
    );
    let result_val: Value = serde_json::from_str(&result).unwrap_or(Value::Null);
    let raw = result_val["result"]
        .as_str()
        .or_else(|| result_val["value"].as_str())
        .unwrap_or_else(|| result_val.as_str().unwrap_or(""))
        .to_owned();

    // Check for error response.
    if raw.starts_with(r#"{"error""#) {
        let v: Value = serde_json::from_str(&raw).unwrap_or_default();
        let err = v["error"].as_str().unwrap_or("unknown error");
        return Err(AgentError::LlmRequestFailed {
            reason: format!("ChatGPT web browser fetch failed: {err}"),
        });
    }

    if raw.is_empty() {
        return Err(AgentError::LlmRequestFailed {
            reason: "ChatGPT web browser fetch returned empty response".into(),
        });
    }

    tracing::debug!(
        bytes = raw.len(),
        "ChatGPT web browser fetch completed"
    );

    Ok(raw)
}

/// Parse raw SSE text (from browser proxy) into an LlmResponse.
pub fn parse_sse_text<F>(
    raw: &str,
    on_text: &mut F,
    has_tools: bool,
) -> Result<(LlmResponse, Usage)>
where
    F: FnMut(&str),
{
    let mut accumulator = ChatGptWebStreamAccumulator::new(has_tools);

    for line in raw.lines() {
        if let Some(delta_text) = accumulator.feed_line(line)? {
            on_text(&delta_text);
        }
        if accumulator.is_done() {
            return accumulator.into_response();
        }
    }

    accumulator.into_response()
}

// ---------------------------------------------------------------------------
// Non-streaming chat
// ---------------------------------------------------------------------------

/// Non-streaming chat — sends the request and collects the full response.
pub async fn chat(
    http: &reqwest::Client,
    auth: &ChatGptWebAuth,
    base_url: &str,
    request: &ChatRequest,
    model: &str,
) -> Result<LlmResponse> {
    let body = build_request_body(request, model);
    let resp = send_request(http, auth, base_url, &body).await?;

    let status = resp.status();
    if !status.is_success() {
        let text = resp.text().await.unwrap_or_default();
        return Err(AgentError::LlmRequestFailed {
            reason: format!("ChatGPT web API returned {status}: {text}"),
        });
    }

    // ChatGPT web always streams, so consume the stream to get the response.
    let has_tools = !request.tools.is_empty();
    let (response, _usage) = consume_stream(resp, &mut |_| {}, has_tools).await?;
    Ok(response)
}
