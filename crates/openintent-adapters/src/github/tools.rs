//! Tool definitions for the GitHub adapter.

use serde_json::json;

use crate::traits::ToolDefinition;

/// Build the list of tool definitions for the GitHub adapter.
pub fn build_tool_definitions() -> Vec<ToolDefinition> {
    vec![
        ToolDefinition {
            name: "github_list_repos".into(),
            description: "List repositories for the authenticated user or an organization".into(),
            parameters: json!({
                "type": "object",
                "properties": {
                    "org": { "type": "string", "description": "Optional organization name. If omitted, lists repos for the authenticated user." },
                    "page": { "type": "integer", "description": "Page number for pagination (default: 1)" },
                    "per_page": { "type": "integer", "description": "Number of results per page (default: 30, max: 100)" },
                    "token": { "type": "string", "description": "Optional per-call GitHub token (overrides configured token)" }
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
                    "owner": { "type": "string", "description": "Repository owner (user or organization)" },
                    "repo": { "type": "string", "description": "Repository name" },
                    "token": { "type": "string", "description": "Optional per-call GitHub token (overrides configured token)" }
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
                    "owner": { "type": "string", "description": "Repository owner (user or organization)" },
                    "repo": { "type": "string", "description": "Repository name" },
                    "state": { "type": "string", "description": "Issue state filter: open, closed, or all (default: open)", "enum": ["open", "closed", "all"] },
                    "page": { "type": "integer", "description": "Page number for pagination (default: 1)" },
                    "token": { "type": "string", "description": "Optional per-call GitHub token (overrides configured token)" }
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
                    "owner": { "type": "string", "description": "Repository owner (user or organization)" },
                    "repo": { "type": "string", "description": "Repository name" },
                    "title": { "type": "string", "description": "Issue title" },
                    "body": { "type": "string", "description": "Optional issue body (Markdown supported)" },
                    "labels": { "type": "array", "items": { "type": "string" }, "description": "Optional list of label names to apply" },
                    "token": { "type": "string", "description": "Optional per-call GitHub token (overrides configured token)" }
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
                    "owner": { "type": "string", "description": "Repository owner (user or organization)" },
                    "repo": { "type": "string", "description": "Repository name" },
                    "number": { "type": "integer", "description": "Issue number" },
                    "token": { "type": "string", "description": "Optional per-call GitHub token (overrides configured token)" }
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
                    "owner": { "type": "string", "description": "Repository owner (user or organization)" },
                    "repo": { "type": "string", "description": "Repository name" },
                    "state": { "type": "string", "description": "PR state filter: open, closed, or all (default: open)", "enum": ["open", "closed", "all"] },
                    "page": { "type": "integer", "description": "Page number for pagination (default: 1)" },
                    "token": { "type": "string", "description": "Optional per-call GitHub token (overrides configured token)" }
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
                    "owner": { "type": "string", "description": "Repository owner (user or organization)" },
                    "repo": { "type": "string", "description": "Repository name" },
                    "number": { "type": "integer", "description": "Pull request number" },
                    "token": { "type": "string", "description": "Optional per-call GitHub token (overrides configured token)" }
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
                    "owner": { "type": "string", "description": "Repository owner (user or organization)" },
                    "repo": { "type": "string", "description": "Repository name" },
                    "title": { "type": "string", "description": "Pull request title" },
                    "body": { "type": "string", "description": "Optional pull request body (Markdown supported)" },
                    "head": { "type": "string", "description": "The branch containing the changes (e.g. `feature-branch`)" },
                    "base": { "type": "string", "description": "The branch to merge into (e.g. `main`)" },
                    "token": { "type": "string", "description": "Optional per-call GitHub token (overrides configured token)" }
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
                    "query": { "type": "string", "description": "Search query (supports GitHub search syntax)" },
                    "page": { "type": "integer", "description": "Page number for pagination (default: 1)" },
                    "token": { "type": "string", "description": "Optional per-call GitHub token (overrides configured token)" }
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
                    "owner": { "type": "string", "description": "Repository owner (user or organization)" },
                    "repo": { "type": "string", "description": "Repository name" },
                    "path": { "type": "string", "description": "Path to the file within the repository" },
                    "ref": { "type": "string", "description": "Optional git ref (branch, tag, or commit SHA)" },
                    "token": { "type": "string", "description": "Optional per-call GitHub token (overrides configured token)" }
                },
                "required": ["owner", "repo", "path"]
            }),
        },
    ]
}
