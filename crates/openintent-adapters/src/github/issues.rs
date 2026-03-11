//! GitHub issue and pull request operations.

use serde_json::{Value, json};
use tracing::debug;

use crate::error::Result;

use super::repos::{require_str, send_get, send_post};

/// List issues for a repository.
pub async fn list_issues(
    client: &reqwest::Client,
    base_url: &str,
    token: &str,
    params: &Value,
) -> Result<Value> {
    let owner = require_str(params, "owner", "github_list_issues")?;
    let repo = require_str(params, "repo", "github_list_issues")?;
    let state = params.get("state").and_then(|v| v.as_str()).unwrap_or("open");
    let page = params.get("page").and_then(|v| v.as_u64()).unwrap_or(1);

    let url = format!("{base_url}/repos/{owner}/{repo}/issues?state={state}&page={page}");
    debug!(url = %url, "listing issues");
    send_get(client, &url, token, "github_list_issues").await
}

/// Create an issue in a repository.
pub async fn create_issue(
    client: &reqwest::Client,
    base_url: &str,
    token: &str,
    params: &Value,
) -> Result<Value> {
    let owner = require_str(params, "owner", "github_create_issue")?;
    let repo = require_str(params, "repo", "github_create_issue")?;
    let title = require_str(params, "title", "github_create_issue")?;

    let mut body_json = json!({ "title": title });
    if let Some(body) = params.get("body").and_then(|v| v.as_str()) {
        body_json["body"] = json!(body);
    }
    if let Some(labels) = params.get("labels").and_then(|v| v.as_array()) {
        body_json["labels"] = json!(labels);
    }

    let url = format!("{base_url}/repos/{owner}/{repo}/issues");
    debug!(url = %url, "creating issue");
    send_post(client, &url, token, body_json, "github_create_issue").await
}

/// Get a specific issue by number.
pub async fn get_issue(
    client: &reqwest::Client,
    base_url: &str,
    token: &str,
    params: &Value,
) -> Result<Value> {
    let owner = require_str(params, "owner", "github_get_issue")?;
    let repo = require_str(params, "repo", "github_get_issue")?;
    let number = params
        .get("number")
        .and_then(|v| v.as_u64())
        .ok_or_else(|| crate::error::AdapterError::InvalidParams {
            tool_name: "github_get_issue".into(),
            reason: "missing required integer field `number`".into(),
        })?;

    let url = format!("{base_url}/repos/{owner}/{repo}/issues/{number}");
    debug!(url = %url, "getting issue");
    send_get(client, &url, token, "github_get_issue").await
}

/// List pull requests for a repository.
pub async fn list_pull_requests(
    client: &reqwest::Client,
    base_url: &str,
    token: &str,
    params: &Value,
) -> Result<Value> {
    let owner = require_str(params, "owner", "github_list_pull_requests")?;
    let repo = require_str(params, "repo", "github_list_pull_requests")?;
    let state = params.get("state").and_then(|v| v.as_str()).unwrap_or("open");
    let page = params.get("page").and_then(|v| v.as_u64()).unwrap_or(1);

    let url = format!("{base_url}/repos/{owner}/{repo}/pulls?state={state}&page={page}");
    debug!(url = %url, "listing pull requests");
    send_get(client, &url, token, "github_list_pull_requests").await
}

/// Get a specific pull request by number.
pub async fn get_pull_request(
    client: &reqwest::Client,
    base_url: &str,
    token: &str,
    params: &Value,
) -> Result<Value> {
    let owner = require_str(params, "owner", "github_get_pull_request")?;
    let repo = require_str(params, "repo", "github_get_pull_request")?;
    let number = params
        .get("number")
        .and_then(|v| v.as_u64())
        .ok_or_else(|| crate::error::AdapterError::InvalidParams {
            tool_name: "github_get_pull_request".into(),
            reason: "missing required integer field `number`".into(),
        })?;

    let url = format!("{base_url}/repos/{owner}/{repo}/pulls/{number}");
    debug!(url = %url, "getting pull request");
    send_get(client, &url, token, "github_get_pull_request").await
}

/// Create a pull request.
pub async fn create_pull_request(
    client: &reqwest::Client,
    base_url: &str,
    token: &str,
    params: &Value,
) -> Result<Value> {
    let owner = require_str(params, "owner", "github_create_pull_request")?;
    let repo = require_str(params, "repo", "github_create_pull_request")?;
    let title = require_str(params, "title", "github_create_pull_request")?;
    let head = require_str(params, "head", "github_create_pull_request")?;
    let base = require_str(params, "base", "github_create_pull_request")?;

    let mut body_json = json!({ "title": title, "head": head, "base": base });
    if let Some(body) = params.get("body").and_then(|v| v.as_str()) {
        body_json["body"] = json!(body);
    }

    let url = format!("{base_url}/repos/{owner}/{repo}/pulls");
    debug!(url = %url, "creating pull request");
    send_post(client, &url, token, body_json, "github_create_pull_request").await
}
