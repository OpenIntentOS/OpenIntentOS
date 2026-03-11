//! Tool definitions for the email adapter.

use serde_json::json;

use crate::traits::ToolDefinition;

/// Build the list of tool definitions for the email adapter.
pub fn build_tool_definitions() -> Vec<ToolDefinition> {
    vec![
        ToolDefinition {
            name: "email_list_inbox".into(),
            description: "List recent emails from the inbox via IMAP".into(),
            parameters: json!({
                "type": "object",
                "properties": {
                    "count": {
                        "type": "integer",
                        "description": "Number of recent emails to list (default: 10)"
                    },
                    "username": {
                        "type": "string",
                        "description": "Email account username"
                    },
                    "password": {
                        "type": "string",
                        "description": "Email account password or app-specific password"
                    },
                    "host": {
                        "type": "string",
                        "description": "IMAP server hostname (optional if configured on adapter)"
                    }
                },
                "required": ["username", "password"]
            }),
        },
        ToolDefinition {
            name: "email_read".into(),
            description: "Read a specific email by sequence number via IMAP".into(),
            parameters: json!({
                "type": "object",
                "properties": {
                    "message_id": {
                        "type": "string",
                        "description": "The email sequence number to read"
                    },
                    "username": {
                        "type": "string",
                        "description": "Email account username"
                    },
                    "password": {
                        "type": "string",
                        "description": "Email account password or app-specific password"
                    },
                    "host": {
                        "type": "string",
                        "description": "IMAP server hostname (optional if configured on adapter)"
                    }
                },
                "required": ["message_id", "username", "password"]
            }),
        },
        ToolDefinition {
            name: "email_send".into(),
            description: "Send an email via SMTP".into(),
            parameters: json!({
                "type": "object",
                "properties": {
                    "to": {
                        "type": "string",
                        "description": "Recipient email address"
                    },
                    "subject": {
                        "type": "string",
                        "description": "Email subject line"
                    },
                    "body": {
                        "type": "string",
                        "description": "Email body text"
                    },
                    "username": {
                        "type": "string",
                        "description": "SMTP account username (usually email address)"
                    },
                    "password": {
                        "type": "string",
                        "description": "SMTP account password or app-specific password"
                    },
                    "host": {
                        "type": "string",
                        "description": "SMTP server hostname (optional if configured on adapter)"
                    }
                },
                "required": ["to", "subject", "body", "username", "password"]
            }),
        },
        ToolDefinition {
            name: "email_search".into(),
            description: "Search emails by IMAP SEARCH query".into(),
            parameters: json!({
                "type": "object",
                "properties": {
                    "query": {
                        "type": "string",
                        "description": "IMAP SEARCH query (e.g., 'FROM \"user@example.com\"', 'SUBJECT \"hello\"', 'UNSEEN')"
                    },
                    "count": {
                        "type": "integer",
                        "description": "Maximum number of results to return (default: 10)"
                    },
                    "username": {
                        "type": "string",
                        "description": "Email account username"
                    },
                    "password": {
                        "type": "string",
                        "description": "Email account password or app-specific password"
                    },
                    "host": {
                        "type": "string",
                        "description": "IMAP server hostname (optional if configured on adapter)"
                    }
                },
                "required": ["query", "username", "password"]
            }),
        },
    ]
}
