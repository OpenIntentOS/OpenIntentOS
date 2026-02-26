//! Email adapter -- read emails via IMAP and send emails via SMTP.
//!
//! This adapter provides four tools for email interaction:
//! - `email_list_inbox` — List recent emails from the inbox
//! - `email_read` — Read a specific email by sequence number
//! - `email_send` — Send an email via SMTP
//! - `email_search` — Search emails by IMAP query
//!
//! All operations use raw TLS connections to IMAP (port 993) and SMTP
//! (port 465) servers, with credentials passed per-call for flexibility.

use async_trait::async_trait;
use rustls::ClientConfig;
use serde_json::{Value, json};
use std::sync::Arc;
use std::time::Duration;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::net::TcpStream;
use tokio_rustls::TlsConnector;
use tracing::{debug, info};

use crate::error::{AdapterError, Result};
use crate::traits::{Adapter, AdapterType, AuthRequirement, HealthStatus, ToolDefinition};

/// Default IMAP TLS port.
const DEFAULT_IMAP_PORT: u16 = 993;

/// Default SMTP TLS port.
const DEFAULT_SMTP_PORT: u16 = 465;

/// Default number of inbox messages to list.
const DEFAULT_LIST_COUNT: u64 = 10;

/// Connection timeout in seconds.
const CONNECT_TIMEOUT_SECS: u64 = 30;

// ---------------------------------------------------------------------------
// IMAP command builders (pure functions, testable)
// ---------------------------------------------------------------------------

/// Build an IMAP LOGIN command.
pub fn imap_login_command(tag: &str, username: &str, password: &str) -> String {
    format!("{tag} LOGIN \"{username}\" \"{password}\"\r\n")
}

/// Build an IMAP SELECT command.
pub fn imap_select_command(tag: &str, mailbox: &str) -> String {
    format!("{tag} SELECT \"{mailbox}\"\r\n")
}

/// Build an IMAP FETCH command for envelope data.
pub fn imap_fetch_envelope_command(tag: &str, sequence_set: &str) -> String {
    format!("{tag} FETCH {sequence_set} (FLAGS ENVELOPE BODY.PEEK[HEADER.FIELDS (MESSAGE-ID)])\r\n")
}

/// Build an IMAP FETCH command for the full message body.
pub fn imap_fetch_body_command(tag: &str, sequence_number: &str) -> String {
    format!("{tag} FETCH {sequence_number} (FLAGS ENVELOPE BODY[TEXT] BODY[HEADER])\r\n")
}

/// Build an IMAP SEARCH command.
pub fn imap_search_command(tag: &str, query: &str) -> String {
    format!("{tag} SEARCH {query}\r\n")
}

/// Build an IMAP LOGOUT command.
pub fn imap_logout_command(tag: &str) -> String {
    format!("{tag} LOGOUT\r\n")
}

// ---------------------------------------------------------------------------
// SMTP command builders (pure functions, testable)
// ---------------------------------------------------------------------------

/// Build an SMTP EHLO command.
pub fn smtp_ehlo_command(domain: &str) -> String {
    format!("EHLO {domain}\r\n")
}

/// Build an SMTP AUTH LOGIN command.
pub fn smtp_auth_login_command() -> String {
    "AUTH LOGIN\r\n".to_string()
}

/// Encode a string to base64 for SMTP AUTH.
pub fn smtp_base64_encode(input: &str) -> String {
    use base64::Engine;
    base64::engine::general_purpose::STANDARD.encode(input)
}

/// Build an SMTP MAIL FROM command.
pub fn smtp_mail_from_command(from: &str) -> String {
    format!("MAIL FROM:<{from}>\r\n")
}

/// Build an SMTP RCPT TO command.
pub fn smtp_rcpt_to_command(to: &str) -> String {
    format!("RCPT TO:<{to}>\r\n")
}

/// Build an SMTP DATA command.
pub fn smtp_data_command() -> String {
    "DATA\r\n".to_string()
}

/// Build a full email message body for SMTP DATA.
pub fn smtp_message_body(from: &str, to: &str, subject: &str, body: &str) -> String {
    format!(
        "From: {from}\r\n\
         To: {to}\r\n\
         Subject: {subject}\r\n\
         MIME-Version: 1.0\r\n\
         Content-Type: text/plain; charset=UTF-8\r\n\
         \r\n\
         {body}\r\n\
         .\r\n"
    )
}

/// Build an SMTP QUIT command.
pub fn smtp_quit_command() -> String {
    "QUIT\r\n".to_string()
}

// ---------------------------------------------------------------------------
// IMAP response parsing
// ---------------------------------------------------------------------------

/// Extract the EXISTS count from IMAP SELECT response lines.
fn parse_exists_count(lines: &[String]) -> Option<u64> {
    for line in lines {
        let trimmed = line.trim();
        // Format: "* N EXISTS"
        if trimmed.starts_with('*') && trimmed.ends_with("EXISTS") {
            let parts: Vec<&str> = trimmed.split_whitespace().collect();
            if parts.len() >= 3 {
                return parts[1].parse::<u64>().ok();
            }
        }
    }
    None
}

/// Parse envelope data from IMAP FETCH response lines into JSON.
///
/// This is a best-effort parser for IMAP FETCH responses containing
/// envelope data. IMAP responses are complex; this handles common formats.
fn parse_fetch_envelopes(lines: &[String]) -> Vec<Value> {
    let mut results = Vec::new();
    let full_response = lines.join("\n");

    // Split by "* N FETCH" boundaries.
    let parts: Vec<&str> = full_response.split("* ").collect();

    for part in parts {
        let trimmed = part.trim();
        if !trimmed.contains("FETCH") {
            continue;
        }

        // Extract sequence number.
        let seq_num = trimmed
            .split_whitespace()
            .next()
            .and_then(|s| s.parse::<u64>().ok());

        let seq_num = match seq_num {
            Some(n) => n,
            None => continue,
        };

        // Extract envelope fields using simple parsing.
        let subject = extract_quoted_field(trimmed, "ENVELOPE")
            .and_then(|env| extract_envelope_subject(&env))
            .unwrap_or_default();

        let from = extract_envelope_from(trimmed).unwrap_or_default();
        let date = extract_envelope_date(trimmed).unwrap_or_default();

        // Extract message ID if present.
        let message_id = extract_header_field(trimmed, "Message-ID")
            .or_else(|| extract_header_field(trimmed, "Message-Id"))
            .unwrap_or_default();

        results.push(json!({
            "sequence": seq_num,
            "subject": subject,
            "from": from,
            "date": date,
            "message_id": message_id,
        }));
    }

    results
}

/// Extract the ENVELOPE parenthesized data from a FETCH response.
fn extract_quoted_field(text: &str, field: &str) -> Option<String> {
    let idx = text.find(field)?;
    let after = &text[idx + field.len()..];
    let paren_start = after.find('(')?;
    let after_paren = &after[paren_start..];

    // Find matching closing parenthesis (handling nesting).
    let mut depth = 0;
    let mut end = 0;
    for (i, ch) in after_paren.chars().enumerate() {
        match ch {
            '(' => depth += 1,
            ')' => {
                depth -= 1;
                if depth == 0 {
                    end = i;
                    break;
                }
            }
            _ => {}
        }
    }

    if end > 0 {
        Some(after_paren[1..end].to_string())
    } else {
        None
    }
}

/// Extract the subject field from an IMAP ENVELOPE string.
///
/// ENVELOPE format: ("date" "subject" ((from)) ((sender)) ((reply-to))
/// ((to)) ((cc)) ((bcc)) "in-reply-to" "message-id")
fn extract_envelope_subject(envelope: &str) -> Option<String> {
    // The subject is the second quoted string in the envelope.
    let mut in_quote = false;
    let mut quote_count = 0;
    let mut current_quote = String::new();

    for ch in envelope.chars() {
        if ch == '"' {
            if in_quote {
                quote_count += 1;
                if quote_count == 2 {
                    return Some(current_quote);
                }
                current_quote.clear();
                in_quote = false;
            } else {
                in_quote = true;
            }
        } else if in_quote {
            current_quote.push(ch);
        }
    }
    None
}

/// Extract the from address from an IMAP FETCH response.
fn extract_envelope_from(text: &str) -> Option<String> {
    // Look for "From:" header in BODY[HEADER] or parse from envelope.
    if let Some(idx) = text.find("From:") {
        let after = &text[idx + 5..];
        let end = after.find('\r').or_else(|| after.find('\n'))?;
        return Some(after[..end].trim().to_string());
    }
    None
}

/// Extract the date from an IMAP ENVELOPE (first quoted string).
fn extract_envelope_date(text: &str) -> Option<String> {
    if let Some(idx) = text.find("Date:") {
        let after = &text[idx + 5..];
        let end = after.find('\r').or_else(|| after.find('\n'))?;
        return Some(after[..end].trim().to_string());
    }

    // Fallback: extract first quoted string from ENVELOPE.
    if let Some(env) = extract_quoted_field(text, "ENVELOPE") {
        let mut in_quote = false;
        let mut current = String::new();
        for ch in env.chars() {
            if ch == '"' {
                if in_quote {
                    return Some(current);
                }
                in_quote = true;
                current.clear();
            } else if in_quote {
                current.push(ch);
            }
        }
    }

    None
}

/// Extract a header field value from text.
fn extract_header_field(text: &str, field_name: &str) -> Option<String> {
    let search = format!("{field_name}:");
    let idx = text.find(&search)?;
    let after = &text[idx + search.len()..];
    let end = after.find('\r').or_else(|| after.find('\n'))?;
    let value = after[..end].trim().to_string();
    if value.is_empty() { None } else { Some(value) }
}

/// Parse IMAP SEARCH response into sequence numbers.
fn parse_search_results(lines: &[String]) -> Vec<u64> {
    let mut results = Vec::new();
    for line in lines {
        let trimmed = line.trim();
        // Format: "* SEARCH 1 2 3 4 5"
        if let Some(nums_str) = trimmed.strip_prefix("* SEARCH") {
            for token in nums_str.split_whitespace() {
                if let Ok(n) = token.parse::<u64>() {
                    results.push(n);
                }
            }
        }
    }
    results
}

/// Parse a FETCH body response to extract the message text.
fn parse_fetch_body(lines: &[String]) -> (String, String) {
    let full = lines.join("\n");

    // Try to separate headers and body.
    let headers = lines
        .iter()
        .filter(|l| {
            let t = l.trim();
            t.contains(':') && !t.starts_with('*') && !t.starts_with(')')
        })
        .cloned()
        .collect::<Vec<_>>()
        .join("\n");

    // The body is typically everything between empty line and closing paren.
    let body = if let Some(idx) = full.find("\r\n\r\n") {
        let after = &full[idx + 4..];
        // Trim IMAP response artifacts.
        after
            .lines()
            .filter(|l| !l.starts_with(')') && !l.contains("OK FETCH") && !l.contains("FLAGS"))
            .collect::<Vec<_>>()
            .join("\n")
            .trim()
            .to_string()
    } else {
        full.clone()
    };

    (headers, body)
}

// ---------------------------------------------------------------------------
// TLS connection helpers
// ---------------------------------------------------------------------------

/// Build a rustls `ClientConfig` using Mozilla's bundled root certificates.
fn tls_client_config() -> Result<Arc<ClientConfig>> {
    let root_store = rustls::RootCertStore {
        roots: webpki_roots::TLS_SERVER_ROOTS.to_vec(),
    };
    let config = ClientConfig::builder()
        .with_root_certificates(root_store)
        .with_no_client_auth();
    Ok(Arc::new(config))
}

/// Establish a TLS connection to the given host and port.
async fn connect_tls(host: &str, port: u16) -> Result<tokio_rustls::client::TlsStream<TcpStream>> {
    let config = tls_client_config()?;
    let connector = TlsConnector::from(config);
    let server_name = rustls::pki_types::ServerName::try_from(host.to_owned())
        .map_err(|e| AdapterError::ExecutionFailed {
            tool_name: "email".into(),
            reason: format!("invalid server name '{host}': {e}"),
        })?;

    let addr = format!("{host}:{port}");

    let tcp_stream = tokio::time::timeout(
        Duration::from_secs(CONNECT_TIMEOUT_SECS),
        TcpStream::connect(&addr),
    )
    .await
    .map_err(|_| AdapterError::Timeout {
        seconds: CONNECT_TIMEOUT_SECS,
        reason: format!("TCP connection to {addr} timed out"),
    })?
    .map_err(|e| AdapterError::ExecutionFailed {
        tool_name: "email".into(),
        reason: format!("TCP connection to {addr} failed: {e}"),
    })?;

    let tls_stream = tokio::time::timeout(
        Duration::from_secs(CONNECT_TIMEOUT_SECS),
        connector.connect(server_name, tcp_stream),
    )
    .await
    .map_err(|_| AdapterError::Timeout {
        seconds: CONNECT_TIMEOUT_SECS,
        reason: format!("TLS handshake with {host} timed out"),
    })?
    .map_err(|e| AdapterError::ExecutionFailed {
        tool_name: "email".into(),
        reason: format!("TLS handshake with {host} failed: {e}"),
    })?;

    Ok(tls_stream)
}

/// Read lines from an IMAP TLS connection until we see a tagged response
/// or a timeout occurs.
async fn imap_read_response(
    reader: &mut BufReader<tokio::io::ReadHalf<tokio_rustls::client::TlsStream<TcpStream>>>,
    tag: &str,
) -> Result<Vec<String>> {
    let mut lines = Vec::new();
    let deadline = tokio::time::Instant::now() + Duration::from_secs(CONNECT_TIMEOUT_SECS);

    loop {
        let mut line = String::new();
        let read_result = tokio::time::timeout_at(deadline, reader.read_line(&mut line)).await;

        match read_result {
            Ok(Ok(0)) => {
                // Connection closed.
                break;
            }
            Ok(Ok(_)) => {
                let trimmed = line.trim().to_string();
                debug!(imap_line = %trimmed, "IMAP response line");
                let is_tagged = trimmed.starts_with(tag);
                lines.push(trimmed.clone());
                if is_tagged {
                    // Check for error response.
                    if trimmed.contains("NO ") || trimmed.contains("BAD ") {
                        return Err(AdapterError::ExecutionFailed {
                            tool_name: "email".into(),
                            reason: format!("IMAP server error: {trimmed}"),
                        });
                    }
                    break;
                }
            }
            Ok(Err(e)) => {
                return Err(AdapterError::ExecutionFailed {
                    tool_name: "email".into(),
                    reason: format!("IMAP read error: {e}"),
                });
            }
            Err(_) => {
                return Err(AdapterError::Timeout {
                    seconds: CONNECT_TIMEOUT_SECS,
                    reason: "IMAP response timed out".into(),
                });
            }
        }
    }

    Ok(lines)
}

/// Read an SMTP response (one or more lines) until the final status line.
async fn smtp_read_response(
    reader: &mut BufReader<tokio::io::ReadHalf<tokio_rustls::client::TlsStream<TcpStream>>>,
) -> Result<(u16, Vec<String>)> {
    let mut lines = Vec::new();
    let deadline = tokio::time::Instant::now() + Duration::from_secs(CONNECT_TIMEOUT_SECS);

    loop {
        let mut line = String::new();
        let read_result = tokio::time::timeout_at(deadline, reader.read_line(&mut line)).await;

        match read_result {
            Ok(Ok(0)) => break,
            Ok(Ok(_)) => {
                let trimmed = line.trim().to_string();
                debug!(smtp_line = %trimmed, "SMTP response line");
                lines.push(trimmed.clone());

                // SMTP responses: "NNN-text" for continuation, "NNN text" for final.
                if trimmed.len() >= 4 {
                    let fourth_char = trimmed.as_bytes().get(3).copied();
                    if fourth_char == Some(b' ') || fourth_char.is_none() {
                        break;
                    }
                } else {
                    break;
                }
            }
            Ok(Err(e)) => {
                return Err(AdapterError::ExecutionFailed {
                    tool_name: "email".into(),
                    reason: format!("SMTP read error: {e}"),
                });
            }
            Err(_) => {
                return Err(AdapterError::Timeout {
                    seconds: CONNECT_TIMEOUT_SECS,
                    reason: "SMTP response timed out".into(),
                });
            }
        }
    }

    // Parse status code from the first line.
    let status = lines
        .first()
        .and_then(|l| l.get(..3))
        .and_then(|s| s.parse::<u16>().ok())
        .unwrap_or(0);

    Ok((status, lines))
}

// ---------------------------------------------------------------------------
// Email adapter
// ---------------------------------------------------------------------------

/// Email service adapter for IMAP reading and SMTP sending.
///
/// Credentials are passed per-call rather than stored, keeping the adapter
/// stateless and avoiding credential storage concerns.
pub struct EmailAdapter {
    /// Unique identifier for this adapter instance.
    id: String,
    /// Whether the adapter is logically connected (ready to process tools).
    connected: bool,
    /// IMAP server hostname.
    imap_host: String,
    /// IMAP server port (default: 993).
    imap_port: u16,
    /// SMTP server hostname.
    smtp_host: String,
    /// SMTP server port (default: 465).
    smtp_port: u16,
}

impl EmailAdapter {
    /// Create a new email adapter with default ports.
    ///
    /// IMAP port defaults to 993 (TLS), SMTP port defaults to 465 (TLS).
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
    fn resolve_imap_host<'a>(&'a self, params: &'a Value, tool_name: &str) -> Result<&'a str> {
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
    fn resolve_smtp_host<'a>(&'a self, params: &'a Value, tool_name: &str) -> Result<&'a str> {
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

    /// Extract required username from params.
    fn extract_username<'a>(&self, params: &'a Value, tool_name: &str) -> Result<&'a str> {
        params
            .get("username")
            .and_then(|v| v.as_str())
            .ok_or_else(|| AdapterError::InvalidParams {
                tool_name: tool_name.into(),
                reason: "missing required string field `username`".into(),
            })
    }

    /// Extract required password from params.
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

    /// List recent emails from the inbox via IMAP.
    async fn tool_email_list_inbox(&self, params: Value) -> Result<Value> {
        let tool_name = "email_list_inbox";
        let host = self.resolve_imap_host(&params, tool_name)?;
        let username = self.extract_username(&params, tool_name)?;
        let password = self.extract_password(&params, tool_name)?;
        let count = params
            .get("count")
            .and_then(|v| v.as_u64())
            .unwrap_or(DEFAULT_LIST_COUNT);

        info!(
            host = host,
            username = username,
            count = count,
            "listing inbox emails"
        );

        let tls_stream = connect_tls(host, self.imap_port).await?;
        let (read_half, mut write_half) = tokio::io::split(tls_stream);
        let mut reader = BufReader::new(read_half);

        // Read server greeting.
        let _greeting = imap_read_response(&mut reader, "*").await?;

        // LOGIN
        let login_cmd = imap_login_command("A001", username, password);
        write_half
            .write_all(login_cmd.as_bytes())
            .await
            .map_err(|e| AdapterError::ExecutionFailed {
                tool_name: tool_name.into(),
                reason: format!("IMAP write error: {e}"),
            })?;
        let _login_resp = imap_read_response(&mut reader, "A001").await?;

        // SELECT INBOX
        let select_cmd = imap_select_command("A002", "INBOX");
        write_half
            .write_all(select_cmd.as_bytes())
            .await
            .map_err(|e| AdapterError::ExecutionFailed {
                tool_name: tool_name.into(),
                reason: format!("IMAP write error: {e}"),
            })?;
        let select_resp = imap_read_response(&mut reader, "A002").await?;

        // Parse EXISTS to know how many messages are in the mailbox.
        let exists = parse_exists_count(&select_resp).unwrap_or(0);

        if exists == 0 {
            // Logout and return empty.
            let logout_cmd = imap_logout_command("A003");
            let _ = write_half.write_all(logout_cmd.as_bytes()).await;
            return Ok(json!({
                "emails": [],
                "total": 0,
                "fetched": 0,
            }));
        }

        // Calculate the range of messages to fetch (most recent N).
        let start = if exists > count {
            exists - count + 1
        } else {
            1
        };
        let sequence_set = format!("{start}:{exists}");

        // FETCH envelopes.
        let fetch_cmd = imap_fetch_envelope_command("A003", &sequence_set);
        write_half
            .write_all(fetch_cmd.as_bytes())
            .await
            .map_err(|e| AdapterError::ExecutionFailed {
                tool_name: tool_name.into(),
                reason: format!("IMAP write error: {e}"),
            })?;
        let fetch_resp = imap_read_response(&mut reader, "A003").await?;

        let emails = parse_fetch_envelopes(&fetch_resp);

        // LOGOUT
        let logout_cmd = imap_logout_command("A004");
        let _ = write_half.write_all(logout_cmd.as_bytes()).await;

        Ok(json!({
            "emails": emails,
            "total": exists,
            "fetched": emails.len(),
        }))
    }

    /// Read a specific email by sequence number via IMAP.
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
        let mut reader = BufReader::new(read_half);

        // Read server greeting.
        let _greeting = imap_read_response(&mut reader, "*").await?;

        // LOGIN
        let login_cmd = imap_login_command("A001", username, password);
        write_half
            .write_all(login_cmd.as_bytes())
            .await
            .map_err(|e| AdapterError::ExecutionFailed {
                tool_name: tool_name.into(),
                reason: format!("IMAP write error: {e}"),
            })?;
        let _login_resp = imap_read_response(&mut reader, "A001").await?;

        // SELECT INBOX
        let select_cmd = imap_select_command("A002", "INBOX");
        write_half
            .write_all(select_cmd.as_bytes())
            .await
            .map_err(|e| AdapterError::ExecutionFailed {
                tool_name: tool_name.into(),
                reason: format!("IMAP write error: {e}"),
            })?;
        let _select_resp = imap_read_response(&mut reader, "A002").await?;

        // FETCH the message body.
        let fetch_cmd = imap_fetch_body_command("A003", message_id);
        write_half
            .write_all(fetch_cmd.as_bytes())
            .await
            .map_err(|e| AdapterError::ExecutionFailed {
                tool_name: tool_name.into(),
                reason: format!("IMAP write error: {e}"),
            })?;
        let fetch_resp = imap_read_response(&mut reader, "A003").await?;

        let (headers, body) = parse_fetch_body(&fetch_resp);

        // LOGOUT
        let logout_cmd = imap_logout_command("A004");
        let _ = write_half.write_all(logout_cmd.as_bytes()).await;

        Ok(json!({
            "message_id": message_id,
            "headers": headers,
            "body": body,
        }))
    }

    /// Send an email via SMTP.
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

        let subject = params
            .get("subject")
            .and_then(|v| v.as_str())
            .ok_or_else(|| AdapterError::InvalidParams {
                tool_name: tool_name.into(),
                reason: "missing required string field `subject`".into(),
            })?;

        let body = params.get("body").and_then(|v| v.as_str()).ok_or_else(|| {
            AdapterError::InvalidParams {
                tool_name: tool_name.into(),
                reason: "missing required string field `body`".into(),
            }
        })?;

        info!(host = host, to = to, subject = subject, "sending email");

        let tls_stream = connect_tls(host, self.smtp_port).await?;
        let (read_half, mut write_half) = tokio::io::split(tls_stream);
        let mut reader = BufReader::new(read_half);

        // Helper to send a command and check the response.
        async fn smtp_send_cmd(
            writer: &mut tokio::io::WriteHalf<tokio_rustls::client::TlsStream<TcpStream>>,
            reader: &mut BufReader<tokio::io::ReadHalf<tokio_rustls::client::TlsStream<TcpStream>>>,
            cmd: &str,
            tool_name: &str,
            expected_status_prefix: u16,
        ) -> Result<(u16, Vec<String>)> {
            writer
                .write_all(cmd.as_bytes())
                .await
                .map_err(|e| AdapterError::ExecutionFailed {
                    tool_name: tool_name.into(),
                    reason: format!("SMTP write error: {e}"),
                })?;
            let (status, lines) = smtp_read_response(reader).await?;
            // Check status is in the expected range (e.g., 2xx or 3xx).
            let expected_first_digit = expected_status_prefix / 100;
            if status / 100 != expected_first_digit {
                return Err(AdapterError::ExecutionFailed {
                    tool_name: tool_name.into(),
                    reason: format!(
                        "SMTP error: expected {}xx, got {status}: {}",
                        expected_first_digit,
                        lines.join("; ")
                    ),
                });
            }
            Ok((status, lines))
        }

        // Read server greeting.
        let (greeting_status, _) = smtp_read_response(&mut reader).await?;
        if greeting_status / 100 != 2 {
            return Err(AdapterError::ExecutionFailed {
                tool_name: tool_name.into(),
                reason: format!("SMTP server rejected connection with status {greeting_status}"),
            });
        }

        // EHLO
        let ehlo = smtp_ehlo_command("openintentos.local");
        smtp_send_cmd(&mut write_half, &mut reader, &ehlo, tool_name, 200).await?;

        // AUTH LOGIN
        let auth_cmd = smtp_auth_login_command();
        smtp_send_cmd(&mut write_half, &mut reader, &auth_cmd, tool_name, 300).await?;

        // Send base64-encoded username.
        let b64_user = format!("{}\r\n", smtp_base64_encode(username));
        smtp_send_cmd(&mut write_half, &mut reader, &b64_user, tool_name, 300).await?;

        // Send base64-encoded password.
        let b64_pass = format!("{}\r\n", smtp_base64_encode(password));
        smtp_send_cmd(&mut write_half, &mut reader, &b64_pass, tool_name, 200).await?;

        // MAIL FROM
        let mail_from = smtp_mail_from_command(username);
        smtp_send_cmd(&mut write_half, &mut reader, &mail_from, tool_name, 200).await?;

        // RCPT TO
        let rcpt_to = smtp_rcpt_to_command(to);
        smtp_send_cmd(&mut write_half, &mut reader, &rcpt_to, tool_name, 200).await?;

        // DATA
        let data_cmd = smtp_data_command();
        smtp_send_cmd(&mut write_half, &mut reader, &data_cmd, tool_name, 300).await?;

        // Send message body.
        let message = smtp_message_body(username, to, subject, body);
        smtp_send_cmd(&mut write_half, &mut reader, &message, tool_name, 200).await?;

        // QUIT
        let quit = smtp_quit_command();
        let _ = write_half.write_all(quit.as_bytes()).await;

        Ok(json!({
            "status": "sent",
            "to": to,
            "subject": subject,
        }))
    }

    /// Search emails by IMAP query.
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

        info!(
            host = host,
            query = query,
            count = count,
            "searching emails"
        );

        let tls_stream = connect_tls(host, self.imap_port).await?;
        let (read_half, mut write_half) = tokio::io::split(tls_stream);
        let mut reader = BufReader::new(read_half);

        // Read server greeting.
        let _greeting = imap_read_response(&mut reader, "*").await?;

        // LOGIN
        let login_cmd = imap_login_command("A001", username, password);
        write_half
            .write_all(login_cmd.as_bytes())
            .await
            .map_err(|e| AdapterError::ExecutionFailed {
                tool_name: tool_name.into(),
                reason: format!("IMAP write error: {e}"),
            })?;
        let _login_resp = imap_read_response(&mut reader, "A001").await?;

        // SELECT INBOX
        let select_cmd = imap_select_command("A002", "INBOX");
        write_half
            .write_all(select_cmd.as_bytes())
            .await
            .map_err(|e| AdapterError::ExecutionFailed {
                tool_name: tool_name.into(),
                reason: format!("IMAP write error: {e}"),
            })?;
        let _select_resp = imap_read_response(&mut reader, "A002").await?;

        // SEARCH
        let search_cmd = imap_search_command("A003", query);
        write_half
            .write_all(search_cmd.as_bytes())
            .await
            .map_err(|e| AdapterError::ExecutionFailed {
                tool_name: tool_name.into(),
                reason: format!("IMAP write error: {e}"),
            })?;
        let search_resp = imap_read_response(&mut reader, "A003").await?;

        let mut matching_ids = parse_search_results(&search_resp);

        // Limit results and fetch envelopes for matching messages.
        let total_matches = matching_ids.len();

        // Take the last `count` results (most recent).
        if matching_ids.len() > count as usize {
            let start = matching_ids.len() - count as usize;
            matching_ids = matching_ids[start..].to_vec();
        }

        let emails = if matching_ids.is_empty() {
            Vec::new()
        } else {
            // Build a sequence set from matching IDs.
            let sequence_set = matching_ids
                .iter()
                .map(|id| id.to_string())
                .collect::<Vec<_>>()
                .join(",");

            let fetch_cmd = imap_fetch_envelope_command("A004", &sequence_set);
            write_half
                .write_all(fetch_cmd.as_bytes())
                .await
                .map_err(|e| AdapterError::ExecutionFailed {
                    tool_name: tool_name.into(),
                    reason: format!("IMAP write error: {e}"),
                })?;
            let fetch_resp = imap_read_response(&mut reader, "A004").await?;

            parse_fetch_envelopes(&fetch_resp)
        };

        // LOGOUT
        let logout_tag = if matching_ids.is_empty() {
            "A004"
        } else {
            "A005"
        };
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

    // -- Constructor tests --------------------------------------------------

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

    // -- Adapter trait tests ------------------------------------------------

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

    // -- Tool definition tests ----------------------------------------------

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
            assert_eq!(
                params.get("type").and_then(|v| v.as_str()),
                Some("object"),
                "tool {} parameters should be an object schema",
                tool.name
            );
            assert!(
                params.get("properties").is_some(),
                "tool {} should have properties",
                tool.name
            );
            assert!(
                params.get("required").is_some(),
                "tool {} should have required fields",
                tool.name
            );
        }
    }

    #[test]
    fn email_list_inbox_tool_has_correct_required_params() {
        let adapter = EmailAdapter::new("email-test");
        let tool = adapter
            .tools()
            .into_iter()
            .find(|t| t.name == "email_list_inbox")
            .expect("email_list_inbox tool should exist");

        let required = tool.parameters.get("required").and_then(|v| v.as_array());
        assert!(required.is_some());
        let required: Vec<&str> = required
            .expect("required should be an array")
            .iter()
            .filter_map(|v| v.as_str())
            .collect();
        assert!(required.contains(&"username"));
        assert!(required.contains(&"password"));
    }

    #[test]
    fn email_send_tool_has_correct_required_params() {
        let adapter = EmailAdapter::new("email-test");
        let tool = adapter
            .tools()
            .into_iter()
            .find(|t| t.name == "email_send")
            .expect("email_send tool should exist");

        let required = tool.parameters.get("required").and_then(|v| v.as_array());
        assert!(required.is_some());
        let required: Vec<&str> = required
            .expect("required should be an array")
            .iter()
            .filter_map(|v| v.as_str())
            .collect();
        assert!(required.contains(&"to"));
        assert!(required.contains(&"subject"));
        assert!(required.contains(&"body"));
        assert!(required.contains(&"username"));
        assert!(required.contains(&"password"));
    }

    #[test]
    fn email_search_tool_has_correct_required_params() {
        let adapter = EmailAdapter::new("email-test");
        let tool = adapter
            .tools()
            .into_iter()
            .find(|t| t.name == "email_search")
            .expect("email_search tool should exist");

        let required = tool.parameters.get("required").and_then(|v| v.as_array());
        assert!(required.is_some());
        let required: Vec<&str> = required
            .expect("required should be an array")
            .iter()
            .filter_map(|v| v.as_str())
            .collect();
        assert!(required.contains(&"query"));
        assert!(required.contains(&"username"));
        assert!(required.contains(&"password"));
    }

    // -- Connect / disconnect / health check --------------------------------

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

    // -- Execute tool rejection tests ---------------------------------------

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
        assert!(
            err.contains("not connected"),
            "error should mention not connected: {err}"
        );
    }

    #[tokio::test]
    async fn email_adapter_rejects_unknown_tool() {
        let mut adapter = EmailAdapter::new("email-test");
        adapter.connect().await.unwrap();
        let result = adapter.execute_tool("nonexistent", json!({})).await;
        assert!(result.is_err());
        let err = result.unwrap_err().to_string();
        assert!(
            err.contains("nonexistent"),
            "error should mention the tool name: {err}"
        );
    }

    // -- IMAP command building tests ----------------------------------------

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
        assert_eq!(
            cmd,
            "A003 FETCH 1:10 (FLAGS ENVELOPE BODY.PEEK[HEADER.FIELDS (MESSAGE-ID)])\r\n"
        );
    }

    #[test]
    fn imap_fetch_body_command_format() {
        let cmd = imap_fetch_body_command("A003", "5");
        assert_eq!(
            cmd,
            "A003 FETCH 5 (FLAGS ENVELOPE BODY[TEXT] BODY[HEADER])\r\n"
        );
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

    // -- SMTP command building tests ----------------------------------------

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

    // -- Base64 encoding tests ----------------------------------------------

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
    fn smtp_base64_encode_password() {
        let encoded = smtp_base64_encode("my-secret-password");
        let decoded_bytes = base64::engine::general_purpose::STANDARD
            .decode(&encoded)
            .unwrap();
        let decoded = String::from_utf8(decoded_bytes).unwrap();
        assert_eq!(decoded, "my-secret-password");
    }

    #[test]
    fn smtp_base64_encode_empty() {
        let encoded = smtp_base64_encode("");
        assert_eq!(encoded, "");
    }

    #[test]
    fn smtp_base64_encode_special_chars() {
        let encoded = smtp_base64_encode("p@$$w0rd!#%&");
        let decoded_bytes = base64::engine::general_purpose::STANDARD
            .decode(&encoded)
            .unwrap();
        let decoded = String::from_utf8(decoded_bytes).unwrap();
        assert_eq!(decoded, "p@$$w0rd!#%&");
    }

    // -- IMAP response parsing tests ----------------------------------------

    #[test]
    fn parse_exists_count_from_select_response() {
        let lines = vec![
            "* FLAGS (\\Answered \\Flagged \\Deleted \\Seen \\Draft)".to_string(),
            "* OK [PERMANENTFLAGS (\\Answered \\Flagged \\Deleted \\Seen \\Draft \\*)]".to_string(),
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
            "* 0 RECENT".to_string(),
            "A002 OK SELECT completed".to_string(),
        ];
        assert_eq!(parse_exists_count(&lines), Some(0));
    }

    #[test]
    fn parse_exists_count_missing() {
        let lines = vec!["A002 OK SELECT completed".to_string()];
        assert_eq!(parse_exists_count(&lines), None);
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

    #[test]
    fn parse_search_results_single_match() {
        let lines = vec![
            "* SEARCH 42".to_string(),
            "A003 OK SEARCH completed".to_string(),
        ];
        let results = parse_search_results(&lines);
        assert_eq!(results, vec![42]);
    }

    // -- Parameter validation tests -----------------------------------------

    #[tokio::test]
    async fn email_list_inbox_missing_username_fails() {
        let mut adapter = EmailAdapter::with_config(
            "email-test",
            "imap.example.com",
            993,
            "smtp.example.com",
            465,
        );
        adapter.connect().await.unwrap();

        let result = adapter
            .execute_tool("email_list_inbox", json!({"password": "secret"}))
            .await;
        assert!(result.is_err());
        let err = result.unwrap_err().to_string();
        assert!(
            err.contains("username"),
            "error should mention username: {err}"
        );
    }

    #[tokio::test]
    async fn email_list_inbox_missing_password_fails() {
        let mut adapter = EmailAdapter::with_config(
            "email-test",
            "imap.example.com",
            993,
            "smtp.example.com",
            465,
        );
        adapter.connect().await.unwrap();

        let result = adapter
            .execute_tool("email_list_inbox", json!({"username": "user"}))
            .await;
        assert!(result.is_err());
        let err = result.unwrap_err().to_string();
        assert!(
            err.contains("password"),
            "error should mention password: {err}"
        );
    }

    #[tokio::test]
    async fn email_send_missing_to_field_fails() {
        let mut adapter = EmailAdapter::with_config(
            "email-test",
            "imap.example.com",
            993,
            "smtp.example.com",
            465,
        );
        adapter.connect().await.unwrap();

        let result = adapter
            .execute_tool(
                "email_send",
                json!({
                    "subject": "Test",
                    "body": "Hello",
                    "username": "user",
                    "password": "pass"
                }),
            )
            .await;
        assert!(result.is_err());
        let err = result.unwrap_err().to_string();
        assert!(err.contains("to"), "error should mention 'to': {err}");
    }

    #[tokio::test]
    async fn email_send_missing_subject_fails() {
        let mut adapter = EmailAdapter::with_config(
            "email-test",
            "imap.example.com",
            993,
            "smtp.example.com",
            465,
        );
        adapter.connect().await.unwrap();

        let result = adapter
            .execute_tool(
                "email_send",
                json!({
                    "to": "recipient@example.com",
                    "body": "Hello",
                    "username": "user",
                    "password": "pass"
                }),
            )
            .await;
        assert!(result.is_err());
        let err = result.unwrap_err().to_string();
        assert!(
            err.contains("subject"),
            "error should mention 'subject': {err}"
        );
    }

    #[tokio::test]
    async fn email_search_missing_query_fails() {
        let mut adapter = EmailAdapter::with_config(
            "email-test",
            "imap.example.com",
            993,
            "smtp.example.com",
            465,
        );
        adapter.connect().await.unwrap();

        let result = adapter
            .execute_tool(
                "email_search",
                json!({"username": "user", "password": "pass"}),
            )
            .await;
        assert!(result.is_err());
        let err = result.unwrap_err().to_string();
        assert!(err.contains("query"), "error should mention 'query': {err}");
    }

    #[tokio::test]
    async fn email_read_missing_message_id_fails() {
        let mut adapter = EmailAdapter::with_config(
            "email-test",
            "imap.example.com",
            993,
            "smtp.example.com",
            465,
        );
        adapter.connect().await.unwrap();

        let result = adapter
            .execute_tool(
                "email_read",
                json!({"username": "user", "password": "pass"}),
            )
            .await;
        assert!(result.is_err());
        let err = result.unwrap_err().to_string();
        assert!(
            err.contains("message_id"),
            "error should mention 'message_id': {err}"
        );
    }

    // -- Host resolution tests ----------------------------------------------

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
            "test",
            "imap.configured.com",
            993,
            "smtp.configured.com",
            465,
        );
        let params = json!({});
        let host = adapter.resolve_imap_host(&params, "test_tool").unwrap();
        assert_eq!(host, "imap.configured.com");
    }

    #[test]
    fn resolve_imap_host_params_override_config() {
        let adapter = EmailAdapter::with_config(
            "test",
            "imap.configured.com",
            993,
            "smtp.configured.com",
            465,
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
    fn resolve_smtp_host_from_params() {
        let adapter = EmailAdapter::new("test");
        let params = json!({"host": "smtp.gmail.com"});
        let host = adapter.resolve_smtp_host(&params, "test_tool").unwrap();
        assert_eq!(host, "smtp.gmail.com");
    }

    #[test]
    fn resolve_smtp_host_from_config() {
        let adapter = EmailAdapter::with_config(
            "test",
            "imap.configured.com",
            993,
            "smtp.configured.com",
            465,
        );
        let params = json!({});
        let host = adapter.resolve_smtp_host(&params, "test_tool").unwrap();
        assert_eq!(host, "smtp.configured.com");
    }

    #[test]
    fn resolve_smtp_host_fails_when_no_host_available() {
        let adapter = EmailAdapter::new("test");
        let params = json!({});
        let result = adapter.resolve_smtp_host(&params, "test_tool");
        assert!(result.is_err());
    }

    // -- Fetch body parsing tests -------------------------------------------

    #[test]
    fn parse_fetch_body_extracts_text() {
        let lines = vec![
            "* 1 FETCH (FLAGS (\\Seen) BODY[HEADER] {200}".to_string(),
            "From: sender@example.com".to_string(),
            "To: recipient@example.com".to_string(),
            "Subject: Test Email".to_string(),
            "Date: Mon, 01 Jan 2024 12:00:00 +0000".to_string(),
            "".to_string(),
            " BODY[TEXT] {50}".to_string(),
            "Hello, this is a test email body.".to_string(),
            ")".to_string(),
            "A003 OK FETCH completed".to_string(),
        ];
        let (_headers, body) = parse_fetch_body(&lines);
        assert!(
            body.contains("test email body"),
            "body should contain message text: {body}"
        );
    }
}
