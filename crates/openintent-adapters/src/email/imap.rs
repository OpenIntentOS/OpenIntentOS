//! IMAP connection and read operations.

use std::sync::Arc;
use std::time::Duration;

use rustls::ClientConfig;
use serde_json::{Value, json};
use tokio::io::{AsyncBufReadExt, BufReader};
use tokio::net::TcpStream;
use tokio_rustls::TlsConnector;
use tracing::debug;

use crate::error::{AdapterError, Result};

/// Connection timeout in seconds.
pub const CONNECT_TIMEOUT_SECS: u64 = 30;

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
// IMAP response parsing
// ---------------------------------------------------------------------------

/// Extract the EXISTS count from IMAP SELECT response lines.
pub fn parse_exists_count(lines: &[String]) -> Option<u64> {
    for line in lines {
        let trimmed = line.trim();
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
pub fn parse_fetch_envelopes(lines: &[String]) -> Vec<Value> {
    let mut results = Vec::new();
    let full_response = lines.join("\n");

    let parts: Vec<&str> = full_response.split("* ").collect();

    for part in parts {
        let trimmed = part.trim();
        if !trimmed.contains("FETCH") {
            continue;
        }

        let seq_num = trimmed
            .split_whitespace()
            .next()
            .and_then(|s| s.parse::<u64>().ok());

        let seq_num = match seq_num {
            Some(n) => n,
            None => continue,
        };

        let subject = extract_quoted_field(trimmed, "ENVELOPE")
            .and_then(|env| extract_envelope_subject(&env))
            .unwrap_or_default();

        let from = extract_envelope_from(trimmed).unwrap_or_default();
        let date = extract_envelope_date(trimmed).unwrap_or_default();

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
fn extract_envelope_subject(envelope: &str) -> Option<String> {
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
pub fn extract_header_field(text: &str, field_name: &str) -> Option<String> {
    let search = format!("{field_name}:");
    let idx = text.find(&search)?;
    let after = &text[idx + search.len()..];
    let end = after.find('\r').or_else(|| after.find('\n'))?;
    let value = after[..end].trim().to_string();
    if value.is_empty() { None } else { Some(value) }
}

/// Parse IMAP SEARCH response into sequence numbers.
pub fn parse_search_results(lines: &[String]) -> Vec<u64> {
    let mut results = Vec::new();
    for line in lines {
        let trimmed = line.trim();
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
pub fn parse_fetch_body(lines: &[String]) -> (String, String) {
    let full = lines.join("\n");

    let headers = lines
        .iter()
        .filter(|l| {
            let t = l.trim();
            t.contains(':') && !t.starts_with('*') && !t.starts_with(')')
        })
        .cloned()
        .collect::<Vec<_>>()
        .join("\n");

    let body = if let Some(idx) = full.find("\r\n\r\n") {
        let after = &full[idx + 4..];
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
pub fn tls_client_config() -> Result<Arc<ClientConfig>> {
    let root_store = rustls::RootCertStore {
        roots: webpki_roots::TLS_SERVER_ROOTS.to_vec(),
    };
    let config = ClientConfig::builder_with_provider(Arc::new(
        rustls::crypto::ring::default_provider(),
    ))
    .with_safe_default_protocol_versions()
    .map_err(|e| AdapterError::ConfigError(format!("TLS protocol config: {e}")))?
    .with_root_certificates(root_store)
    .with_no_client_auth();
    Ok(Arc::new(config))
}

/// Establish a TLS connection to the given host and port.
pub async fn connect_tls(
    host: &str,
    port: u16,
) -> Result<tokio_rustls::client::TlsStream<TcpStream>> {
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

/// Read lines from an IMAP TLS connection until we see a tagged response.
pub async fn imap_read_response(
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
                break;
            }
            Ok(Ok(_)) => {
                let trimmed = line.trim().to_string();
                debug!(imap_line = %trimmed, "IMAP response line");
                let is_tagged = trimmed.starts_with(tag);
                lines.push(trimmed.clone());
                if is_tagged {
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
