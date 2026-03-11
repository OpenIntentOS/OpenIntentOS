//! Tool definitions for the Feishu adapter.

use serde_json::json;

use crate::traits::ToolDefinition;

/// Build the list of tool definitions for the Feishu adapter.
pub fn build_tool_definitions() -> Vec<ToolDefinition> {
    vec![
        ToolDefinition {
            name: "feishu_send_message".into(),
            description: "Send a text or interactive message to a Feishu user or group chat"
                .into(),
            parameters: json!({
                "type": "object",
                "properties": {
                    "receive_id": {
                        "type": "string",
                        "description": "The ID of the message recipient (user or chat)"
                    },
                    "receive_id_type": {
                        "type": "string",
                        "description": "Type of receive_id: open_id, user_id, or chat_id",
                        "enum": ["open_id", "user_id", "chat_id"]
                    },
                    "msg_type": {
                        "type": "string",
                        "description": "Message type: text or interactive",
                        "enum": ["text", "interactive"]
                    },
                    "content": {
                        "type": "string",
                        "description": "Message content as JSON string"
                    }
                },
                "required": ["receive_id", "receive_id_type", "msg_type", "content"]
            }),
        },
        ToolDefinition {
            name: "feishu_list_chats".into(),
            description: "List available group chats the bot has joined".into(),
            parameters: json!({
                "type": "object",
                "properties": {
                    "page_size": {
                        "type": "integer",
                        "description": "Number of chats per page (default: 20)"
                    },
                    "page_token": {
                        "type": "string",
                        "description": "Pagination token for the next page"
                    }
                },
                "required": []
            }),
        },
        ToolDefinition {
            name: "feishu_get_chat_messages".into(),
            description: "Get recent messages from a Feishu group chat".into(),
            parameters: json!({
                "type": "object",
                "properties": {
                    "container_id": {
                        "type": "string",
                        "description": "The chat ID to retrieve messages from"
                    },
                    "page_size": {
                        "type": "integer",
                        "description": "Number of messages to retrieve (default: 20)"
                    }
                },
                "required": ["container_id"]
            }),
        },
        ToolDefinition {
            name: "feishu_create_doc".into(),
            description: "Create a new document in Feishu Docs".into(),
            parameters: json!({
                "type": "object",
                "properties": {
                    "title": {
                        "type": "string",
                        "description": "Title of the document"
                    },
                    "folder_token": {
                        "type": "string",
                        "description": "Optional folder token to create the document in"
                    }
                },
                "required": ["title"]
            }),
        },
        ToolDefinition {
            name: "feishu_search_users".into(),
            description: "Search for Feishu users by name or email".into(),
            parameters: json!({
                "type": "object",
                "properties": {
                    "query": {
                        "type": "string",
                        "description": "Search query (name, email, etc.)"
                    },
                    "page_size": {
                        "type": "integer",
                        "description": "Number of results per page (default: 20)"
                    }
                },
                "required": ["query"]
            }),
        },
        ToolDefinition {
            name: "feishu_get_user_info".into(),
            description: "Get detailed information about a Feishu user".into(),
            parameters: json!({
                "type": "object",
                "properties": {
                    "user_id": {
                        "type": "string",
                        "description": "The user ID to look up"
                    },
                    "user_id_type": {
                        "type": "string",
                        "description": "Type of user_id: open_id or user_id (default: open_id)",
                        "enum": ["open_id", "user_id"]
                    }
                },
                "required": ["user_id"]
            }),
        },
    ]
}
