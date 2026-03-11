//! CalDAV HTTP request helpers and iCalendar format utilities (RFC 5545).
//!
//! Provides XML builders for CalDAV REPORT/PROPFIND, iCal generation, and
//! iCal parsing for use by the CalendarAdapter tool implementations.

use chrono::{Duration, Utc};
use serde_json::{Value, json};
use tracing::debug;
use uuid::Uuid;

use crate::error::{AdapterError, Result};

/// Default number of days ahead to look for events.
pub const DEFAULT_DAYS_AHEAD: i64 = 7;

// ---------------------------------------------------------------------------
// iCalendar format helpers (RFC 5545)
// ---------------------------------------------------------------------------

/// Generate an iCalendar VCALENDAR string for a new event.
///
/// The `start` and `end` parameters should be ISO 8601 formatted strings.
pub fn generate_ical_event(
    uid: &str,
    summary: &str,
    start: &str,
    end: &str,
    description: Option<&str>,
    location: Option<&str>,
) -> String {
    let dtstart = format_ical_datetime(start);
    let dtend = format_ical_datetime(end);

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
    if let Ok(dt) = chrono::DateTime::parse_from_rfc3339(iso) {
        return dt.format("%Y%m%dT%H%M%SZ").to_string();
    }
    if let Ok(dt) = chrono::NaiveDateTime::parse_from_str(iso, "%Y-%m-%dT%H:%M:%S") {
        return dt.format("%Y%m%dT%H%M%S").to_string();
    }
    if let Ok(dt) = chrono::NaiveDate::parse_from_str(iso, "%Y-%m-%d") {
        return dt.format("%Y%m%d").to_string();
    }
    iso.to_string()
}

/// Format a chrono DateTime as a CalDAV time-range value (yyyymmddThhmmssZ).
pub fn format_caldav_datetime(dt: &chrono::DateTime<chrono::Utc>) -> String {
    dt.format("%Y%m%dT%H%M%SZ").to_string()
}

// ---------------------------------------------------------------------------
// CalDAV XML request builders
// ---------------------------------------------------------------------------

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

// ---------------------------------------------------------------------------
// iCal response parsing
// ---------------------------------------------------------------------------

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

// ---------------------------------------------------------------------------
// HTTP helpers
// ---------------------------------------------------------------------------

/// Build a CalDAV HTTP request with optional basic auth.
pub fn build_request(
    client: &reqwest::Client,
    method: reqwest::Method,
    url: &str,
    username: Option<&str>,
    password: Option<&str>,
) -> reqwest::RequestBuilder {
    let mut builder = client.request(method, url);
    if let (Some(user), Some(pass)) = (username, password) {
        builder = builder.basic_auth(user, Some(pass));
    }
    builder
}

// ---------------------------------------------------------------------------
// Tool implementations
// ---------------------------------------------------------------------------

/// List upcoming calendar events.
pub async fn tool_list_events(
    client: &reqwest::Client,
    caldav_url: &str,
    username: Option<&str>,
    password: Option<&str>,
    params: &Value,
) -> Result<Value> {
    let days_ahead = params
        .get("days_ahead")
        .and_then(|v| v.as_i64())
        .unwrap_or(DEFAULT_DAYS_AHEAD);

    let now = Utc::now();
    let end = now + Duration::days(days_ahead);
    let start_str = format_caldav_datetime(&now);
    let end_str = format_caldav_datetime(&end);

    let xml_body = build_calendar_query_xml(&start_str, &end_str);

    debug!(url = %caldav_url, days_ahead = days_ahead, "listing calendar events");

    let response = build_request(
        client,
        reqwest::Method::from_bytes(b"REPORT").unwrap_or(reqwest::Method::POST),
        caldav_url,
        username,
        password,
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

    let events = parse_ical_events(&body);
    let count = events.len();

    Ok(json!({
        "success": true,
        "events": events,
        "count": count,
        "range": {
            "start": start_str,
            "end": end_str,
        }
    }))
}

/// Create a new calendar event.
pub async fn tool_create_event(
    client: &reqwest::Client,
    caldav_url: &str,
    username: Option<&str>,
    password: Option<&str>,
    params: &Value,
) -> Result<Value> {
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
    let ical_body = generate_ical_event(&uid, summary, start, end, description, location);

    let event_url = format!("{}/{}.ics", caldav_url.trim_end_matches('/'), uid);

    debug!(url = %event_url, summary = %summary, "creating calendar event");

    let response = build_request(client, reqwest::Method::PUT, &event_url, username, password)
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
pub async fn tool_delete_event(
    client: &reqwest::Client,
    caldav_url: &str,
    username: Option<&str>,
    password: Option<&str>,
    params: &Value,
) -> Result<Value> {
    let uid = params.get("uid").and_then(|v| v.as_str()).ok_or_else(|| {
        AdapterError::InvalidParams {
            tool_name: "calendar_delete_event".into(),
            reason: "missing required string field `uid`".into(),
        }
    })?;

    let event_url = format!("{}/{}.ics", caldav_url.trim_end_matches('/'), uid);

    debug!(url = %event_url, uid = %uid, "deleting calendar event");

    let response = build_request(
        client,
        reqwest::Method::DELETE,
        &event_url,
        username,
        password,
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
pub async fn tool_search_events(
    client: &reqwest::Client,
    caldav_url: &str,
    username: Option<&str>,
    password: Option<&str>,
    params: &Value,
) -> Result<Value> {
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
    let start_str = format_caldav_datetime(&now);
    let end_str = format_caldav_datetime(&end);

    let xml_body = build_calendar_query_xml(&start_str, &end_str);

    debug!(url = %caldav_url, query = %query, "searching calendar events");

    let response = build_request(
        client,
        reqwest::Method::from_bytes(b"REPORT").unwrap_or(reqwest::Method::POST),
        caldav_url,
        username,
        password,
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

    let all_events = parse_ical_events(&body);

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

    let count = matched.len();

    Ok(json!({
        "success": true,
        "events": matched,
        "count": count,
        "query": query,
    }))
}

/// Get a specific event by UID.
pub async fn tool_get_event(
    client: &reqwest::Client,
    caldav_url: &str,
    username: Option<&str>,
    password: Option<&str>,
    params: &Value,
) -> Result<Value> {
    let uid = params.get("uid").and_then(|v| v.as_str()).ok_or_else(|| {
        AdapterError::InvalidParams {
            tool_name: "calendar_get_event".into(),
            reason: "missing required string field `uid`".into(),
        }
    })?;

    let event_url = format!("{}/{}.ics", caldav_url.trim_end_matches('/'), uid);

    debug!(url = %event_url, uid = %uid, "getting calendar event");

    let response = build_request(client, reqwest::Method::GET, &event_url, username, password)
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

    let events = parse_ical_events(&body);
    let event = events.first().cloned().unwrap_or(json!({}));

    Ok(json!({
        "success": true,
        "uid": uid,
        "event": event,
        "raw": body,
    }))
}
