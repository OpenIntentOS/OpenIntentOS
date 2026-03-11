//! GitHub REST API v3 adapter for OpenIntentOS.
//!
//! Provides tools for interacting with GitHub repositories, issues, pull
//! requests, code search, and file content retrieval.  Supports both
//! github.com and GitHub Enterprise via configurable base URL.

pub mod issues;
pub mod repos;
pub mod tools;
pub mod types;

use async_trait::async_trait;
use serde_json::Value;
use tracing::{info, warn};

use crate::error::{AdapterError, Result};
use crate::proxy;
use crate::traits::{Adapter, AdapterType, AuthRequirement, HealthStatus, ToolDefinition};

use repos::{get_file_content, get_repo, list_repos, search_code};
use issues::{
    create_issue, create_pull_request, get_issue, get_pull_request, list_issues,
    list_pull_requests,
};

/// Default GitHub API base URL.
const DEFAULT_BASE_URL: &str = "https://api.github.com";

/// GitHub REST API v3 adapter.
pub struct GitHubAdapter {
    id: String,
    connected: bool,
    token: Option<String>,
    base_url: String,
    client: reqwest::Client,
}

impl GitHubAdapter {
    /// Create a new GitHub adapter with the default API URL and no token.
    pub fn new(id: &str) -> Self {
        let client = proxy::build_client(std::time::Duration::from_secs(30))
            .user_agent("OpenIntentOS/0.1")
            .build()
            .unwrap_or_default();

        Self {
            id: id.to_string(),
            connected: false,
            token: None,
            base_url: DEFAULT_BASE_URL.to_string(),
            client,
        }
    }

    /// Create a new GitHub adapter with a pre-configured token.
    pub fn with_token(id: &str, token: &str) -> Self {
        let mut adapter = Self::new(id);
        adapter.token = Some(token.to_string());
        adapter
    }

    /// Create a new GitHub adapter for a GitHub Enterprise instance.
    pub fn with_base_url(id: &str, base_url: &str) -> Self {
        let mut adapter = Self::new(id);
        adapter.base_url = base_url.trim_end_matches('/').to_string();
        adapter
    }

    /// Resolve the token to use for a request.
    pub fn resolve_token(&self, params: &Value) -> Result<String> {
        if let Some(per_call) = params.get("token").and_then(|v| v.as_str())
            && !per_call.is_empty()
        {
            return Ok(per_call.to_string());
        }
        self.token
            .clone()
            .ok_or_else(|| AdapterError::AuthRequired {
                adapter_id: self.id.clone(),
                provider: "github".to_string(),
            })
    }
}

#[async_trait]
impl Adapter for GitHubAdapter {
    fn id(&self) -> &str {
        &self.id
    }

    fn adapter_type(&self) -> AdapterType {
        AdapterType::DevTools
    }

    async fn connect(&mut self) -> Result<()> {
        if let Some(ref token) = self.token {
            let url = format!("{}/user", self.base_url);
            let response = self
                .client
                .get(&url)
                .header("Accept", "application/vnd.github+json")
                .header("Authorization", format!("Bearer {token}"))
                .header("X-GitHub-Api-Version", "2022-11-28")
                .send()
                .await
                .map_err(|e| AdapterError::ExecutionFailed {
                    tool_name: "connect".into(),
                    reason: format!("failed to verify GitHub token: {e}"),
                })?;

            if !response.status().is_success() {
                return Err(AdapterError::AuthRequired {
                    adapter_id: self.id.clone(),
                    provider: "github".into(),
                });
            }

            let user: serde_json::Value = response
                .json()
                .await
                .map_err(|e| AdapterError::ExecutionFailed {
                    tool_name: "connect".into(),
                    reason: format!("failed to parse user response: {e}"),
                })?;

            info!(
                id = %self.id,
                user = %user.get("login").and_then(|v| v.as_str()).unwrap_or("unknown"),
                "GitHub adapter connected and authenticated"
            );
        } else {
            info!(id = %self.id, "GitHub adapter connected (no token configured)");
        }

        self.connected = true;
        Ok(())
    }

    async fn disconnect(&mut self) -> Result<()> {
        info!(id = %self.id, "GitHub adapter disconnected");
        self.connected = false;
        Ok(())
    }

    async fn health_check(&self) -> Result<HealthStatus> {
        if !self.connected {
            return Ok(HealthStatus::Unhealthy);
        }

        let token = match &self.token {
            Some(t) => t.clone(),
            None => return Ok(HealthStatus::Degraded),
        };

        let url = format!("{}/rate_limit", self.base_url);
        let response = self
            .client
            .get(&url)
            .header("Accept", "application/vnd.github+json")
            .header("Authorization", format!("Bearer {token}"))
            .header("X-GitHub-Api-Version", "2022-11-28")
            .send()
            .await
            .map_err(|e| AdapterError::ExecutionFailed {
                tool_name: "health_check".into(),
                reason: format!("rate limit check failed: {e}"),
            })?;

        if !response.status().is_success() {
            return Ok(HealthStatus::Degraded);
        }

        let body: serde_json::Value = response
            .json()
            .await
            .map_err(|e| AdapterError::ExecutionFailed {
                tool_name: "health_check".into(),
                reason: format!("failed to parse rate limit response: {e}"),
            })?;

        let remaining = body
            .pointer("/resources/core/remaining")
            .and_then(|v| v.as_u64())
            .unwrap_or(0);

        if remaining > 100 {
            Ok(HealthStatus::Healthy)
        } else if remaining > 0 {
            warn!(remaining = remaining, "GitHub API rate limit is low");
            Ok(HealthStatus::Degraded)
        } else {
            warn!("GitHub API rate limit exhausted");
            Ok(HealthStatus::Unhealthy)
        }
    }

    fn tools(&self) -> Vec<ToolDefinition> {
        tools::build_tool_definitions()
    }

    async fn execute_tool(&self, name: &str, params: Value) -> Result<Value> {
        if !self.connected {
            return Err(AdapterError::ExecutionFailed {
                tool_name: name.to_string(),
                reason: format!("adapter `{}` is not connected", self.id),
            });
        }

        let token = self.resolve_token(&params)?;
        let base = &self.base_url;
        let client = &self.client;

        match name {
            "github_list_repos" => list_repos(client, base, &token, &params).await,
            "github_get_repo" => get_repo(client, base, &token, &params).await,
            "github_list_issues" => list_issues(client, base, &token, &params).await,
            "github_create_issue" => create_issue(client, base, &token, &params).await,
            "github_get_issue" => get_issue(client, base, &token, &params).await,
            "github_list_pull_requests" => list_pull_requests(client, base, &token, &params).await,
            "github_get_pull_request" => get_pull_request(client, base, &token, &params).await,
            "github_create_pull_request" => {
                create_pull_request(client, base, &token, &params).await
            }
            "github_search_code" => search_code(client, base, &token, &params).await,
            "github_get_file_content" => get_file_content(client, base, &token, &params).await,
            _ => Err(AdapterError::ToolNotFound {
                adapter_id: self.id.clone(),
                tool_name: name.to_string(),
            }),
        }
    }

    fn required_auth(&self) -> Option<AuthRequirement> {
        Some(AuthRequirement {
            provider: "github".into(),
            scopes: vec!["repo".into(), "read:org".into()],
        })
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn new_creates_adapter_with_defaults() {
        let adapter = GitHubAdapter::new("gh-test");
        assert_eq!(adapter.id, "gh-test");
        assert!(!adapter.connected);
        assert!(adapter.token.is_none());
        assert_eq!(adapter.base_url, DEFAULT_BASE_URL);
    }

    #[test]
    fn with_token_sets_token() {
        let adapter = GitHubAdapter::with_token("gh-test", "ghp_abc123");
        assert_eq!(adapter.id, "gh-test");
        assert_eq!(adapter.token.as_deref(), Some("ghp_abc123"));
        assert_eq!(adapter.base_url, DEFAULT_BASE_URL);
    }

    #[test]
    fn with_base_url_sets_custom_url() {
        let adapter = GitHubAdapter::with_base_url("gh-ent", "https://github.example.com/api/v3/");
        assert_eq!(adapter.base_url, "https://github.example.com/api/v3");
        assert!(adapter.token.is_none());
    }

    #[test]
    fn adapter_id_returns_id() {
        let adapter = GitHubAdapter::new("my-gh");
        assert_eq!(adapter.id(), "my-gh");
    }

    #[test]
    fn adapter_type_is_devtools() {
        let adapter = GitHubAdapter::new("gh");
        assert_eq!(adapter.adapter_type(), AdapterType::DevTools);
    }

    #[test]
    fn required_auth_returns_github_scopes() {
        let adapter = GitHubAdapter::new("gh");
        let auth = adapter.required_auth().expect("should require auth");
        assert_eq!(auth.provider, "github");
        assert!(auth.scopes.contains(&"repo".to_string()));
        assert!(auth.scopes.contains(&"read:org".to_string()));
    }

    #[test]
    fn tools_returns_exactly_ten() {
        let adapter = GitHubAdapter::new("gh");
        let tools = adapter.tools();
        assert_eq!(tools.len(), 10);
    }

    #[test]
    fn tools_have_expected_names() {
        let adapter = GitHubAdapter::new("gh");
        let names: Vec<String> = adapter.tools().iter().map(|t| t.name.clone()).collect();
        let expected = vec![
            "github_list_repos",
            "github_get_repo",
            "github_list_issues",
            "github_create_issue",
            "github_get_issue",
            "github_list_pull_requests",
            "github_get_pull_request",
            "github_create_pull_request",
            "github_search_code",
            "github_get_file_content",
        ];
        assert_eq!(names, expected);
    }

    #[test]
    fn tool_parameters_have_required_fields() {
        let adapter = GitHubAdapter::new("gh");
        let tools = adapter.tools();

        let get_repo = tools.iter().find(|t| t.name == "github_get_repo").unwrap();
        let required = get_repo.parameters["required"].as_array().expect("required should be an array");
        assert!(required.contains(&json!("owner")));
        assert!(required.contains(&json!("repo")));

        let create_issue = tools.iter().find(|t| t.name == "github_create_issue").unwrap();
        let required = create_issue.parameters["required"].as_array().expect("required should be an array");
        assert!(required.contains(&json!("owner")));
        assert!(required.contains(&json!("repo")));
        assert!(required.contains(&json!("title")));

        let create_pr = tools.iter().find(|t| t.name == "github_create_pull_request").unwrap();
        let required = create_pr.parameters["required"].as_array().expect("required should be an array");
        assert_eq!(required.len(), 5);
        assert!(required.contains(&json!("head")));
        assert!(required.contains(&json!("base")));

        let search = tools.iter().find(|t| t.name == "github_search_code").unwrap();
        let required = search.parameters["required"].as_array().expect("required should be an array");
        assert!(required.contains(&json!("query")));
    }

    #[tokio::test]
    async fn health_check_returns_unhealthy_when_disconnected() {
        let adapter = GitHubAdapter::new("gh");
        let status = adapter.health_check().await.unwrap();
        assert_eq!(status, HealthStatus::Unhealthy);
    }

    #[test]
    fn resolve_token_uses_configured_token() {
        let adapter = GitHubAdapter::with_token("gh", "configured-token");
        let token = adapter.resolve_token(&json!({})).unwrap();
        assert_eq!(token, "configured-token");
    }

    #[test]
    fn resolve_token_per_call_overrides_configured() {
        let adapter = GitHubAdapter::with_token("gh", "configured-token");
        let token = adapter.resolve_token(&json!({"token": "per-call-token"})).unwrap();
        assert_eq!(token, "per-call-token");
    }

    #[test]
    fn resolve_token_fails_when_none_available() {
        let adapter = GitHubAdapter::new("gh");
        let result = adapter.resolve_token(&json!({}));
        assert!(result.is_err());
    }

    #[test]
    fn resolve_token_ignores_empty_per_call_token() {
        let adapter = GitHubAdapter::with_token("gh", "configured-token");
        let token = adapter.resolve_token(&json!({"token": ""})).unwrap();
        assert_eq!(token, "configured-token");
    }

    #[tokio::test]
    async fn execute_tool_rejects_when_not_connected() {
        let adapter = GitHubAdapter::with_token("gh", "some-token");
        let result = adapter.execute_tool("github_list_repos", json!({})).await;
        assert!(result.is_err());
        let err = result.unwrap_err();
        assert!(err.to_string().contains("not connected"), "error should mention not connected: {err}");
    }

    #[tokio::test]
    async fn execute_tool_rejects_unknown_tool() {
        let mut adapter = GitHubAdapter::with_token("gh", "some-token");
        adapter.connected = true;
        let result = adapter.execute_tool("nonexistent_tool", json!({})).await;
        assert!(result.is_err());
        let err = result.unwrap_err();
        assert!(err.to_string().contains("tool not found"), "error should mention tool not found: {err}");
    }

    #[tokio::test]
    async fn get_repo_rejects_missing_owner() {
        let mut adapter = GitHubAdapter::with_token("gh", "token");
        adapter.connected = true;
        let result = adapter.execute_tool("github_get_repo", json!({"repo": "test"})).await;
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("owner"));
    }

    #[tokio::test]
    async fn get_repo_rejects_missing_repo() {
        let mut adapter = GitHubAdapter::with_token("gh", "token");
        adapter.connected = true;
        let result = adapter.execute_tool("github_get_repo", json!({"owner": "test"})).await;
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("repo"));
    }

    #[tokio::test]
    async fn create_issue_rejects_missing_title() {
        let mut adapter = GitHubAdapter::with_token("gh", "token");
        adapter.connected = true;
        let result = adapter
            .execute_tool("github_create_issue", json!({"owner": "test", "repo": "test"}))
            .await;
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("title"));
    }

    #[tokio::test]
    async fn search_code_rejects_missing_query() {
        let mut adapter = GitHubAdapter::with_token("gh", "token");
        adapter.connected = true;
        let result = adapter.execute_tool("github_search_code", json!({})).await;
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("query"));
    }

    #[tokio::test]
    async fn get_file_content_rejects_missing_path() {
        let mut adapter = GitHubAdapter::with_token("gh", "token");
        adapter.connected = true;
        let result = adapter
            .execute_tool("github_get_file_content", json!({"owner": "test", "repo": "test"}))
            .await;
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("path"));
    }

    #[tokio::test]
    async fn create_pr_rejects_missing_head() {
        let mut adapter = GitHubAdapter::with_token("gh", "token");
        adapter.connected = true;
        let result = adapter
            .execute_tool(
                "github_create_pull_request",
                json!({"owner": "o", "repo": "r", "title": "t", "base": "main"}),
            )
            .await;
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("head"));
    }

    #[tokio::test]
    async fn connect_succeeds_without_token() {
        let mut adapter = GitHubAdapter::new("gh");
        let result = adapter.connect().await;
        assert!(result.is_ok());
        assert!(adapter.connected);
    }

    #[tokio::test]
    async fn disconnect_sets_connected_false() {
        let mut adapter = GitHubAdapter::new("gh");
        adapter.connected = true;
        adapter.disconnect().await.unwrap();
        assert!(!adapter.connected);
    }
}
