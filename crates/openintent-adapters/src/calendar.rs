//! Calendar adapter -- manage events via CalDAV/iCal protocol.
//!
//! This adapter provides tools for interacting with CalDAV-compatible calendar
//! servers (such as Nextcloud, Radicale, Google Calendar via CalDAV, etc.).
//! It supports listing, creating, deleting, searching, and retrieving calendar
//! events using standard CalDAV HTTP methods and iCalendar (RFC 5545) format.

use async_trait::async_trait;
use chrono::{Duration, Utc};
use serde_json::{Value, json};
use tracing::{debug, info};
use uuid::Uuid;

use crate::error::{AdapterError, Result};
use crate::traits::{Adapter, AdapterType, AuthRequirement, HealthStatus, ToolDefinition};

/// Default number of days ahead to look for events.
const DEFAULT_DAYS_AHEAD: i64 = 7;

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

    // -----------------------------------------------------------------------
    // Credential resolution
    // -----------------------------------------------------------------------

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

    // -----------------------------------------------------------------------
    // iCalendar format helpers (RFC 5545)
    // -----------------------------------------------------------------------

    /// Generate an iCalendar VCALENDAR string for a new event.
    ///
    /// The `start` and `end` parameters should be ISO 8601 formatted strings.
    /// Returns the iCalendar string and the generated UID.
    pub fn generate_ical_event(
        uid: &str,
        summary: &str,
        start: &str,
        end: &str,
        description: Option<&str>,
        location: Option<&str>,
    ) -> String {
        let dtstart = Self::format_ical_datetime(start);
        let dtend = Self::format_ical_datetime(end);

        let mut ical = String::with_capacity(512);
        ical.push_str("BEGIN:VCALENDAR\r\n");
        ical.push_str("VERSION:2.0\r\n");
        ical.push_str("PRODID:-//OpenIntentOS//Calendar//EN\r\n");
        ical.push_str("BEGIN:VEVENT\r\n");
        ical.push_str(&format!("UID:{uid}\r\n"));
        ical.push_str(&format!("DTSTART:{dtstart}\r\n"));
        ical.push_str(&format!("DTEND:{dtend}\r\n"));
        ical.push_str(&format!("SUMMARY:{summary}\r\n"));
        if let Some(desc) = description {
            ical.push_str(&format!("DESCRIPTION:{desc}\r\n"));
        }
        if let Some(loc) = location {
            ical.push_str(&format!("LOCATION:{loc}\r\n"));
        }
        ical.push_str("END:VEVENT\r\n");
        ical.push_str("END:VCALENDAR\r\n");
        ical
    }

    /// Format an ISO 8601 datetime string into iCalendar DTSTART/DTEND format.
    ///
    /// Converts `2026-02-24T10:00:00Z` to `20260224T100000Z`.
    /// If the input does not parse correctly, returns it unchanged.
    pub fn format_ical_datetime(iso: &str) -> String {
        // Try to parse as a full ISO 8601 datetime with timezone
        if let Ok(dt) = chrono::DateTime::parse_from_rfc3339(iso) {
            return dt.format("%Y%m%dT%H%M%SZ").to_string();
        }
        // Try to parse as NaiveDateTime (no timezone)
        if let Ok(dt) = chrono::NaiveDateTime::parse_from_str(iso, "%Y-%m-%dT%H:%M:%S") {
            return dt.format("%Y%m%dT%H%M%S").to_string();
        }
        // Try date-only format
        if let Ok(dt) = chrono::NaiveDate::parse_from_str(iso, "%Y-%m-%d") {
            return dt.format("%Y%m%d").to_string();
        }
        // Fallback: return as-is
        iso.to_string()
    }

    // -----------------------------------------------------------------------
    // CalDAV XML request builders
    // -----------------------------------------------------------------------

    /// Build a CalDAV REPORT XML body for listing events in a time range.
    pub fn build_calendar_query_xml(start: &str, end: &str) -> String {
        format!(
            r#"<?xml version="1.0" encoding="utf-8" ?>
<C:calendar-query xmlns:D="DAV:" xmlns:C="urn:ietf:params:xml:ns:caldav">
  <D:prop>
    <D:getetag/>
    <C:calendar-data/>
  </D:prop>
  <C:filter>
    <C:comp-filter name="VCALENDAR">
      <C:comp-filter name="VEVENT">
        <C:time-range start="{start}" end="{end}"/>
      </C:comp-filter>
    </C:comp-filter>
  </C:filter>
</C:calendar-query>"#
        )
    }

    /// Build a CalDAV PROPFIND XML body for discovering calendars.
    pub fn build_propfind_xml() -> String {
        r#"<?xml version="1.0" encoding="utf-8" ?>
<D:propfind xmlns:D="DAV:" xmlns:C="urn:ietf:params:xml:ns:caldav">
  <D:prop>
    <D:displayname/>
    <D:resourcetype/>
    <C:supported-calendar-component-set/>
  </D:prop>
</D:propfind>"#
            .to_string()
    }

    /// Format a chrono DateTime as a CalDAV time-range value (yyyymmddThhmmssZ).
    pub fn format_caldav_datetime(dt: &chrono::DateTime<chrono::Utc>) -> String {
        dt.format("%Y%m%dT%H%M%SZ").to_string()
    }

    // -----------------------------------------------------------------------
    // Basic iCal response parsing
    // -----------------------------------------------------------------------

    /// Extract VEVENT blocks from raw iCalendar data.
    ///
    /// Returns a list of JSON objects with parsed event fields.
    pub fn parse_ical_events(ical_data: &str) -> Vec<Value> {
        let mut events = Vec::new();
        let mut in_vevent = false;
        let mut current_event: serde_json::Map<String, Value> = serde_json::Map::new();

        for line in ical_data.lines() {
            let trimmed = line.trim();
            if trimmed == "BEGIN:VEVENT" {
                in_vevent = true;
                current_event = serde_json::Map::new();
            } else if trimmed == "END:VEVENT" {
                in_vevent = false;
                events.push(Value::Object(current_event.clone()));
            } else if in_vevent && let Some((key, value)) = trimmed.split_once(':') {
                // Strip parameter parts (e.g., DTSTART;VALUE=DATE:20260101)
                let clean_key = key.split(';').next().unwrap_or(key);
                current_event.insert(clean_key.to_lowercase(), Value::String(value.to_string()));
            }
        }

        events
    }

    // -----------------------------------------------------------------------
    // HTTP helpers
    // -----------------------------------------------------------------------

    /// Build a request with optional basic auth.
    fn build_request(
        &self,
        method: reqwest::Method,
        url: &str,
        username: Option<&str>,
        password: Option<&str>,
    ) -> reqwest::RequestBuilder {
        let mut builder = self.client.request(method, url);
        if let (Some(user), Some(pass)) = (username, password) {
            builder = builder.basic_auth(user, Some(pass));
        }
        builder
    }

    // -----------------------------------------------------------------------
    // Tool implementations
    // -----------------------------------------------------------------------

    /// List upcoming calendar events.
    async fn tool_list_events(&self, params: Value) -> Result<Value> {
        let caldav_url = self.resolve_caldav_url(&params)?;
        let username = self.resolve_username(&params);
        let password = self.resolve_password(&params);

        let days_ahead = params
            .get("days_ahead")
            .and_then(|v| v.as_i64())
            .unwrap_or(DEFAULT_DAYS_AHEAD);

        let now = Utc::now();
        let end = now + Duration::days(days_ahead);
        let start_str = Self::format_caldav_datetime(&now);
        let end_str = Self::format_caldav_datetime(&end);

        let xml_body = Self::build_calendar_query_xml(&start_str, &end_str);

        debug!(url = %caldav_url, days_ahead = days_ahead, "listing calendar events");

        let response = self
            .build_request(
                reqwest::Method::from_bytes(b"REPORT").unwrap_or(reqwest::Method::POST),
                &caldav_url,
                username.as_deref(),
                password.as_deref(),
            )
            .header("Content-Type", "application/xml; charset=utf-8")
            .header("Depth", "1")
            .body(xml_body)
            .send()
            .await
            .map_err(|e| AdapterError::ExecutionFailed {
                tool_name: "calendar_list_events".into(),
                reason: format!("failed to list events: {e}"),
            })?;

        let body = response
            .text()
            .await
            .map_err(|e| AdapterError::ExecutionFailed {
                tool_name: "calendar_list_events".into(),
                reason: format!("failed to read response: {e}"),
            })?;

        let events = Self::parse_ical_events(&body);

        Ok(json!({
            "success": true,
            "events": events,
            "count": events.len(),
            "range": {
                "start": start_str,
                "end": end_str,
            }
        }))
    }

    /// Create a new calendar event.
    async fn tool_create_event(&self, params: Value) -> Result<Value> {
        let caldav_url = self.resolve_caldav_url(&params)?;
        let username = self.resolve_username(&params);
        let password = self.resolve_password(&params);

        let summary = params
            .get("summary")
            .and_then(|v| v.as_str())
            .ok_or_else(|| AdapterError::InvalidParams {
                tool_name: "calendar_create_event".into(),
                reason: "missing required string field `summary`".into(),
            })?;

        let start = params
            .get("start")
            .and_then(|v| v.as_str())
            .ok_or_else(|| AdapterError::InvalidParams {
                tool_name: "calendar_create_event".into(),
                reason: "missing required string field `start` (ISO 8601)".into(),
            })?;

        let end = params.get("end").and_then(|v| v.as_str()).ok_or_else(|| {
            AdapterError::InvalidParams {
                tool_name: "calendar_create_event".into(),
                reason: "missing required string field `end` (ISO 8601)".into(),
            }
        })?;

        let description = params.get("description").and_then(|v| v.as_str());
        let location = params.get("location").and_then(|v| v.as_str());

        let uid = Uuid::new_v4().to_string();
        let ical_body = Self::generate_ical_event(&uid, summary, start, end, description, location);

        let event_url = format!("{}/{}.ics", caldav_url.trim_end_matches('/'), uid);

        debug!(url = %event_url, summary = %summary, "creating calendar event");

        let response = self
            .build_request(
                reqwest::Method::PUT,
                &event_url,
                username.as_deref(),
                password.as_deref(),
            )
            .header("Content-Type", "text/calendar; charset=utf-8")
            .body(ical_body)
            .send()
            .await
            .map_err(|e| AdapterError::ExecutionFailed {
                tool_name: "calendar_create_event".into(),
                reason: format!("failed to create event: {e}"),
            })?;

        let status = response.status().as_u16();

        Ok(json!({
            "success": status == 201 || status == 204 || status == 200,
            "uid": uid,
            "url": event_url,
            "status": status,
        }))
    }

    /// Delete a calendar event by UID.
    async fn tool_delete_event(&self, params: Value) -> Result<Value> {
        let caldav_url = self.resolve_caldav_url(&params)?;
        let username = self.resolve_username(&params);
        let password = self.resolve_password(&params);

        let uid = params.get("uid").and_then(|v| v.as_str()).ok_or_else(|| {
            AdapterError::InvalidParams {
                tool_name: "calendar_delete_event".into(),
                reason: "missing required string field `uid`".into(),
            }
        })?;

        let event_url = format!("{}/{}.ics", caldav_url.trim_end_matches('/'), uid);

        debug!(url = %event_url, uid = %uid, "deleting calendar event");

        let response = self
            .build_request(
                reqwest::Method::DELETE,
                &event_url,
                username.as_deref(),
                password.as_deref(),
            )
            .send()
            .await
            .map_err(|e| AdapterError::ExecutionFailed {
                tool_name: "calendar_delete_event".into(),
                reason: format!("failed to delete event: {e}"),
            })?;

        let status = response.status().as_u16();

        Ok(json!({
            "success": status == 204 || status == 200,
            "uid": uid,
            "status": status,
        }))
    }

    /// Search events by text query.
    async fn tool_search_events(&self, params: Value) -> Result<Value> {
        let caldav_url = self.resolve_caldav_url(&params)?;
        let username = self.resolve_username(&params);
        let password = self.resolve_password(&params);

        let query = params
            .get("query")
            .and_then(|v| v.as_str())
            .ok_or_else(|| AdapterError::InvalidParams {
                tool_name: "calendar_search_events".into(),
                reason: "missing required string field `query`".into(),
            })?;

        let days_ahead = params
            .get("days_ahead")
            .and_then(|v| v.as_i64())
            .unwrap_or(DEFAULT_DAYS_AHEAD);

        let now = Utc::now();
        let end = now + Duration::days(days_ahead);
        let start_str = Self::format_caldav_datetime(&now);
        let end_str = Self::format_caldav_datetime(&end);

        let xml_body = Self::build_calendar_query_xml(&start_str, &end_str);

        debug!(url = %caldav_url, query = %query, "searching calendar events");

        let response = self
            .build_request(
                reqwest::Method::from_bytes(b"REPORT").unwrap_or(reqwest::Method::POST),
                &caldav_url,
                username.as_deref(),
                password.as_deref(),
            )
            .header("Content-Type", "application/xml; charset=utf-8")
            .header("Depth", "1")
            .body(xml_body)
            .send()
            .await
            .map_err(|e| AdapterError::ExecutionFailed {
                tool_name: "calendar_search_events".into(),
                reason: format!("failed to search events: {e}"),
            })?;

        let body = response
            .text()
            .await
            .map_err(|e| AdapterError::ExecutionFailed {
                tool_name: "calendar_search_events".into(),
                reason: format!("failed to read response: {e}"),
            })?;

        let all_events = Self::parse_ical_events(&body);

        // Filter events matching the query in summary or description.
        let query_lower = query.to_lowercase();
        let matched: Vec<Value> = all_events
            .into_iter()
            .filter(|evt| {
                let summary = evt.get("summary").and_then(|v| v.as_str()).unwrap_or("");
                let desc = evt
                    .get("description")
                    .and_then(|v| v.as_str())
                    .unwrap_or("");
                summary.to_lowercase().contains(&query_lower)
                    || desc.to_lowercase().contains(&query_lower)
            })
            .collect();

        Ok(json!({
            "success": true,
            "events": matched,
            "count": matched.len(),
            "query": query,
        }))
    }

    /// Get a specific event by UID.
    async fn tool_get_event(&self, params: Value) -> Result<Value> {
        let caldav_url = self.resolve_caldav_url(&params)?;
        let username = self.resolve_username(&params);
        let password = self.resolve_password(&params);

        let uid = params.get("uid").and_then(|v| v.as_str()).ok_or_else(|| {
            AdapterError::InvalidParams {
                tool_name: "calendar_get_event".into(),
                reason: "missing required string field `uid`".into(),
            }
        })?;

        let event_url = format!("{}/{}.ics", caldav_url.trim_end_matches('/'), uid);

        debug!(url = %event_url, uid = %uid, "getting calendar event");

        let response = self
            .build_request(
                reqwest::Method::GET,
                &event_url,
                username.as_deref(),
                password.as_deref(),
            )
            .send()
            .await
            .map_err(|e| AdapterError::ExecutionFailed {
                tool_name: "calendar_get_event".into(),
                reason: format!("failed to get event: {e}"),
            })?;

        let status = response.status().as_u16();
        let body = response
            .text()
            .await
            .map_err(|e| AdapterError::ExecutionFailed {
                tool_name: "calendar_get_event".into(),
                reason: format!("failed to read response: {e}"),
            })?;

        if status == 404 {
            return Ok(json!({
                "success": false,
                "error": "event not found",
                "uid": uid,
            }));
        }

        let events = Self::parse_ical_events(&body);
        let event = events.first().cloned().unwrap_or(json!({}));

        Ok(json!({
            "success": true,
            "uid": uid,
            "event": event,
            "raw": body,
        }))
    }
}

// ---------------------------------------------------------------------------
// Adapter trait implementation
// ---------------------------------------------------------------------------

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

    async fn execute_tool(&self, name: &str, params: Value) -> Result<Value> {
        if !self.connected {
            return Err(AdapterError::ExecutionFailed {
                tool_name: name.to_string(),
                reason: format!("adapter `{}` is not connected", self.id),
            });
        }

        match name {
            "calendar_list_events" => self.tool_list_events(params).await,
            "calendar_create_event" => self.tool_create_event(params).await,
            "calendar_delete_event" => self.tool_delete_event(params).await,
            "calendar_search_events" => self.tool_search_events(params).await,
            "calendar_get_event" => self.tool_get_event(params).await,
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

    // -- Construction tests --

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

    // -- Adapter trait basics --

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

    // -- Tool definitions --

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

    // -- Connect / disconnect --

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

    // -- Health check --

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

    // -- iCalendar generation --

    #[test]
    fn generate_ical_event_basic() {
        let ical = CalendarAdapter::generate_ical_event(
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
        let ical = CalendarAdapter::generate_ical_event(
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

    // -- iCalendar datetime formatting --

    #[test]
    fn format_ical_datetime_rfc3339() {
        let result = CalendarAdapter::format_ical_datetime("2026-02-24T10:00:00Z");
        assert_eq!(result, "20260224T100000Z");
    }

    #[test]
    fn format_ical_datetime_naive() {
        let result = CalendarAdapter::format_ical_datetime("2026-02-24T10:00:00");
        assert_eq!(result, "20260224T100000");
    }

    #[test]
    fn format_ical_datetime_date_only() {
        let result = CalendarAdapter::format_ical_datetime("2026-02-24");
        assert_eq!(result, "20260224");
    }

    #[test]
    fn format_ical_datetime_fallback() {
        let result = CalendarAdapter::format_ical_datetime("invalid-date");
        assert_eq!(result, "invalid-date");
    }

    // -- CalDAV XML building --

    #[test]
    fn build_calendar_query_xml_contains_time_range() {
        let xml = CalendarAdapter::build_calendar_query_xml("20260224T000000Z", "20260301T000000Z");
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
        let xml = CalendarAdapter::build_propfind_xml();
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
        let result = CalendarAdapter::format_caldav_datetime(&dt);
        assert_eq!(result, "20260224T153000Z");
    }

    // -- iCal parsing --

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

        let events = CalendarAdapter::parse_ical_events(ical_data);
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

        let events = CalendarAdapter::parse_ical_events(ical_data);
        assert_eq!(events.len(), 2);
        assert_eq!(events[0]["uid"], "event-1");
        assert_eq!(events[1]["uid"], "event-2");
    }

    #[test]
    fn parse_ical_events_handles_empty_input() {
        let events = CalendarAdapter::parse_ical_events("");
        assert!(events.is_empty());
    }

    #[test]
    fn parse_ical_events_strips_parameter_parts() {
        let ical_data = "\
BEGIN:VEVENT\r\n\
DTSTART;VALUE=DATE:20260224\r\n\
SUMMARY:Date-only Event\r\n\
END:VEVENT\r\n";

        let events = CalendarAdapter::parse_ical_events(ical_data);
        assert_eq!(events.len(), 1);
        assert_eq!(events[0]["dtstart"], "20260224");
    }

    // -- Credential resolution --

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

    // -- Execute tool when not connected --

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

    // -- Execute tool rejects unknown tool --

    #[tokio::test]
    async fn execute_tool_rejects_unknown_tool() {
        let mut adapter = CalendarAdapter::new("cal");
        adapter.connected = true;
        let result = adapter.execute_tool("nonexistent_tool", json!({})).await;
        assert!(result.is_err());
        let err = result.unwrap_err();
        assert!(err.to_string().contains("tool not found"));
    }

    // -- Missing required parameters --

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
