//! Email adapter -- read emails via IMAP and send emails via SMTP.
//!
//! This adapter provides four tools for email interaction:
//! - `email_list_inbox` — List recent emails from the inbox
//! - `email_read` — Read a specific email by sequence number
//! - `email_send` — Send an email via SMTP
//! - `email_search` — Search emails by IMAP query

pub mod imap;
pub mod smtp;
pub mod tools;
pub mod types;

use async_trait::async_trait;
use serde_json::{Value, json};
use tokio::io::AsyncWriteExt;
use tracing::info;

use crate::error::{AdapterError, Result};
use crate::traits::{Adapter, AdapterType, AuthRequirement, HealthStatus, ToolDefinition};

use imap::{
    connect_tls, imap_fetch_body_command, imap_fetch_envelope_command, imap_login_command,
    imap_logout_command, imap_read_response, imap_search_command, imap_select_command,
    parse_exists_count, parse_fetch_body, parse_fetch_envelopes, parse_search_results,
};
use smtp::send_email;

/// Default IMAP TLS port.
const DEFAULT_IMAP_PORT: u16 = 993;

/// Default SMTP TLS port.
const DEFAULT_SMTP_PORT: u16 = 465;

/// Default number of inbox messages to list.
const DEFAULT_LIST_COUNT: u64 = 10;

/// Email service adapter for IMAP reading and SMTP sending.
pub struct EmailAdapter {
    id: String,
    connected: bool,
    imap_host: String,
    imap_port: u16,
    smtp_host: String,
    smtp_port: u16,
}

impl EmailAdapter {
    /// Create a new email adapter with default ports.
    pub fn new(id: &str) -> Self {
        Self {
            id: id.to_string(),
            connected: false,
            imap_host: String::new(),
            imap_port: DEFAULT_IMAP_PORT,
            smtp_host: String::new(),
            smtp_port: DEFAULT_SMTP_PORT,
        }
    }

    /// Create a new email adapter with custom server configuration.
    pub fn with_config(
        id: &str,
        imap_host: &str,
        imap_port: u16,
        smtp_host: &str,
        smtp_port: u16,
    ) -> Self {
        Self {
            id: id.to_string(),
            connected: false,
            imap_host: imap_host.to_string(),
            imap_port,
            smtp_host: smtp_host.to_string(),
            smtp_port,
        }
    }

    /// Resolve the IMAP host: use the per-call host override, fall back to
    /// the adapter-level host, or return an error.
    pub fn resolve_imap_host<'a>(&'a self, params: &'a Value, tool_name: &str) -> Result<&'a str> {
        if let Some(host) = params.get("host").and_then(|v| v.as_str()) {
            return Ok(host);
        }
        if !self.imap_host.is_empty() {
            return Ok(&self.imap_host);
        }
        Err(AdapterError::InvalidParams {
            tool_name: tool_name.into(),
            reason: "missing `host` parameter and no default IMAP host configured".into(),
        })
    }

    /// Resolve the SMTP host: use the per-call host override, fall back to
    /// the adapter-level host, or return an error.
    pub fn resolve_smtp_host<'a>(&'a self, params: &'a Value, tool_name: &str) -> Result<&'a str> {
        if let Some(host) = params.get("host").and_then(|v| v.as_str()) {
            return Ok(host);
        }
        if !self.smtp_host.is_empty() {
            return Ok(&self.smtp_host);
        }
        Err(AdapterError::InvalidParams {
            tool_name: tool_name.into(),
            reason: "missing `host` parameter and no default SMTP host configured".into(),
        })
    }

    fn extract_username<'a>(&self, params: &'a Value, tool_name: &str) -> Result<&'a str> {
        params
            .get("username")
            .and_then(|v| v.as_str())
            .ok_or_else(|| AdapterError::InvalidParams {
                tool_name: tool_name.into(),
                reason: "missing required string field `username`".into(),
            })
    }

    fn extract_password<'a>(&self, params: &'a Value, tool_name: &str) -> Result<&'a str> {
        params
            .get("password")
            .and_then(|v| v.as_str())
            .ok_or_else(|| AdapterError::InvalidParams {
                tool_name: tool_name.into(),
                reason: "missing required string field `password`".into(),
            })
    }

    // -----------------------------------------------------------------------
    // Tool implementations
    // -----------------------------------------------------------------------

    async fn tool_email_list_inbox(&self, params: Value) -> Result<Value> {
        let tool_name = "email_list_inbox";
        let host = self.resolve_imap_host(&params, tool_name)?;
        let username = self.extract_username(&params, tool_name)?;
        let password = self.extract_password(&params, tool_name)?;
        let count = params
            .get("count")
            .and_then(|v| v.as_u64())
            .unwrap_or(DEFAULT_LIST_COUNT);

        info!(host = host, username = username, count = count, "listing inbox emails");

        let tls_stream = connect_tls(host, self.imap_port).await?;
        let (read_half, mut write_half) = tokio::io::split(tls_stream);
        let mut reader = tokio::io::BufReader::new(read_half);

        let _greeting = imap_read_response(&mut reader, "*").await?;

        let login_cmd = imap_login_command("A001", username, password);
        write_half.write_all(login_cmd.as_bytes()).await.map_err(|e| AdapterError::ExecutionFailed {
            tool_name: tool_name.into(),
            reason: format!("IMAP write error: {e}"),
        })?;
        let _login_resp = imap_read_response(&mut reader, "A001").await?;

        let select_cmd = imap_select_command("A002", "INBOX");
        write_half.write_all(select_cmd.as_bytes()).await.map_err(|e| AdapterError::ExecutionFailed {
            tool_name: tool_name.into(),
            reason: format!("IMAP write error: {e}"),
        })?;
        let select_resp = imap_read_response(&mut reader, "A002").await?;

        let exists = parse_exists_count(&select_resp).unwrap_or(0);

        if exists == 0 {
            let logout_cmd = imap_logout_command("A003");
            let _ = write_half.write_all(logout_cmd.as_bytes()).await;
            return Ok(json!({ "emails": [], "total": 0, "fetched": 0 }));
        }

        let start = if exists > count { exists - count + 1 } else { 1 };
        let sequence_set = format!("{start}:{exists}");

        let fetch_cmd = imap_fetch_envelope_command("A003", &sequence_set);
        write_half.write_all(fetch_cmd.as_bytes()).await.map_err(|e| AdapterError::ExecutionFailed {
            tool_name: tool_name.into(),
            reason: format!("IMAP write error: {e}"),
        })?;
        let fetch_resp = imap_read_response(&mut reader, "A003").await?;

        let emails = parse_fetch_envelopes(&fetch_resp);

        let logout_cmd = imap_logout_command("A004");
        let _ = write_half.write_all(logout_cmd.as_bytes()).await;

        Ok(json!({ "emails": emails, "total": exists, "fetched": emails.len() }))
    }

    async fn tool_email_read(&self, params: Value) -> Result<Value> {
        let tool_name = "email_read";
        let host = self.resolve_imap_host(&params, tool_name)?;
        let username = self.extract_username(&params, tool_name)?;
        let password = self.extract_password(&params, tool_name)?;
        let message_id = params
            .get("message_id")
            .and_then(|v| v.as_str())
            .ok_or_else(|| AdapterError::InvalidParams {
                tool_name: tool_name.into(),
                reason: "missing required string field `message_id`".into(),
            })?;

        info!(host = host, message_id = message_id, "reading email");

        let tls_stream = connect_tls(host, self.imap_port).await?;
        let (read_half, mut write_half) = tokio::io::split(tls_stream);
        let mut reader = tokio::io::BufReader::new(read_half);

        let _greeting = imap_read_response(&mut reader, "*").await?;

        let login_cmd = imap_login_command("A001", username, password);
        write_half.write_all(login_cmd.as_bytes()).await.map_err(|e| AdapterError::ExecutionFailed {
            tool_name: tool_name.into(),
            reason: format!("IMAP write error: {e}"),
        })?;
        let _login_resp = imap_read_response(&mut reader, "A001").await?;

        let select_cmd = imap_select_command("A002", "INBOX");
        write_half.write_all(select_cmd.as_bytes()).await.map_err(|e| AdapterError::ExecutionFailed {
            tool_name: tool_name.into(),
            reason: format!("IMAP write error: {e}"),
        })?;
        let _select_resp = imap_read_response(&mut reader, "A002").await?;

        let fetch_cmd = imap_fetch_body_command("A003", message_id);
        write_half.write_all(fetch_cmd.as_bytes()).await.map_err(|e| AdapterError::ExecutionFailed {
            tool_name: tool_name.into(),
            reason: format!("IMAP write error: {e}"),
        })?;
        let fetch_resp = imap_read_response(&mut reader, "A003").await?;

        let (headers, body) = parse_fetch_body(&fetch_resp);

        let logout_cmd = imap_logout_command("A004");
        let _ = write_half.write_all(logout_cmd.as_bytes()).await;

        Ok(json!({ "message_id": message_id, "headers": headers, "body": body }))
    }

    async fn tool_email_send(&self, params: Value) -> Result<Value> {
        let tool_name = "email_send";
        let host = self.resolve_smtp_host(&params, tool_name)?;
        let username = self.extract_username(&params, tool_name)?;
        let password = self.extract_password(&params, tool_name)?;

        let to = params.get("to").and_then(|v| v.as_str()).ok_or_else(|| {
            AdapterError::InvalidParams {
                tool_name: tool_name.into(),
                reason: "missing required string field `to`".into(),
            }
        })?;

        let subject = params.get("subject").and_then(|v| v.as_str()).ok_or_else(|| {
            AdapterError::InvalidParams {
                tool_name: tool_name.into(),
                reason: "missing required string field `subject`".into(),
            }
        })?;

        let body = params.get("body").and_then(|v| v.as_str()).ok_or_else(|| {
            AdapterError::InvalidParams {
                tool_name: tool_name.into(),
                reason: "missing required string field `body`".into(),
            }
        })?;

        info!(host = host, to = to, subject = subject, "sending email");

        send_email(host, self.smtp_port, username, password, to, subject, body).await?;

        Ok(json!({ "status": "sent", "to": to, "subject": subject }))
    }

    async fn tool_email_search(&self, params: Value) -> Result<Value> {
        let tool_name = "email_search";
        let host = self.resolve_imap_host(&params, tool_name)?;
        let username = self.extract_username(&params, tool_name)?;
        let password = self.extract_password(&params, tool_name)?;

        let query = params
            .get("query")
            .and_then(|v| v.as_str())
            .ok_or_else(|| AdapterError::InvalidParams {
                tool_name: tool_name.into(),
                reason: "missing required string field `query`".into(),
            })?;

        let count = params
            .get("count")
            .and_then(|v| v.as_u64())
            .unwrap_or(DEFAULT_LIST_COUNT);

        info!(host = host, query = query, count = count, "searching emails");

        let tls_stream = connect_tls(host, self.imap_port).await?;
        let (read_half, mut write_half) = tokio::io::split(tls_stream);
        let mut reader = tokio::io::BufReader::new(read_half);

        let _greeting = imap_read_response(&mut reader, "*").await?;

        let login_cmd = imap_login_command("A001", username, password);
        write_half.write_all(login_cmd.as_bytes()).await.map_err(|e| AdapterError::ExecutionFailed {
            tool_name: tool_name.into(),
            reason: format!("IMAP write error: {e}"),
        })?;
        let _login_resp = imap_read_response(&mut reader, "A001").await?;

        let select_cmd = imap_select_command("A002", "INBOX");
        write_half.write_all(select_cmd.as_bytes()).await.map_err(|e| AdapterError::ExecutionFailed {
            tool_name: tool_name.into(),
            reason: format!("IMAP write error: {e}"),
        })?;
        let _select_resp = imap_read_response(&mut reader, "A002").await?;

        let search_cmd = imap_search_command("A003", query);
        write_half.write_all(search_cmd.as_bytes()).await.map_err(|e| AdapterError::ExecutionFailed {
            tool_name: tool_name.into(),
            reason: format!("IMAP write error: {e}"),
        })?;
        let search_resp = imap_read_response(&mut reader, "A003").await?;

        let mut matching_ids = parse_search_results(&search_resp);
        let total_matches = matching_ids.len();

        if matching_ids.len() > count as usize {
            let start = matching_ids.len() - count as usize;
            matching_ids = matching_ids[start..].to_vec();
        }

        let emails = if matching_ids.is_empty() {
            Vec::new()
        } else {
            let sequence_set = matching_ids
                .iter()
                .map(|id| id.to_string())
                .collect::<Vec<_>>()
                .join(",");

            let fetch_cmd = imap_fetch_envelope_command("A004", &sequence_set);
            write_half.write_all(fetch_cmd.as_bytes()).await.map_err(|e| AdapterError::ExecutionFailed {
                tool_name: tool_name.into(),
                reason: format!("IMAP write error: {e}"),
            })?;
            let fetch_resp = imap_read_response(&mut reader, "A004").await?;
            parse_fetch_envelopes(&fetch_resp)
        };

        let logout_tag = if matching_ids.is_empty() { "A004" } else { "A005" };
        let logout_cmd = imap_logout_command(logout_tag);
        let _ = write_half.write_all(logout_cmd.as_bytes()).await;

        Ok(json!({
            "query": query,
            "total_matches": total_matches,
            "emails": emails,
            "fetched": emails.len(),
        }))
    }
}

#[async_trait]
impl Adapter for EmailAdapter {
    fn id(&self) -> &str {
        &self.id
    }

    fn adapter_type(&self) -> AdapterType {
        AdapterType::Productivity
    }

    async fn connect(&mut self) -> Result<()> {
        info!(id = %self.id, "email adapter connected");
        self.connected = true;
        Ok(())
    }

    async fn disconnect(&mut self) -> Result<()> {
        info!(id = %self.id, "email adapter disconnected");
        self.connected = false;
        Ok(())
    }

    async fn health_check(&self) -> Result<HealthStatus> {
        if !self.connected {
            return Ok(HealthStatus::Unhealthy);
        }
        Ok(HealthStatus::Healthy)
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

        match name {
            "email_list_inbox" => self.tool_email_list_inbox(params).await,
            "email_read" => self.tool_email_read(params).await,
            "email_send" => self.tool_email_send(params).await,
            "email_search" => self.tool_email_search(params).await,
            _ => Err(AdapterError::ToolNotFound {
                adapter_id: self.id.clone(),
                tool_name: name.to_string(),
            }),
        }
    }

    fn required_auth(&self) -> Option<AuthRequirement> {
        None
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use base64::Engine;
    use crate::email::imap::{
        imap_login_command, imap_select_command, imap_fetch_envelope_command,
        imap_fetch_body_command, imap_search_command, imap_logout_command,
        parse_exists_count, parse_search_results, parse_fetch_body,
    };
    use crate::email::smtp::{
        smtp_ehlo_command, smtp_auth_login_command, smtp_mail_from_command,
        smtp_rcpt_to_command, smtp_data_command, smtp_quit_command,
        smtp_message_body, smtp_base64_encode,
    };

    #[test]
    fn email_adapter_new_default_ports() {
        let adapter = EmailAdapter::new("email-test");
        assert_eq!(adapter.id, "email-test");
        assert_eq!(adapter.imap_port, 993);
        assert_eq!(adapter.smtp_port, 465);
        assert!(!adapter.connected);
        assert!(adapter.imap_host.is_empty());
        assert!(adapter.smtp_host.is_empty());
    }

    #[test]
    fn email_adapter_with_config() {
        let adapter = EmailAdapter::with_config(
            "email-cfg",
            "imap.example.com",
            1993,
            "smtp.example.com",
            1465,
        );
        assert_eq!(adapter.id, "email-cfg");
        assert_eq!(adapter.imap_host, "imap.example.com");
        assert_eq!(adapter.imap_port, 1993);
        assert_eq!(adapter.smtp_host, "smtp.example.com");
        assert_eq!(adapter.smtp_port, 1465);
        assert!(!adapter.connected);
    }

    #[test]
    fn email_adapter_id() {
        let adapter = EmailAdapter::new("my-email");
        assert_eq!(adapter.id(), "my-email");
    }

    #[test]
    fn email_adapter_type() {
        let adapter = EmailAdapter::new("email-test");
        assert_eq!(adapter.adapter_type(), AdapterType::Productivity);
    }

    #[test]
    fn email_adapter_required_auth_is_none() {
        let adapter = EmailAdapter::new("email-test");
        assert!(adapter.required_auth().is_none());
    }

    #[test]
    fn email_adapter_tools_count() {
        let adapter = EmailAdapter::new("email-test");
        let tools = adapter.tools();
        assert_eq!(tools.len(), 4);
    }

    #[test]
    fn email_adapter_tool_names() {
        let adapter = EmailAdapter::new("email-test");
        let tools = adapter.tools();
        let names: Vec<&str> = tools.iter().map(|t| t.name.as_str()).collect();
        assert!(names.contains(&"email_list_inbox"));
        assert!(names.contains(&"email_read"));
        assert!(names.contains(&"email_send"));
        assert!(names.contains(&"email_search"));
    }

    #[test]
    fn email_adapter_tool_definitions_have_parameters() {
        let adapter = EmailAdapter::new("email-test");
        for tool in adapter.tools() {
            let params = &tool.parameters;
            assert_eq!(params.get("type").and_then(|v| v.as_str()), Some("object"));
            assert!(params.get("properties").is_some());
            assert!(params.get("required").is_some());
        }
    }

    #[tokio::test]
    async fn email_adapter_connect_disconnect() {
        let mut adapter = EmailAdapter::new("email-test");
        assert!(!adapter.connected);
        adapter.connect().await.unwrap();
        assert!(adapter.connected);
        adapter.disconnect().await.unwrap();
        assert!(!adapter.connected);
    }

    #[tokio::test]
    async fn email_adapter_health_when_disconnected() {
        let adapter = EmailAdapter::new("email-test");
        let status = adapter.health_check().await.unwrap();
        assert_eq!(status, HealthStatus::Unhealthy);
    }

    #[tokio::test]
    async fn email_adapter_health_when_connected() {
        let mut adapter = EmailAdapter::new("email-test");
        adapter.connect().await.unwrap();
        let status = adapter.health_check().await.unwrap();
        assert_eq!(status, HealthStatus::Healthy);
    }

    #[tokio::test]
    async fn email_adapter_rejects_when_not_connected() {
        let adapter = EmailAdapter::new("email-test");
        let result = adapter
            .execute_tool(
                "email_list_inbox",
                json!({"username": "a", "password": "b", "host": "imap.example.com"}),
            )
            .await;
        assert!(result.is_err());
        let err = result.unwrap_err().to_string();
        assert!(err.contains("not connected"), "error should mention not connected: {err}");
    }

    #[tokio::test]
    async fn email_adapter_rejects_unknown_tool() {
        let mut adapter = EmailAdapter::new("email-test");
        adapter.connect().await.unwrap();
        let result = adapter.execute_tool("nonexistent", json!({})).await;
        assert!(result.is_err());
        let err = result.unwrap_err().to_string();
        assert!(err.contains("nonexistent"), "error should mention the tool name: {err}");
    }

    #[test]
    fn imap_login_command_format() {
        let cmd = imap_login_command("A001", "user@example.com", "secret");
        assert_eq!(cmd, "A001 LOGIN \"user@example.com\" \"secret\"\r\n");
    }

    #[test]
    fn imap_select_command_format() {
        let cmd = imap_select_command("A002", "INBOX");
        assert_eq!(cmd, "A002 SELECT \"INBOX\"\r\n");
    }

    #[test]
    fn imap_fetch_envelope_command_format() {
        let cmd = imap_fetch_envelope_command("A003", "1:10");
        assert_eq!(cmd, "A003 FETCH 1:10 (FLAGS ENVELOPE BODY.PEEK[HEADER.FIELDS (MESSAGE-ID)])\r\n");
    }

    #[test]
    fn imap_fetch_body_command_format() {
        let cmd = imap_fetch_body_command("A003", "5");
        assert_eq!(cmd, "A003 FETCH 5 (FLAGS ENVELOPE BODY[TEXT] BODY[HEADER])\r\n");
    }

    #[test]
    fn imap_search_command_format() {
        let cmd = imap_search_command("A003", "FROM \"user@example.com\"");
        assert_eq!(cmd, "A003 SEARCH FROM \"user@example.com\"\r\n");
    }

    #[test]
    fn imap_logout_command_format() {
        let cmd = imap_logout_command("A004");
        assert_eq!(cmd, "A004 LOGOUT\r\n");
    }

    #[test]
    fn smtp_ehlo_command_format() {
        let cmd = smtp_ehlo_command("openintentos.local");
        assert_eq!(cmd, "EHLO openintentos.local\r\n");
    }

    #[test]
    fn smtp_auth_login_command_format() {
        let cmd = smtp_auth_login_command();
        assert_eq!(cmd, "AUTH LOGIN\r\n");
    }

    #[test]
    fn smtp_mail_from_command_format() {
        let cmd = smtp_mail_from_command("sender@example.com");
        assert_eq!(cmd, "MAIL FROM:<sender@example.com>\r\n");
    }

    #[test]
    fn smtp_rcpt_to_command_format() {
        let cmd = smtp_rcpt_to_command("recipient@example.com");
        assert_eq!(cmd, "RCPT TO:<recipient@example.com>\r\n");
    }

    #[test]
    fn smtp_data_command_format() {
        let cmd = smtp_data_command();
        assert_eq!(cmd, "DATA\r\n");
    }

    #[test]
    fn smtp_quit_command_format() {
        let cmd = smtp_quit_command();
        assert_eq!(cmd, "QUIT\r\n");
    }

    #[test]
    fn smtp_message_body_format() {
        let msg = smtp_message_body("from@x.com", "to@y.com", "Hello", "Test body");
        assert!(msg.contains("From: from@x.com\r\n"));
        assert!(msg.contains("To: to@y.com\r\n"));
        assert!(msg.contains("Subject: Hello\r\n"));
        assert!(msg.contains("MIME-Version: 1.0\r\n"));
        assert!(msg.contains("Content-Type: text/plain; charset=UTF-8\r\n"));
        assert!(msg.contains("Test body\r\n.\r\n"));
    }

    #[test]
    fn smtp_base64_encode_username() {
        let encoded = smtp_base64_encode("user@example.com");
        let decoded_bytes = base64::engine::general_purpose::STANDARD
            .decode(&encoded)
            .unwrap();
        let decoded = String::from_utf8(decoded_bytes).unwrap();
        assert_eq!(decoded, "user@example.com");
    }

    #[test]
    fn smtp_base64_encode_empty() {
        let encoded = smtp_base64_encode("");
        assert_eq!(encoded, "");
    }

    #[test]
    fn parse_exists_count_from_select_response() {
        let lines = vec![
            "* FLAGS (\\Answered \\Flagged \\Deleted \\Seen \\Draft)".to_string(),
            "* 42 EXISTS".to_string(),
            "* 0 RECENT".to_string(),
            "A002 OK [READ-WRITE] SELECT completed".to_string(),
        ];
        assert_eq!(parse_exists_count(&lines), Some(42));
    }

    #[test]
    fn parse_exists_count_empty_mailbox() {
        let lines = vec![
            "* 0 EXISTS".to_string(),
            "A002 OK SELECT completed".to_string(),
        ];
        assert_eq!(parse_exists_count(&lines), Some(0));
    }

    #[test]
    fn parse_search_results_with_matches() {
        let lines = vec![
            "* SEARCH 1 4 7 12 15".to_string(),
            "A003 OK SEARCH completed".to_string(),
        ];
        let results = parse_search_results(&lines);
        assert_eq!(results, vec![1, 4, 7, 12, 15]);
    }

    #[test]
    fn parse_search_results_no_matches() {
        let lines = vec![
            "* SEARCH".to_string(),
            "A003 OK SEARCH completed".to_string(),
        ];
        let results = parse_search_results(&lines);
        assert!(results.is_empty());
    }

    #[tokio::test]
    async fn email_list_inbox_missing_username_fails() {
        let mut adapter = EmailAdapter::with_config(
            "email-test", "imap.example.com", 993, "smtp.example.com", 465,
        );
        adapter.connect().await.unwrap();
        let result = adapter.execute_tool("email_list_inbox", json!({"password": "secret"})).await;
        assert!(result.is_err());
        let err = result.unwrap_err().to_string();
        assert!(err.contains("username"), "error should mention username: {err}");
    }

    #[tokio::test]
    async fn email_list_inbox_missing_password_fails() {
        let mut adapter = EmailAdapter::with_config(
            "email-test", "imap.example.com", 993, "smtp.example.com", 465,
        );
        adapter.connect().await.unwrap();
        let result = adapter.execute_tool("email_list_inbox", json!({"username": "user"})).await;
        assert!(result.is_err());
        let err = result.unwrap_err().to_string();
        assert!(err.contains("password"), "error should mention password: {err}");
    }

    #[tokio::test]
    async fn email_send_missing_to_field_fails() {
        let mut adapter = EmailAdapter::with_config(
            "email-test", "imap.example.com", 993, "smtp.example.com", 465,
        );
        adapter.connect().await.unwrap();
        let result = adapter
            .execute_tool("email_send", json!({"subject": "Test", "body": "Hello", "username": "user", "password": "pass"}))
            .await;
        assert!(result.is_err());
        let err = result.unwrap_err().to_string();
        assert!(err.contains("to"), "error should mention 'to': {err}");
    }

    #[tokio::test]
    async fn email_search_missing_query_fails() {
        let mut adapter = EmailAdapter::with_config(
            "email-test", "imap.example.com", 993, "smtp.example.com", 465,
        );
        adapter.connect().await.unwrap();
        let result = adapter
            .execute_tool("email_search", json!({"username": "user", "password": "pass"}))
            .await;
        assert!(result.is_err());
        let err = result.unwrap_err().to_string();
        assert!(err.contains("query"), "error should mention 'query': {err}");
    }

    #[tokio::test]
    async fn email_read_missing_message_id_fails() {
        let mut adapter = EmailAdapter::with_config(
            "email-test", "imap.example.com", 993, "smtp.example.com", 465,
        );
        adapter.connect().await.unwrap();
        let result = adapter
            .execute_tool("email_read", json!({"username": "user", "password": "pass"}))
            .await;
        assert!(result.is_err());
        let err = result.unwrap_err().to_string();
        assert!(err.contains("message_id"), "error should mention 'message_id': {err}");
    }

    #[test]
    fn resolve_imap_host_from_params() {
        let adapter = EmailAdapter::new("test");
        let params = json!({"host": "imap.gmail.com"});
        let host = adapter.resolve_imap_host(&params, "test_tool").unwrap();
        assert_eq!(host, "imap.gmail.com");
    }

    #[test]
    fn resolve_imap_host_from_config() {
        let adapter = EmailAdapter::with_config(
            "test", "imap.configured.com", 993, "smtp.configured.com", 465,
        );
        let params = json!({});
        let host = adapter.resolve_imap_host(&params, "test_tool").unwrap();
        assert_eq!(host, "imap.configured.com");
    }

    #[test]
    fn resolve_imap_host_params_override_config() {
        let adapter = EmailAdapter::with_config(
            "test", "imap.configured.com", 993, "smtp.configured.com", 465,
        );
        let params = json!({"host": "imap.override.com"});
        let host = adapter.resolve_imap_host(&params, "test_tool").unwrap();
        assert_eq!(host, "imap.override.com");
    }

    #[test]
    fn resolve_imap_host_fails_when_no_host_available() {
        let adapter = EmailAdapter::new("test");
        let params = json!({});
        let result = adapter.resolve_imap_host(&params, "test_tool");
        assert!(result.is_err());
    }

    #[test]
    fn parse_fetch_body_extracts_text() {
        let lines = vec![
            "* 1 FETCH (FLAGS (\\Seen) BODY[HEADER] {200}".to_string(),
            "From: sender@example.com".to_string(),
            "To: recipient@example.com".to_string(),
            "Subject: Test Email".to_string(),
            "".to_string(),
            " BODY[TEXT] {50}".to_string(),
            "Hello, this is a test email body.".to_string(),
            ")".to_string(),
            "A003 OK FETCH completed".to_string(),
        ];
        let (_headers, body) = parse_fetch_body(&lines);
        assert!(body.contains("test email body"), "body should contain message text: {body}");
    }
}
