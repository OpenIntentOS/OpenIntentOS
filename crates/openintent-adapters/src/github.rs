//! GitHub REST API v3 adapter for OpenIntentOS.
//!
//! Provides tools for interacting with GitHub repositories, issues, pull
//! requests, code search, and file content retrieval.  Supports both
//! github.com and GitHub Enterprise via configurable base URL.

use async_trait::async_trait;
use serde_json::{Value, json};
use tracing::{debug, info, warn};

use crate::error::{AdapterError, Result};
use crate::traits::{Adapter, AdapterType, AuthRequirement, HealthStatus, ToolDefinition};

/// Default GitHub API base URL.
const DEFAULT_BASE_URL: &str = "https://api.github.com";

/// GitHub REST API v3 adapter.
///
/// Provides tools for repositories, issues, pull requests, code search, and
/// file content retrieval.  Tokens can be configured at construction time or
/// overridden per-call via a `"token"` field in the tool parameters.
pub struct GitHubAdapter {
    /// Unique identifier for this adapter instance.
    id: String,
    /// Whether the adapter has been connected.
    connected: bool,
    /// GitHub personal access token or OAuth token.
    token: Option<String>,
    /// Base URL for the GitHub API (default: `https://api.github.com`).
    base_url: String,
    /// HTTP client for making requests.
    client: reqwest::Client,
}

impl GitHubAdapter {
    /// Create a new GitHub adapter with the default API URL and no token.
    pub fn new(id: &str) -> Self {
        let client = reqwest::Client::builder()
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

    // -----------------------------------------------------------------------
    // Token resolution
    // -----------------------------------------------------------------------

    /// Resolve the token to use for a request.  Per-call token overrides the
    /// configured token.
    fn resolve_token(&self, params: &Value) -> Result<String> {
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

    // -----------------------------------------------------------------------
    // HTTP helpers
    // -----------------------------------------------------------------------

    /// Build a GET request with standard GitHub headers.
    fn get_request(&self, url: &str, token: &str) -> reqwest::RequestBuilder {
        self.client
            .get(url)
            .header("Accept", "application/vnd.github+json")
            .header("Authorization", format!("Bearer {token}"))
            .header("X-GitHub-Api-Version", "2022-11-28")
    }

    /// Build a POST request with standard GitHub headers.
    fn post_request(&self, url: &str, token: &str) -> reqwest::RequestBuilder {
        self.client
            .post(url)
            .header("Accept", "application/vnd.github+json")
            .header("Authorization", format!("Bearer {token}"))
            .header("X-GitHub-Api-Version", "2022-11-28")
    }

    /// Send a request and parse the JSON response, handling rate limits.
    async fn send_request(
        &self,
        request: reqwest::RequestBuilder,
        tool_name: &str,
    ) -> Result<Value> {
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

        // Check rate limit headers.
        let rate_remaining = response
            .headers()
            .get("x-ratelimit-remaining")
            .and_then(|v| v.to_str().ok())
            .and_then(|v| v.parse::<u64>().ok());

        if let Some(remaining) = rate_remaining
            && remaining < 10
        {
            warn!(
                remaining = remaining,
                tool = tool_name,
                "GitHub API rate limit is low"
            );
        }

        let body_text = response
            .text()
            .await
            .map_err(|e| AdapterError::ExecutionFailed {
                tool_name: tool_name.to_string(),
                reason: format!("failed to read response body: {e}"),
            })?;

        if !status.is_success() {
            let error_body: Value = serde_json::from_str(&body_text)
                .unwrap_or_else(|_| json!({ "message": body_text }));
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

    // -----------------------------------------------------------------------
    // URL construction helpers
    // -----------------------------------------------------------------------

    /// Build a full API URL from a path segment.
    fn api_url(&self, path: &str) -> String {
        format!("{}{}", self.base_url, path)
    }

    // -----------------------------------------------------------------------
    // Tool implementations
    // -----------------------------------------------------------------------

    /// List repositories for the authenticated user or an organization.
    async fn tool_list_repos(&self, params: Value) -> Result<Value> {
        let token = self.resolve_token(&params)?;
        let page = params.get("page").and_then(|v| v.as_u64()).unwrap_or(1);
        let per_page = params
            .get("per_page")
            .and_then(|v| v.as_u64())
            .unwrap_or(30);

        let url = if let Some(org) = params.get("org").and_then(|v| v.as_str()) {
            self.api_url(&format!(
                "/orgs/{org}/repos?page={page}&per_page={per_page}"
            ))
        } else {
            self.api_url(&format!("/user/repos?page={page}&per_page={per_page}"))
        };

        debug!(url = %url, "listing repositories");
        let request = self.get_request(&url, &token);
        self.send_request(request, "github_list_repos").await
    }

    /// Get repository details.
    async fn tool_get_repo(&self, params: Value) -> Result<Value> {
        let token = self.resolve_token(&params)?;
        let owner = params
            .get("owner")
            .and_then(|v| v.as_str())
            .ok_or_else(|| AdapterError::InvalidParams {
                tool_name: "github_get_repo".into(),
                reason: "missing required string field `owner`".into(),
            })?;
        let repo = params.get("repo").and_then(|v| v.as_str()).ok_or_else(|| {
            AdapterError::InvalidParams {
                tool_name: "github_get_repo".into(),
                reason: "missing required string field `repo`".into(),
            }
        })?;

        let url = self.api_url(&format!("/repos/{owner}/{repo}"));
        debug!(url = %url, "getting repository details");
        let request = self.get_request(&url, &token);
        self.send_request(request, "github_get_repo").await
    }

    /// List issues for a repository.
    async fn tool_list_issues(&self, params: Value) -> Result<Value> {
        let token = self.resolve_token(&params)?;
        let owner = params
            .get("owner")
            .and_then(|v| v.as_str())
            .ok_or_else(|| AdapterError::InvalidParams {
                tool_name: "github_list_issues".into(),
                reason: "missing required string field `owner`".into(),
            })?;
        let repo = params.get("repo").and_then(|v| v.as_str()).ok_or_else(|| {
            AdapterError::InvalidParams {
                tool_name: "github_list_issues".into(),
                reason: "missing required string field `repo`".into(),
            }
        })?;

        let state = params
            .get("state")
            .and_then(|v| v.as_str())
            .unwrap_or("open");
        let page = params.get("page").and_then(|v| v.as_u64()).unwrap_or(1);

        let url = self.api_url(&format!(
            "/repos/{owner}/{repo}/issues?state={state}&page={page}"
        ));
        debug!(url = %url, "listing issues");
        let request = self.get_request(&url, &token);
        self.send_request(request, "github_list_issues").await
    }

    /// Create an issue in a repository.
    async fn tool_create_issue(&self, params: Value) -> Result<Value> {
        let token = self.resolve_token(&params)?;
        let owner = params
            .get("owner")
            .and_then(|v| v.as_str())
            .ok_or_else(|| AdapterError::InvalidParams {
                tool_name: "github_create_issue".into(),
                reason: "missing required string field `owner`".into(),
            })?;
        let repo = params.get("repo").and_then(|v| v.as_str()).ok_or_else(|| {
            AdapterError::InvalidParams {
                tool_name: "github_create_issue".into(),
                reason: "missing required string field `repo`".into(),
            }
        })?;
        let title = params
            .get("title")
            .and_then(|v| v.as_str())
            .ok_or_else(|| AdapterError::InvalidParams {
                tool_name: "github_create_issue".into(),
                reason: "missing required string field `title`".into(),
            })?;

        let mut body_json = json!({ "title": title });
        if let Some(body) = params.get("body").and_then(|v| v.as_str()) {
            body_json["body"] = json!(body);
        }
        if let Some(labels) = params.get("labels").and_then(|v| v.as_array()) {
            body_json["labels"] = json!(labels);
        }

        let url = self.api_url(&format!("/repos/{owner}/{repo}/issues"));
        debug!(url = %url, "creating issue");
        let request = self.post_request(&url, &token).json(&body_json);
        self.send_request(request, "github_create_issue").await
    }

    /// Get a specific issue by number.
    async fn tool_get_issue(&self, params: Value) -> Result<Value> {
        let token = self.resolve_token(&params)?;
        let owner = params
            .get("owner")
            .and_then(|v| v.as_str())
            .ok_or_else(|| AdapterError::InvalidParams {
                tool_name: "github_get_issue".into(),
                reason: "missing required string field `owner`".into(),
            })?;
        let repo = params.get("repo").and_then(|v| v.as_str()).ok_or_else(|| {
            AdapterError::InvalidParams {
                tool_name: "github_get_issue".into(),
                reason: "missing required string field `repo`".into(),
            }
        })?;
        let number = params
            .get("number")
            .and_then(|v| v.as_u64())
            .ok_or_else(|| AdapterError::InvalidParams {
                tool_name: "github_get_issue".into(),
                reason: "missing required integer field `number`".into(),
            })?;

        let url = self.api_url(&format!("/repos/{owner}/{repo}/issues/{number}"));
        debug!(url = %url, "getting issue");
        let request = self.get_request(&url, &token);
        self.send_request(request, "github_get_issue").await
    }

    /// List pull requests for a repository.
    async fn tool_list_pull_requests(&self, params: Value) -> Result<Value> {
        let token = self.resolve_token(&params)?;
        let owner = params
            .get("owner")
            .and_then(|v| v.as_str())
            .ok_or_else(|| AdapterError::InvalidParams {
                tool_name: "github_list_pull_requests".into(),
                reason: "missing required string field `owner`".into(),
            })?;
        let repo = params.get("repo").and_then(|v| v.as_str()).ok_or_else(|| {
            AdapterError::InvalidParams {
                tool_name: "github_list_pull_requests".into(),
                reason: "missing required string field `repo`".into(),
            }
        })?;

        let state = params
            .get("state")
            .and_then(|v| v.as_str())
            .unwrap_or("open");
        let page = params.get("page").and_then(|v| v.as_u64()).unwrap_or(1);

        let url = self.api_url(&format!(
            "/repos/{owner}/{repo}/pulls?state={state}&page={page}"
        ));
        debug!(url = %url, "listing pull requests");
        let request = self.get_request(&url, &token);
        self.send_request(request, "github_list_pull_requests")
            .await
    }

    /// Get a specific pull request by number.
    async fn tool_get_pull_request(&self, params: Value) -> Result<Value> {
        let token = self.resolve_token(&params)?;
        let owner = params
            .get("owner")
            .and_then(|v| v.as_str())
            .ok_or_else(|| AdapterError::InvalidParams {
                tool_name: "github_get_pull_request".into(),
                reason: "missing required string field `owner`".into(),
            })?;
        let repo = params.get("repo").and_then(|v| v.as_str()).ok_or_else(|| {
            AdapterError::InvalidParams {
                tool_name: "github_get_pull_request".into(),
                reason: "missing required string field `repo`".into(),
            }
        })?;
        let number = params
            .get("number")
            .and_then(|v| v.as_u64())
            .ok_or_else(|| AdapterError::InvalidParams {
                tool_name: "github_get_pull_request".into(),
                reason: "missing required integer field `number`".into(),
            })?;

        let url = self.api_url(&format!("/repos/{owner}/{repo}/pulls/{number}"));
        debug!(url = %url, "getting pull request");
        let request = self.get_request(&url, &token);
        self.send_request(request, "github_get_pull_request").await
    }

    /// Create a pull request.
    async fn tool_create_pull_request(&self, params: Value) -> Result<Value> {
        let token = self.resolve_token(&params)?;
        let owner = params
            .get("owner")
            .and_then(|v| v.as_str())
            .ok_or_else(|| AdapterError::InvalidParams {
                tool_name: "github_create_pull_request".into(),
                reason: "missing required string field `owner`".into(),
            })?;
        let repo = params.get("repo").and_then(|v| v.as_str()).ok_or_else(|| {
            AdapterError::InvalidParams {
                tool_name: "github_create_pull_request".into(),
                reason: "missing required string field `repo`".into(),
            }
        })?;
        let title = params
            .get("title")
            .and_then(|v| v.as_str())
            .ok_or_else(|| AdapterError::InvalidParams {
                tool_name: "github_create_pull_request".into(),
                reason: "missing required string field `title`".into(),
            })?;
        let head = params.get("head").and_then(|v| v.as_str()).ok_or_else(|| {
            AdapterError::InvalidParams {
                tool_name: "github_create_pull_request".into(),
                reason: "missing required string field `head`".into(),
            }
        })?;
        let base = params.get("base").and_then(|v| v.as_str()).ok_or_else(|| {
            AdapterError::InvalidParams {
                tool_name: "github_create_pull_request".into(),
                reason: "missing required string field `base`".into(),
            }
        })?;

        let mut body_json = json!({
            "title": title,
            "head": head,
            "base": base,
        });
        if let Some(body) = params.get("body").and_then(|v| v.as_str()) {
            body_json["body"] = json!(body);
        }

        let url = self.api_url(&format!("/repos/{owner}/{repo}/pulls"));
        debug!(url = %url, "creating pull request");
        let request = self.post_request(&url, &token).json(&body_json);
        self.send_request(request, "github_create_pull_request")
            .await
    }

    /// Search code across GitHub.
    async fn tool_search_code(&self, params: Value) -> Result<Value> {
        let token = self.resolve_token(&params)?;
        let query = params
            .get("query")
            .and_then(|v| v.as_str())
            .ok_or_else(|| AdapterError::InvalidParams {
                tool_name: "github_search_code".into(),
                reason: "missing required string field `query`".into(),
            })?;
        let page = params.get("page").and_then(|v| v.as_u64()).unwrap_or(1);

        // URL-encode the query.
        let encoded_query = urlencoding::encode(query);
        let url = self.api_url(&format!("/search/code?q={encoded_query}&page={page}"));
        debug!(url = %url, "searching code");
        let request = self.get_request(&url, &token);
        self.send_request(request, "github_search_code").await
    }

    /// Get file content from a repository.
    async fn tool_get_file_content(&self, params: Value) -> Result<Value> {
        let token = self.resolve_token(&params)?;
        let owner = params
            .get("owner")
            .and_then(|v| v.as_str())
            .ok_or_else(|| AdapterError::InvalidParams {
                tool_name: "github_get_file_content".into(),
                reason: "missing required string field `owner`".into(),
            })?;
        let repo = params.get("repo").and_then(|v| v.as_str()).ok_or_else(|| {
            AdapterError::InvalidParams {
                tool_name: "github_get_file_content".into(),
                reason: "missing required string field `repo`".into(),
            }
        })?;
        let path = params.get("path").and_then(|v| v.as_str()).ok_or_else(|| {
            AdapterError::InvalidParams {
                tool_name: "github_get_file_content".into(),
                reason: "missing required string field `path`".into(),
            }
        })?;

        let mut url = self.api_url(&format!("/repos/{owner}/{repo}/contents/{path}"));
        if let Some(git_ref) = params.get("ref").and_then(|v| v.as_str()) {
            url = format!("{url}?ref={git_ref}");
        }

        debug!(url = %url, "getting file content");
        let request = self.get_request(&url, &token);
        self.send_request(request, "github_get_file_content").await
    }
}

// ---------------------------------------------------------------------------
// Tool definitions
// ---------------------------------------------------------------------------

/// Build the list of tool definitions for the GitHub adapter.
fn build_tool_definitions() -> Vec<ToolDefinition> {
    vec![
        ToolDefinition {
            name: "github_list_repos".into(),
            description: "List repositories for the authenticated user or an organization".into(),
            parameters: json!({
                "type": "object",
                "properties": {
                    "org": {
                        "type": "string",
                        "description": "Optional organization name. If omitted, lists repos for the authenticated user."
                    },
                    "page": {
                        "type": "integer",
                        "description": "Page number for pagination (default: 1)"
                    },
                    "per_page": {
                        "type": "integer",
                        "description": "Number of results per page (default: 30, max: 100)"
                    },
                    "token": {
                        "type": "string",
                        "description": "Optional per-call GitHub token (overrides configured token)"
                    }
                },
                "required": []
            }),
        },
        ToolDefinition {
            name: "github_get_repo".into(),
            description: "Get detailed information about a specific repository".into(),
            parameters: json!({
                "type": "object",
                "properties": {
                    "owner": {
                        "type": "string",
                        "description": "Repository owner (user or organization)"
                    },
                    "repo": {
                        "type": "string",
                        "description": "Repository name"
                    },
                    "token": {
                        "type": "string",
                        "description": "Optional per-call GitHub token (overrides configured token)"
                    }
                },
                "required": ["owner", "repo"]
            }),
        },
        ToolDefinition {
            name: "github_list_issues".into(),
            description: "List issues for a repository".into(),
            parameters: json!({
                "type": "object",
                "properties": {
                    "owner": {
                        "type": "string",
                        "description": "Repository owner (user or organization)"
                    },
                    "repo": {
                        "type": "string",
                        "description": "Repository name"
                    },
                    "state": {
                        "type": "string",
                        "description": "Issue state filter: open, closed, or all (default: open)",
                        "enum": ["open", "closed", "all"]
                    },
                    "page": {
                        "type": "integer",
                        "description": "Page number for pagination (default: 1)"
                    },
                    "token": {
                        "type": "string",
                        "description": "Optional per-call GitHub token (overrides configured token)"
                    }
                },
                "required": ["owner", "repo"]
            }),
        },
        ToolDefinition {
            name: "github_create_issue".into(),
            description: "Create a new issue in a repository".into(),
            parameters: json!({
                "type": "object",
                "properties": {
                    "owner": {
                        "type": "string",
                        "description": "Repository owner (user or organization)"
                    },
                    "repo": {
                        "type": "string",
                        "description": "Repository name"
                    },
                    "title": {
                        "type": "string",
                        "description": "Issue title"
                    },
                    "body": {
                        "type": "string",
                        "description": "Optional issue body (Markdown supported)"
                    },
                    "labels": {
                        "type": "array",
                        "items": { "type": "string" },
                        "description": "Optional list of label names to apply"
                    },
                    "token": {
                        "type": "string",
                        "description": "Optional per-call GitHub token (overrides configured token)"
                    }
                },
                "required": ["owner", "repo", "title"]
            }),
        },
        ToolDefinition {
            name: "github_get_issue".into(),
            description: "Get a specific issue by number".into(),
            parameters: json!({
                "type": "object",
                "properties": {
                    "owner": {
                        "type": "string",
                        "description": "Repository owner (user or organization)"
                    },
                    "repo": {
                        "type": "string",
                        "description": "Repository name"
                    },
                    "number": {
                        "type": "integer",
                        "description": "Issue number"
                    },
                    "token": {
                        "type": "string",
                        "description": "Optional per-call GitHub token (overrides configured token)"
                    }
                },
                "required": ["owner", "repo", "number"]
            }),
        },
        ToolDefinition {
            name: "github_list_pull_requests".into(),
            description: "List pull requests for a repository".into(),
            parameters: json!({
                "type": "object",
                "properties": {
                    "owner": {
                        "type": "string",
                        "description": "Repository owner (user or organization)"
                    },
                    "repo": {
                        "type": "string",
                        "description": "Repository name"
                    },
                    "state": {
                        "type": "string",
                        "description": "PR state filter: open, closed, or all (default: open)",
                        "enum": ["open", "closed", "all"]
                    },
                    "page": {
                        "type": "integer",
                        "description": "Page number for pagination (default: 1)"
                    },
                    "token": {
                        "type": "string",
                        "description": "Optional per-call GitHub token (overrides configured token)"
                    }
                },
                "required": ["owner", "repo"]
            }),
        },
        ToolDefinition {
            name: "github_get_pull_request".into(),
            description: "Get a specific pull request by number".into(),
            parameters: json!({
                "type": "object",
                "properties": {
                    "owner": {
                        "type": "string",
                        "description": "Repository owner (user or organization)"
                    },
                    "repo": {
                        "type": "string",
                        "description": "Repository name"
                    },
                    "number": {
                        "type": "integer",
                        "description": "Pull request number"
                    },
                    "token": {
                        "type": "string",
                        "description": "Optional per-call GitHub token (overrides configured token)"
                    }
                },
                "required": ["owner", "repo", "number"]
            }),
        },
        ToolDefinition {
            name: "github_create_pull_request".into(),
            description: "Create a new pull request".into(),
            parameters: json!({
                "type": "object",
                "properties": {
                    "owner": {
                        "type": "string",
                        "description": "Repository owner (user or organization)"
                    },
                    "repo": {
                        "type": "string",
                        "description": "Repository name"
                    },
                    "title": {
                        "type": "string",
                        "description": "Pull request title"
                    },
                    "body": {
                        "type": "string",
                        "description": "Optional pull request body (Markdown supported)"
                    },
                    "head": {
                        "type": "string",
                        "description": "The branch containing the changes (e.g. `feature-branch`)"
                    },
                    "base": {
                        "type": "string",
                        "description": "The branch to merge into (e.g. `main`)"
                    },
                    "token": {
                        "type": "string",
                        "description": "Optional per-call GitHub token (overrides configured token)"
                    }
                },
                "required": ["owner", "repo", "title", "head", "base"]
            }),
        },
        ToolDefinition {
            name: "github_search_code".into(),
            description: "Search code across GitHub repositories".into(),
            parameters: json!({
                "type": "object",
                "properties": {
                    "query": {
                        "type": "string",
                        "description": "Search query (supports GitHub search syntax)"
                    },
                    "page": {
                        "type": "integer",
                        "description": "Page number for pagination (default: 1)"
                    },
                    "token": {
                        "type": "string",
                        "description": "Optional per-call GitHub token (overrides configured token)"
                    }
                },
                "required": ["query"]
            }),
        },
        ToolDefinition {
            name: "github_get_file_content".into(),
            description: "Get the content of a file from a repository".into(),
            parameters: json!({
                "type": "object",
                "properties": {
                    "owner": {
                        "type": "string",
                        "description": "Repository owner (user or organization)"
                    },
                    "repo": {
                        "type": "string",
                        "description": "Repository name"
                    },
                    "path": {
                        "type": "string",
                        "description": "Path to the file within the repository"
                    },
                    "ref": {
                        "type": "string",
                        "description": "Optional git ref (branch, tag, or commit SHA)"
                    },
                    "token": {
                        "type": "string",
                        "description": "Optional per-call GitHub token (overrides configured token)"
                    }
                },
                "required": ["owner", "repo", "path"]
            }),
        },
    ]
}

// ---------------------------------------------------------------------------
// Adapter trait implementation
// ---------------------------------------------------------------------------

#[async_trait]
impl Adapter for GitHubAdapter {
    fn id(&self) -> &str {
        &self.id
    }

    fn adapter_type(&self) -> AdapterType {
        AdapterType::DevTools
    }

    async fn connect(&mut self) -> Result<()> {
        // If a token is configured, verify it by calling GET /user.
        if let Some(ref token) = self.token {
            let url = self.api_url("/user");
            let request = self.get_request(&url, token);
            let response = request
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

            let user: Value = response
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

        // If no token, we cannot check rate limit; report degraded.
        let token = match &self.token {
            Some(t) => t.clone(),
            None => return Ok(HealthStatus::Degraded),
        };

        let url = self.api_url("/rate_limit");
        let request = self.get_request(&url, &token);
        let response = request
            .send()
            .await
            .map_err(|e| AdapterError::ExecutionFailed {
                tool_name: "health_check".into(),
                reason: format!("rate limit check failed: {e}"),
            })?;

        if !response.status().is_success() {
            return Ok(HealthStatus::Degraded);
        }

        let body: Value = response
            .json()
            .await
            .map_err(|e| AdapterError::ExecutionFailed {
                tool_name: "health_check".into(),
                reason: format!("failed to parse rate limit response: {e}"),
            })?;

        // Check the core rate limit remaining.
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
        build_tool_definitions()
    }

    async fn execute_tool(&self, name: &str, params: Value) -> Result<Value> {
        if !self.connected {
            return Err(AdapterError::ExecutionFailed {
                tool_name: name.to_string(),
                reason: format!("adapter `{}` is not connected", self.id),
            });
        }

        match name {
            "github_list_repos" => self.tool_list_repos(params).await,
            "github_get_repo" => self.tool_get_repo(params).await,
            "github_list_issues" => self.tool_list_issues(params).await,
            "github_create_issue" => self.tool_create_issue(params).await,
            "github_get_issue" => self.tool_get_issue(params).await,
            "github_list_pull_requests" => self.tool_list_pull_requests(params).await,
            "github_get_pull_request" => self.tool_get_pull_request(params).await,
            "github_create_pull_request" => self.tool_create_pull_request(params).await,
            "github_search_code" => self.tool_search_code(params).await,
            "github_get_file_content" => self.tool_get_file_content(params).await,
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
// URL encoding helper (inline to avoid extra dependency)
// ---------------------------------------------------------------------------

mod urlencoding {
    /// Percent-encode a string for use in a URL query parameter.
    pub fn encode(input: &str) -> String {
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
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    // -- Construction tests --

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

    // -- Adapter trait basics --

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

    // -- Tool definitions --

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

        // github_get_repo requires owner and repo
        let get_repo = tools.iter().find(|t| t.name == "github_get_repo").unwrap();
        let required = get_repo.parameters["required"]
            .as_array()
            .expect("required should be an array");
        assert!(required.contains(&json!("owner")));
        assert!(required.contains(&json!("repo")));

        // github_create_issue requires owner, repo, title
        let create_issue = tools
            .iter()
            .find(|t| t.name == "github_create_issue")
            .unwrap();
        let required = create_issue.parameters["required"]
            .as_array()
            .expect("required should be an array");
        assert!(required.contains(&json!("owner")));
        assert!(required.contains(&json!("repo")));
        assert!(required.contains(&json!("title")));

        // github_create_pull_request requires owner, repo, title, head, base
        let create_pr = tools
            .iter()
            .find(|t| t.name == "github_create_pull_request")
            .unwrap();
        let required = create_pr.parameters["required"]
            .as_array()
            .expect("required should be an array");
        assert_eq!(required.len(), 5);
        assert!(required.contains(&json!("head")));
        assert!(required.contains(&json!("base")));

        // github_search_code requires query
        let search = tools
            .iter()
            .find(|t| t.name == "github_search_code")
            .unwrap();
        let required = search.parameters["required"]
            .as_array()
            .expect("required should be an array");
        assert!(required.contains(&json!("query")));
    }

    #[test]
    fn tool_parameters_list_repos_has_no_required_fields() {
        let adapter = GitHubAdapter::new("gh");
        let tools = adapter.tools();
        let list_repos = tools
            .iter()
            .find(|t| t.name == "github_list_repos")
            .unwrap();
        let required = list_repos.parameters["required"]
            .as_array()
            .expect("required should be an array");
        assert!(required.is_empty());
    }

    // -- Health check when not connected --

    #[tokio::test]
    async fn health_check_returns_unhealthy_when_disconnected() {
        let adapter = GitHubAdapter::new("gh");
        let status = adapter.health_check().await.unwrap();
        assert_eq!(status, HealthStatus::Unhealthy);
    }

    // -- Token resolution --

    #[test]
    fn resolve_token_uses_configured_token() {
        let adapter = GitHubAdapter::with_token("gh", "configured-token");
        let token = adapter.resolve_token(&json!({})).unwrap();
        assert_eq!(token, "configured-token");
    }

    #[test]
    fn resolve_token_per_call_overrides_configured() {
        let adapter = GitHubAdapter::with_token("gh", "configured-token");
        let token = adapter
            .resolve_token(&json!({"token": "per-call-token"}))
            .unwrap();
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

    // -- URL construction --

    #[test]
    fn api_url_constructs_correct_urls() {
        let adapter = GitHubAdapter::new("gh");
        assert_eq!(adapter.api_url("/user"), "https://api.github.com/user");
        assert_eq!(
            adapter.api_url("/repos/octocat/hello-world"),
            "https://api.github.com/repos/octocat/hello-world"
        );
    }

    #[test]
    fn api_url_works_with_custom_base_url() {
        let adapter = GitHubAdapter::with_base_url("gh-ent", "https://github.example.com/api/v3");
        assert_eq!(
            adapter.api_url("/user"),
            "https://github.example.com/api/v3/user"
        );
    }

    // -- Execute tool when not connected --

    #[tokio::test]
    async fn execute_tool_rejects_when_not_connected() {
        let adapter = GitHubAdapter::with_token("gh", "some-token");
        let result = adapter.execute_tool("github_list_repos", json!({})).await;
        assert!(result.is_err());
        let err = result.unwrap_err();
        assert!(
            err.to_string().contains("not connected"),
            "error should mention not connected: {err}"
        );
    }

    // -- Execute tool rejects unknown tool --

    #[tokio::test]
    async fn execute_tool_rejects_unknown_tool() {
        let mut adapter = GitHubAdapter::with_token("gh", "some-token");
        adapter.connected = true;
        let result = adapter.execute_tool("nonexistent_tool", json!({})).await;
        assert!(result.is_err());
        let err = result.unwrap_err();
        assert!(
            err.to_string().contains("tool not found"),
            "error should mention tool not found: {err}"
        );
    }

    // -- Missing required parameters --

    #[tokio::test]
    async fn get_repo_rejects_missing_owner() {
        let mut adapter = GitHubAdapter::with_token("gh", "token");
        adapter.connected = true;
        let result = adapter
            .execute_tool("github_get_repo", json!({"repo": "test"}))
            .await;
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("owner"));
    }

    #[tokio::test]
    async fn get_repo_rejects_missing_repo() {
        let mut adapter = GitHubAdapter::with_token("gh", "token");
        adapter.connected = true;
        let result = adapter
            .execute_tool("github_get_repo", json!({"owner": "test"}))
            .await;
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("repo"));
    }

    #[tokio::test]
    async fn create_issue_rejects_missing_title() {
        let mut adapter = GitHubAdapter::with_token("gh", "token");
        adapter.connected = true;
        let result = adapter
            .execute_tool(
                "github_create_issue",
                json!({"owner": "test", "repo": "test"}),
            )
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
            .execute_tool(
                "github_get_file_content",
                json!({"owner": "test", "repo": "test"}),
            )
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

    // -- Connect / disconnect --

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

    // -- URL encoding --

    #[test]
    fn urlencoding_encodes_spaces_and_special_chars() {
        assert_eq!(urlencoding::encode("hello world"), "hello%20world");
        assert_eq!(urlencoding::encode("a+b"), "a%2Bb");
        assert_eq!(urlencoding::encode("foo/bar"), "foo%2Fbar");
        assert_eq!(
            urlencoding::encode("safe-string_v1.0~beta"),
            "safe-string_v1.0~beta"
        );
    }
}
