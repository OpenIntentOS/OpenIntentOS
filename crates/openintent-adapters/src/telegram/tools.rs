//! Tool definitions for the Telegram adapter.

use serde_json::json;

use crate::traits::ToolDefinition;

/// Build the list of tool definitions for the Telegram adapter.
pub fn build_tool_definitions() -> Vec<ToolDefinition> {
    vec![
        ToolDefinition {
            name: "telegram_send_message".into(),
            description: "Send a text message to a Telegram chat".into(),
            parameters: json!({
                "type": "object",
                "properties": {
                    "chat_id": {
                        "type": "string",
                        "description": "Unique identifier for the target chat or username of the target channel (e.g. @channelusername)"
                    },
                    "text": {
                        "type": "string",
                        "description": "Text of the message to send"
                    },
                    "parse_mode": {
                        "type": "string",
                        "description": "Mode for parsing entities in the message text: HTML or Markdown",
                        "enum": ["HTML", "Markdown"]
                    }
                },
                "required": ["chat_id", "text"]
            }),
        },
        ToolDefinition {
            name: "telegram_send_photo".into(),
            description: "Send a photo to a Telegram chat".into(),
            parameters: json!({
                "type": "object",
                "properties": {
                    "chat_id": {
                        "type": "string",
                        "description": "Unique identifier for the target chat or username of the target channel"
                    },
                    "photo_url": {
                        "type": "string",
                        "description": "URL of the photo to send"
                    },
                    "caption": {
                        "type": "string",
                        "description": "Photo caption, 0-1024 characters"
                    }
                },
                "required": ["chat_id", "photo_url"]
            }),
        },
        ToolDefinition {
            name: "telegram_send_document".into(),
            description: "Send a local file as a document to a Telegram chat. Use this to deliver files (PDF, CSV, ZIP, etc.) to the user.".into(),
            parameters: json!({
                "type": "object",
                "properties": {
                    "chat_id": {
                        "type": "string",
                        "description": "Unique identifier for the target chat"
                    },
                    "file_path": {
                        "type": "string",
                        "description": "Absolute path to the local file to send"
                    },
                    "caption": {
                        "type": "string",
                        "description": "Document caption, 0-1024 characters"
                    }
                },
                "required": ["chat_id", "file_path"]
            }),
        },
        ToolDefinition {
            name: "telegram_send_video".into(),
            description: "Send a local video file to a Telegram chat. Use this to deliver video files (MP4) to the user. Max 50MB.".into(),
            parameters: json!({
                "type": "object",
                "properties": {
                    "chat_id": {
                        "type": "string",
                        "description": "Unique identifier for the target chat"
                    },
                    "file_path": {
                        "type": "string",
                        "description": "Absolute path to the local video file to send (MP4 format)"
                    },
                    "caption": {
                        "type": "string",
                        "description": "Video caption, 0-1024 characters"
                    }
                },
                "required": ["chat_id", "file_path"]
            }),
        },
        ToolDefinition {
            name: "telegram_get_updates".into(),
            description: "Get recent incoming updates (messages, callback queries, etc.) for the bot".into(),
            parameters: json!({
                "type": "object",
                "properties": {
                    "limit": {
                        "type": "integer",
                        "description": "Maximum number of updates to retrieve (1-100, default: 100)"
                    },
                    "offset": {
                        "type": "integer",
                        "description": "Identifier of the first update to be returned; use to acknowledge previous updates"
                    }
                },
                "required": []
            }),
        },
        ToolDefinition {
            name: "telegram_get_chat".into(),
            description: "Get up-to-date information about a Telegram chat".into(),
            parameters: json!({
                "type": "object",
                "properties": {
                    "chat_id": {
                        "type": "string",
                        "description": "Unique identifier for the target chat or username of the target supergroup/channel"
                    }
                },
                "required": ["chat_id"]
            }),
        },
        ToolDefinition {
            name: "telegram_set_webhook".into(),
            description: "Set a webhook URL for the bot to receive updates via HTTPS POST".into(),
            parameters: json!({
                "type": "object",
                "properties": {
                    "url": {
                        "type": "string",
                        "description": "HTTPS URL to send updates to; use an empty string to remove the webhook"
                    }
                },
                "required": ["url"]
            }),
        },
        ToolDefinition {
            name: "telegram_configure_group_chat".into(),
            description: "Configure group chat settings including bot permissions and moderation features".into(),
            parameters: json!({
                "type": "object",
                "properties": {
                    "chat_id": {
                        "type": "string",
                        "description": "Unique identifier for the target group chat"
                    },
                    "allow_bots": {
                        "type": "boolean",
                        "description": "Whether to allow other bots to send messages in the group (default: true)"
                    },
                    "auto_delete_service_messages": {
                        "type": "boolean",
                        "description": "Whether to automatically delete service messages like 'user joined' (default: false)"
                    },
                    "protect_content": {
                        "type": "boolean",
                        "description": "Whether to protect content from forwarding (default: false)"
                    }
                },
                "required": ["chat_id"]
            }),
        },
        ToolDefinition {
            name: "telegram_get_chat_member".into(),
            description: "Get detailed information about a chat member including their permissions".into(),
            parameters: json!({
                "type": "object",
                "properties": {
                    "chat_id": {
                        "type": "string",
                        "description": "Unique identifier for the target chat"
                    },
                    "user_id": {
                        "type": "integer",
                        "description": "Unique identifier of the target user"
                    }
                },
                "required": ["chat_id", "user_id"]
            }),
        },
    ]
}
