//! GitHub repository operations.

use serde_json::{Value, json};
use tracing::debug;

use crate::error::{AdapterError, Result};

/// Percent-encode a string for use in a URL query parameter.
pub fn url_encode(input: &str) -> String {
    let mut encoded = String::with_capacity(input.len() * 2);
    for byte in input.bytes() {
        match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                encoded.push(byte as char);
            }
            _ => {
                encoded.push('%');
                encoded.push_str(&format!("{byte:02X}"));
            }
        }
    }
    encoded
}

/// List repositories for the authenticated user or an organization.
pub async fn list_repos(
    client: &reqwest::Client,
    base_url: &str,
    token: &str,
    params: &Value,
) -> Result<Value> {
    let page = params.get("page").and_then(|v| v.as_u64()).unwrap_or(1);
    let per_page = params.get("per_page").and_then(|v| v.as_u64()).unwrap_or(30);

    let url = if let Some(org) = params.get("org").and_then(|v| v.as_str()) {
        format!("{base_url}/orgs/{org}/repos?page={page}&per_page={per_page}")
    } else {
        format!("{base_url}/user/repos?page={page}&per_page={per_page}")
    };

    debug!(url = %url, "listing repositories");
    send_get(client, &url, token, "github_list_repos").await
}

/// Get repository details.
pub async fn get_repo(
    client: &reqwest::Client,
    base_url: &str,
    token: &str,
    params: &Value,
) -> Result<Value> {
    let owner = require_str(params, "owner", "github_get_repo")?;
    let repo = require_str(params, "repo", "github_get_repo")?;

    let url = format!("{base_url}/repos/{owner}/{repo}");
    debug!(url = %url, "getting repository details");
    send_get(client, &url, token, "github_get_repo").await
}

/// Search code across GitHub.
pub async fn search_code(
    client: &reqwest::Client,
    base_url: &str,
    token: &str,
    params: &Value,
) -> Result<Value> {
    let query = require_str(params, "query", "github_search_code")?;
    let page = params.get("page").and_then(|v| v.as_u64()).unwrap_or(1);

    let encoded_query = url_encode(query);
    let url = format!("{base_url}/search/code?q={encoded_query}&page={page}");
    debug!(url = %url, "searching code");
    send_get(client, &url, token, "github_search_code").await
}

/// Get file content from a repository.
pub async fn get_file_content(
    client: &reqwest::Client,
    base_url: &str,
    token: &str,
    params: &Value,
) -> Result<Value> {
    let owner = require_str(params, "owner", "github_get_file_content")?;
    let repo = require_str(params, "repo", "github_get_file_content")?;
    let path = require_str(params, "path", "github_get_file_content")?;

    let mut url = format!("{base_url}/repos/{owner}/{repo}/contents/{path}");
    if let Some(git_ref) = params.get("ref").and_then(|v| v.as_str()) {
        url = format!("{url}?ref={git_ref}");
    }

    debug!(url = %url, "getting file content");
    send_get(client, &url, token, "github_get_file_content").await
}

// ---------------------------------------------------------------------------
// Internal helpers
// ---------------------------------------------------------------------------

pub(crate) fn require_str<'a>(params: &'a Value, field: &str, tool: &str) -> Result<&'a str> {
    params
        .get(field)
        .and_then(|v| v.as_str())
        .ok_or_else(|| AdapterError::InvalidParams {
            tool_name: tool.into(),
            reason: format!("missing required string field `{field}`"),
        })
}

pub(crate) async fn send_get(
    client: &reqwest::Client,
    url: &str,
    token: &str,
    tool_name: &str,
) -> Result<Value> {
    let request = client
        .get(url)
        .header("Accept", "application/vnd.github+json")
        .header("Authorization", format!("Bearer {token}"))
        .header("X-GitHub-Api-Version", "2022-11-28");
    send_request(client, request, tool_name).await
}

pub(crate) async fn send_post(
    client: &reqwest::Client,
    url: &str,
    token: &str,
    body: Value,
    tool_name: &str,
) -> Result<Value> {
    let request = client
        .post(url)
        .header("Accept", "application/vnd.github+json")
        .header("Authorization", format!("Bearer {token}"))
        .header("X-GitHub-Api-Version", "2022-11-28")
        .json(&body);
    send_request(client, request, tool_name).await
}

async fn send_request(
    _client: &reqwest::Client,
    request: reqwest::RequestBuilder,
    tool_name: &str,
) -> Result<Value> {
    use tracing::warn;

    let response = request.send().await.map_err(|e| {
        if e.is_timeout() {
            AdapterError::Timeout {
                seconds: 30,
                reason: format!("GitHub API request timed out: {e}"),
            }
        } else {
            AdapterError::ExecutionFailed {
                tool_name: tool_name.to_string(),
                reason: format!("GitHub API request failed: {e}"),
            }
        }
    })?;

    let status = response.status();

    let rate_remaining = response
        .headers()
        .get("x-ratelimit-remaining")
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.parse::<u64>().ok());

    if let Some(remaining) = rate_remaining
        && remaining < 10
    {
        warn!(remaining = remaining, tool = tool_name, "GitHub API rate limit is low");
    }

    let body_text = response.text().await.map_err(|e| AdapterError::ExecutionFailed {
        tool_name: tool_name.to_string(),
        reason: format!("failed to read response body: {e}"),
    })?;

    if !status.is_success() {
        let error_body: Value =
            serde_json::from_str(&body_text).unwrap_or_else(|_| json!({ "message": body_text }));
        return Err(AdapterError::ExecutionFailed {
            tool_name: tool_name.to_string(),
            reason: format!(
                "GitHub API returned {}: {}",
                status.as_u16(),
                error_body
                    .get("message")
                    .and_then(|m| m.as_str())
                    .unwrap_or(&body_text)
            ),
        });
    }

    serde_json::from_str(&body_text).map_err(|e| AdapterError::ExecutionFailed {
        tool_name: tool_name.to_string(),
        reason: format!("failed to parse GitHub API response as JSON: {e}"),
    })
}
