//! Calendar adapter — manage events via CalDAV/iCal protocol.
//!
//! This adapter provides tools for interacting with CalDAV-compatible calendar
//! servers (such as Nextcloud, Radicale, Google Calendar via CalDAV, etc.).
//! It supports listing, creating, deleting, searching, and retrieving calendar
//! events using standard CalDAV HTTP methods and iCalendar (RFC 5545) format.

pub mod caldav;
pub mod tools;
pub mod types;

use async_trait::async_trait;
use serde_json::Value;
use tracing::info;

use crate::error::{AdapterError, Result};
use crate::traits::{Adapter, AdapterType, AuthRequirement, HealthStatus, ToolDefinition};

use caldav::{
    tool_create_event, tool_delete_event, tool_get_event, tool_list_events, tool_search_events,
};

/// Calendar service adapter using CalDAV/iCal protocol.
///
/// Provides tools for managing calendar events on any CalDAV-compatible
/// server.  Credentials can be pre-configured or supplied per-call.
pub struct CalendarAdapter {
    /// Unique identifier for this adapter instance.
    id: String,
    /// Whether the adapter has been connected.
    connected: bool,
    /// CalDAV server URL.
    caldav_url: Option<String>,
    /// Username for CalDAV authentication.
    username: Option<String>,
    /// Password for CalDAV authentication.
    password: Option<String>,
    /// HTTP client for making requests.
    client: reqwest::Client,
}

impl CalendarAdapter {
    /// Create a new calendar adapter with default configuration.
    pub fn new(id: impl Into<String>) -> Self {
        let client = reqwest::Client::builder()
            .user_agent("OpenIntentOS/0.1")
            .build()
            .unwrap_or_default();

        Self {
            id: id.into(),
            connected: false,
            caldav_url: None,
            username: None,
            password: None,
            client,
        }
    }

    /// Create a new calendar adapter with pre-configured CalDAV credentials.
    pub fn with_caldav(
        id: impl Into<String>,
        url: impl Into<String>,
        username: impl Into<String>,
        password: impl Into<String>,
    ) -> Self {
        let mut adapter = Self::new(id);
        adapter.caldav_url = Some(url.into());
        adapter.username = Some(username.into());
        adapter.password = Some(password.into());
        adapter
    }

    /// Resolve CalDAV URL from per-call params or pre-configured value.
    fn resolve_caldav_url(&self, params: &Value) -> Result<String> {
        if let Some(url) = params.get("caldav_url").and_then(|v| v.as_str())
            && !url.is_empty()
        {
            return Ok(url.to_string());
        }
        self.caldav_url
            .clone()
            .ok_or_else(|| AdapterError::InvalidParams {
                tool_name: "calendar".into(),
                reason: "no CalDAV URL configured or provided in params".into(),
            })
    }

    /// Resolve username from per-call params or pre-configured value.
    fn resolve_username(&self, params: &Value) -> Option<String> {
        params
            .get("username")
            .and_then(|v| v.as_str())
            .filter(|s| !s.is_empty())
            .map(|s| s.to_string())
            .or_else(|| self.username.clone())
    }

    /// Resolve password from per-call params or pre-configured value.
    fn resolve_password(&self, params: &Value) -> Option<String> {
        params
            .get("password")
            .and_then(|v| v.as_str())
            .filter(|s| !s.is_empty())
            .map(|s| s.to_string())
            .or_else(|| self.password.clone())
    }
}

#[async_trait]
impl Adapter for CalendarAdapter {
    fn id(&self) -> &str {
        &self.id
    }

    fn adapter_type(&self) -> AdapterType {
        AdapterType::Productivity
    }

    async fn connect(&mut self) -> Result<()> {
        if let Some(url) = &self.caldav_url {
            info!(id = %self.id, url = %url, "Calendar adapter connected with CalDAV URL");
        } else {
            info!(id = %self.id, "Calendar adapter connected without CalDAV URL");
        }
        self.connected = true;
        Ok(())
    }

    async fn disconnect(&mut self) -> Result<()> {
        info!(id = %self.id, "Calendar adapter disconnected");
        self.connected = false;
        Ok(())
    }

    async fn health_check(&self) -> Result<HealthStatus> {
        if !self.connected {
            return Ok(HealthStatus::Unhealthy);
        }
        if self.caldav_url.is_some() {
            Ok(HealthStatus::Healthy)
        } else {
            Ok(HealthStatus::Degraded)
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

        let known = matches!(
            name,
            "calendar_list_events"
                | "calendar_create_event"
                | "calendar_delete_event"
                | "calendar_search_events"
                | "calendar_get_event"
        );
        if !known {
            return Err(AdapterError::ToolNotFound {
                adapter_id: self.id.clone(),
                tool_name: name.to_string(),
            });
        }

        let caldav_url = self.resolve_caldav_url(&params)?;
        let username = self.resolve_username(&params);
        let password = self.resolve_password(&params);

        match name {
            "calendar_list_events" => {
                tool_list_events(
                    &self.client,
                    &caldav_url,
                    username.as_deref(),
                    password.as_deref(),
                    &params,
                )
                .await
            }
            "calendar_create_event" => {
                tool_create_event(
                    &self.client,
                    &caldav_url,
                    username.as_deref(),
                    password.as_deref(),
                    &params,
                )
                .await
            }
            "calendar_delete_event" => {
                tool_delete_event(
                    &self.client,
                    &caldav_url,
                    username.as_deref(),
                    password.as_deref(),
                    &params,
                )
                .await
            }
            "calendar_search_events" => {
                tool_search_events(
                    &self.client,
                    &caldav_url,
                    username.as_deref(),
                    password.as_deref(),
                    &params,
                )
                .await
            }
            "calendar_get_event" => {
                tool_get_event(
                    &self.client,
                    &caldav_url,
                    username.as_deref(),
                    password.as_deref(),
                    &params,
                )
                .await
            }
            _ => Err(AdapterError::ToolNotFound {
                adapter_id: self.id.clone(),
                tool_name: name.to_string(),
            }),
        }
    }

    fn required_auth(&self) -> Option<AuthRequirement> {
        Some(AuthRequirement {
            provider: "caldav".into(),
            scopes: vec!["calendar:read".into(), "calendar:write".into()],
        })
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use caldav::{
        build_calendar_query_xml, build_propfind_xml, format_caldav_datetime, format_ical_datetime,
        generate_ical_event, parse_ical_events,
    };
    use serde_json::json;

    #[test]
    fn new_creates_adapter_with_defaults() {
        let adapter = CalendarAdapter::new("cal-test");
        assert_eq!(adapter.id, "cal-test");
        assert!(!adapter.connected);
        assert!(adapter.caldav_url.is_none());
        assert!(adapter.username.is_none());
        assert!(adapter.password.is_none());
    }

    #[test]
    fn with_caldav_sets_credentials() {
        let adapter = CalendarAdapter::with_caldav(
            "cal-test",
            "https://caldav.example.com/cal",
            "user",
            "pass",
        );
        assert_eq!(adapter.id, "cal-test");
        assert_eq!(
            adapter.caldav_url.as_deref(),
            Some("https://caldav.example.com/cal")
        );
        assert_eq!(adapter.username.as_deref(), Some("user"));
        assert_eq!(adapter.password.as_deref(), Some("pass"));
    }

    #[test]
    fn adapter_id_returns_id() {
        let adapter = CalendarAdapter::new("my-cal");
        assert_eq!(adapter.id(), "my-cal");
    }

    #[test]
    fn adapter_type_is_productivity() {
        let adapter = CalendarAdapter::new("cal");
        assert_eq!(adapter.adapter_type(), AdapterType::Productivity);
    }

    #[test]
    fn required_auth_returns_caldav_scopes() {
        let adapter = CalendarAdapter::new("cal");
        let auth = adapter.required_auth().expect("should require auth");
        assert_eq!(auth.provider, "caldav");
        assert!(auth.scopes.contains(&"calendar:read".to_string()));
        assert!(auth.scopes.contains(&"calendar:write".to_string()));
    }

    #[test]
    fn tools_returns_exactly_five() {
        let adapter = CalendarAdapter::new("cal");
        let tools = adapter.tools();
        assert_eq!(tools.len(), 5);
    }

    #[test]
    fn tools_have_expected_names() {
        let adapter = CalendarAdapter::new("cal");
        let names: Vec<String> = adapter.tools().iter().map(|t| t.name.clone()).collect();
        let expected = vec![
            "calendar_list_events",
            "calendar_create_event",
            "calendar_delete_event",
            "calendar_search_events",
            "calendar_get_event",
        ];
        assert_eq!(names, expected);
    }

    #[test]
    fn tool_create_event_has_required_fields() {
        let adapter = CalendarAdapter::new("cal");
        let tools = adapter.tools();
        let create_event = tools
            .iter()
            .find(|t| t.name == "calendar_create_event")
            .expect("should have calendar_create_event");
        let required = create_event.parameters["required"]
            .as_array()
            .expect("required should be an array");
        assert!(required.contains(&json!("summary")));
        assert!(required.contains(&json!("start")));
        assert!(required.contains(&json!("end")));
    }

    #[test]
    fn tool_list_events_has_no_required_fields() {
        let adapter = CalendarAdapter::new("cal");
        let tools = adapter.tools();
        let list_events = tools
            .iter()
            .find(|t| t.name == "calendar_list_events")
            .expect("should have calendar_list_events");
        let required = list_events.parameters["required"]
            .as_array()
            .expect("required should be an array");
        assert!(required.is_empty());
    }

    #[tokio::test]
    async fn connect_succeeds_without_caldav_url() {
        let mut adapter = CalendarAdapter::new("cal");
        let result = adapter.connect().await;
        assert!(result.is_ok());
        assert!(adapter.connected);
    }

    #[tokio::test]
    async fn connect_succeeds_with_caldav_url() {
        let mut adapter =
            CalendarAdapter::with_caldav("cal", "https://caldav.example.com", "u", "p");
        let result = adapter.connect().await;
        assert!(result.is_ok());
        assert!(adapter.connected);
    }

    #[tokio::test]
    async fn disconnect_sets_connected_false() {
        let mut adapter = CalendarAdapter::new("cal");
        adapter.connected = true;
        adapter.disconnect().await.unwrap();
        assert!(!adapter.connected);
    }

    #[tokio::test]
    async fn health_check_returns_unhealthy_when_disconnected() {
        let adapter = CalendarAdapter::new("cal");
        let status = adapter.health_check().await.unwrap();
        assert_eq!(status, HealthStatus::Unhealthy);
    }

    #[tokio::test]
    async fn health_check_returns_degraded_when_connected_without_url() {
        let mut adapter = CalendarAdapter::new("cal");
        adapter.connected = true;
        let status = adapter.health_check().await.unwrap();
        assert_eq!(status, HealthStatus::Degraded);
    }

    #[tokio::test]
    async fn health_check_returns_healthy_when_connected_with_url() {
        let mut adapter =
            CalendarAdapter::with_caldav("cal", "https://caldav.example.com", "u", "p");
        adapter.connected = true;
        let status = adapter.health_check().await.unwrap();
        assert_eq!(status, HealthStatus::Healthy);
    }

    #[test]
    fn generate_ical_event_basic() {
        let ical = generate_ical_event(
            "test-uid-123",
            "Team Meeting",
            "2026-02-24T10:00:00Z",
            "2026-02-24T11:00:00Z",
            None,
            None,
        );
        assert!(ical.contains("BEGIN:VCALENDAR"));
        assert!(ical.contains("END:VCALENDAR"));
        assert!(ical.contains("BEGIN:VEVENT"));
        assert!(ical.contains("END:VEVENT"));
        assert!(ical.contains("UID:test-uid-123"));
        assert!(ical.contains("SUMMARY:Team Meeting"));
        assert!(ical.contains("DTSTART:20260224T100000Z"));
        assert!(ical.contains("DTEND:20260224T110000Z"));
        assert!(ical.contains("PRODID:-//OpenIntentOS//Calendar//EN"));
        assert!(!ical.contains("DESCRIPTION:"));
        assert!(!ical.contains("LOCATION:"));
    }

    #[test]
    fn generate_ical_event_with_description_and_location() {
        let ical = generate_ical_event(
            "uid-456",
            "Lunch",
            "2026-03-01T12:00:00Z",
            "2026-03-01T13:00:00Z",
            Some("Team lunch at the cafe"),
            Some("Downtown Cafe"),
        );
        assert!(ical.contains("DESCRIPTION:Team lunch at the cafe"));
        assert!(ical.contains("LOCATION:Downtown Cafe"));
    }

    #[test]
    fn format_ical_datetime_rfc3339() {
        let result = format_ical_datetime("2026-02-24T10:00:00Z");
        assert_eq!(result, "20260224T100000Z");
    }

    #[test]
    fn format_ical_datetime_naive() {
        let result = format_ical_datetime("2026-02-24T10:00:00");
        assert_eq!(result, "20260224T100000");
    }

    #[test]
    fn format_ical_datetime_date_only() {
        let result = format_ical_datetime("2026-02-24");
        assert_eq!(result, "20260224");
    }

    #[test]
    fn format_ical_datetime_fallback() {
        let result = format_ical_datetime("invalid-date");
        assert_eq!(result, "invalid-date");
    }

    #[test]
    fn build_calendar_query_xml_contains_time_range() {
        let xml = build_calendar_query_xml("20260224T000000Z", "20260301T000000Z");
        assert!(xml.contains("calendar-query"));
        assert!(xml.contains("VCALENDAR"));
        assert!(xml.contains("VEVENT"));
        assert!(xml.contains(r#"start="20260224T000000Z""#));
        assert!(xml.contains(r#"end="20260301T000000Z""#));
        assert!(xml.contains("getetag"));
        assert!(xml.contains("calendar-data"));
    }

    #[test]
    fn build_propfind_xml_contains_required_elements() {
        let xml = build_propfind_xml();
        assert!(xml.contains("propfind"));
        assert!(xml.contains("displayname"));
        assert!(xml.contains("resourcetype"));
        assert!(xml.contains("supported-calendar-component-set"));
    }

    #[test]
    fn format_caldav_datetime_formats_correctly() {
        let dt = chrono::DateTime::parse_from_rfc3339("2026-02-24T15:30:00Z")
            .unwrap()
            .with_timezone(&chrono::Utc);
        let result = format_caldav_datetime(&dt);
        assert_eq!(result, "20260224T153000Z");
    }

    #[test]
    fn parse_ical_events_extracts_single_event() {
        let ical_data = "\
BEGIN:VCALENDAR\r\n\
VERSION:2.0\r\n\
BEGIN:VEVENT\r\n\
 UID:abc-123\r\n\
SUMMARY:Test Event\r\n\
DTSTART:20260224T100000Z\r\n\
DTEND:20260224T110000Z\r\n\
DESCRIPTION:A test event\r\n\
END:VEVENT\r\n\
END:VCALENDAR\r\n";

        let events = parse_ical_events(ical_data);
        assert_eq!(events.len(), 1);
        assert_eq!(events[0]["uid"], "abc-123");
        assert_eq!(events[0]["summary"], "Test Event");
        assert_eq!(events[0]["dtstart"], "20260224T100000Z");
        assert_eq!(events[0]["dtend"], "20260224T110000Z");
        assert_eq!(events[0]["description"], "A test event");
    }

    #[test]
    fn parse_ical_events_extracts_multiple_events() {
        let ical_data = "\
BEGIN:VCALENDAR\r\n\
BEGIN:VEVENT\r\n\
 UID:event-1\r\n\
SUMMARY:First\r\n\
END:VEVENT\r\n\
BEGIN:VEVENT\r\n\
 UID:event-2\r\n\
SUMMARY:Second\r\n\
END:VEVENT\r\n\
END:VCALENDAR\r\n";

        let events = parse_ical_events(ical_data);
        assert_eq!(events.len(), 2);
        assert_eq!(events[0]["uid"], "event-1");
        assert_eq!(events[1]["uid"], "event-2");
    }

    #[test]
    fn parse_ical_events_handles_empty_input() {
        let events = parse_ical_events("");
        assert!(events.is_empty());
    }

    #[test]
    fn parse_ical_events_strips_parameter_parts() {
        let ical_data = "\
BEGIN:VEVENT\r\n\
DTSTART;VALUE=DATE:20260224\r\n\
SUMMARY:Date-only Event\r\n\
END:VEVENT\r\n";

        let events = parse_ical_events(ical_data);
        assert_eq!(events.len(), 1);
        assert_eq!(events[0]["dtstart"], "20260224");
    }

    #[test]
    fn resolve_caldav_url_uses_configured() {
        let adapter = CalendarAdapter::with_caldav("cal", "https://caldav.example.com", "u", "p");
        let url = adapter.resolve_caldav_url(&json!({})).unwrap();
        assert_eq!(url, "https://caldav.example.com");
    }

    #[test]
    fn resolve_caldav_url_per_call_overrides() {
        let adapter = CalendarAdapter::with_caldav("cal", "https://caldav.example.com", "u", "p");
        let url = adapter
            .resolve_caldav_url(&json!({"caldav_url": "https://other.example.com"}))
            .unwrap();
        assert_eq!(url, "https://other.example.com");
    }

    #[test]
    fn resolve_caldav_url_fails_when_none() {
        let adapter = CalendarAdapter::new("cal");
        let result = adapter.resolve_caldav_url(&json!({}));
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn execute_tool_rejects_when_not_connected() {
        let adapter = CalendarAdapter::with_caldav("cal", "https://caldav.example.com", "u", "p");
        let result = adapter
            .execute_tool("calendar_list_events", json!({}))
            .await;
        assert!(result.is_err());
        let err = result.unwrap_err();
        assert!(err.to_string().contains("not connected"));
    }

    #[tokio::test]
    async fn execute_tool_rejects_unknown_tool() {
        let mut adapter = CalendarAdapter::new("cal");
        adapter.connected = true;
        let result = adapter.execute_tool("nonexistent_tool", json!({})).await;
        assert!(result.is_err());
        let err = result.unwrap_err();
        assert!(err.to_string().contains("tool not found"));
    }

    #[tokio::test]
    async fn create_event_rejects_missing_summary() {
        let mut adapter =
            CalendarAdapter::with_caldav("cal", "https://caldav.example.com", "u", "p");
        adapter.connected = true;
        let result = adapter
            .execute_tool(
                "calendar_create_event",
                json!({
                    "start": "2026-02-24T10:00:00Z",
                    "end": "2026-02-24T11:00:00Z"
                }),
            )
            .await;
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("summary"));
    }

    #[tokio::test]
    async fn create_event_rejects_missing_start() {
        let mut adapter =
            CalendarAdapter::with_caldav("cal", "https://caldav.example.com", "u", "p");
        adapter.connected = true;
        let result = adapter
            .execute_tool(
                "calendar_create_event",
                json!({
                    "summary": "Test",
                    "end": "2026-02-24T11:00:00Z"
                }),
            )
            .await;
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("start"));
    }

    #[tokio::test]
    async fn delete_event_rejects_missing_uid() {
        let mut adapter =
            CalendarAdapter::with_caldav("cal", "https://caldav.example.com", "u", "p");
        adapter.connected = true;
        let result = adapter
            .execute_tool("calendar_delete_event", json!({}))
            .await;
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("uid"));
    }

    #[tokio::test]
    async fn search_events_rejects_missing_query() {
        let mut adapter =
            CalendarAdapter::with_caldav("cal", "https://caldav.example.com", "u", "p");
        adapter.connected = true;
        let result = adapter
            .execute_tool("calendar_search_events", json!({}))
            .await;
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("query"));
    }

    #[tokio::test]
    async fn get_event_rejects_missing_uid() {
        let mut adapter =
            CalendarAdapter::with_caldav("cal", "https://caldav.example.com", "u", "p");
        adapter.connected = true;
        let result = adapter.execute_tool("calendar_get_event", json!({})).await;
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("uid"));
    }
}
