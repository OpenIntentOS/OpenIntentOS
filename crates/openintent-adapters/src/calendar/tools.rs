//! Tool definitions for the calendar adapter.

use serde_json::json;

use crate::traits::ToolDefinition;

/// Build the list of tool definitions for the calendar adapter.
pub fn build_tool_definitions() -> Vec<ToolDefinition> {
    vec![
        ToolDefinition {
            name: "calendar_list_events".into(),
            description: "List upcoming calendar events from a CalDAV server".into(),
            parameters: json!({
                "type": "object",
                "properties": {
                    "days_ahead": {
                        "type": "integer",
                        "description": "Number of days ahead to look (default: 7)"
                    },
                    "caldav_url": {
                        "type": "string",
                        "description": "CalDAV server URL (overrides configured URL)"
                    },
                    "username": {
                        "type": "string",
                        "description": "CalDAV username (overrides configured)"
                    },
                    "password": {
                        "type": "string",
                        "description": "CalDAV password (overrides configured)"
                    }
                },
                "required": []
            }),
        },
        ToolDefinition {
            name: "calendar_create_event".into(),
            description: "Create a new calendar event on a CalDAV server".into(),
            parameters: json!({
                "type": "object",
                "properties": {
                    "summary": {
                        "type": "string",
                        "description": "Event title/summary"
                    },
                    "start": {
                        "type": "string",
                        "description": "Event start time in ISO 8601 format"
                    },
                    "end": {
                        "type": "string",
                        "description": "Event end time in ISO 8601 format"
                    },
                    "description": {
                        "type": "string",
                        "description": "Optional event description"
                    },
                    "location": {
                        "type": "string",
                        "description": "Optional event location"
                    }
                },
                "required": ["summary", "start", "end"]
            }),
        },
        ToolDefinition {
            name: "calendar_delete_event".into(),
            description: "Delete a calendar event by its UID".into(),
            parameters: json!({
                "type": "object",
                "properties": {
                    "uid": {
                        "type": "string",
                        "description": "The UID of the event to delete"
                    }
                },
                "required": ["uid"]
            }),
        },
        ToolDefinition {
            name: "calendar_search_events".into(),
            description: "Search calendar events by text query".into(),
            parameters: json!({
                "type": "object",
                "properties": {
                    "query": {
                        "type": "string",
                        "description": "Text to search for in event summaries and descriptions"
                    },
                    "days_ahead": {
                        "type": "integer",
                        "description": "Number of days ahead to search (default: 7)"
                    }
                },
                "required": ["query"]
            }),
        },
        ToolDefinition {
            name: "calendar_get_event".into(),
            description: "Get detailed information about a calendar event by UID".into(),
            parameters: json!({
                "type": "object",
                "properties": {
                    "uid": {
                        "type": "string",
                        "description": "The UID of the event to retrieve"
                    }
                },
                "required": ["uid"]
            }),
        },
    ]
}
