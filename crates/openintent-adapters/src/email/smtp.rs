//! SMTP connection and send operations.

use std::time::Duration;

use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::net::TcpStream;
use tracing::debug;

use crate::error::{AdapterError, Result};
use crate::email::imap::{CONNECT_TIMEOUT_SECS, connect_tls};

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
// SMTP response reading
// ---------------------------------------------------------------------------

/// Read an SMTP response (one or more lines) until the final status line.
pub async fn smtp_read_response(
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

    let status = lines
        .first()
        .and_then(|l| l.get(..3))
        .and_then(|s| s.parse::<u16>().ok())
        .unwrap_or(0);

    Ok((status, lines))
}

// ---------------------------------------------------------------------------
// SMTP send helper
// ---------------------------------------------------------------------------

/// Send an SMTP command, read the response, and verify the expected status class.
pub async fn smtp_send_cmd(
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

/// Send an email via SMTP over TLS.
pub async fn send_email(
    host: &str,
    port: u16,
    username: &str,
    password: &str,
    to: &str,
    subject: &str,
    body: &str,
) -> Result<()> {
    let tool_name = "email_send";
    let tls_stream = connect_tls(host, port).await?;
    let (read_half, mut write_half) = tokio::io::split(tls_stream);
    let mut reader = BufReader::new(read_half);

    let (greeting_status, _) = smtp_read_response(&mut reader).await?;
    if greeting_status / 100 != 2 {
        return Err(AdapterError::ExecutionFailed {
            tool_name: tool_name.into(),
            reason: format!("SMTP server rejected connection with status {greeting_status}"),
        });
    }

    let ehlo = smtp_ehlo_command("openintentos.local");
    smtp_send_cmd(&mut write_half, &mut reader, &ehlo, tool_name, 200).await?;

    let auth_cmd = smtp_auth_login_command();
    smtp_send_cmd(&mut write_half, &mut reader, &auth_cmd, tool_name, 300).await?;

    let b64_user = format!("{}\r\n", smtp_base64_encode(username));
    smtp_send_cmd(&mut write_half, &mut reader, &b64_user, tool_name, 300).await?;

    let b64_pass = format!("{}\r\n", smtp_base64_encode(password));
    smtp_send_cmd(&mut write_half, &mut reader, &b64_pass, tool_name, 200).await?;

    let mail_from = smtp_mail_from_command(username);
    smtp_send_cmd(&mut write_half, &mut reader, &mail_from, tool_name, 200).await?;

    let rcpt_to = smtp_rcpt_to_command(to);
    smtp_send_cmd(&mut write_half, &mut reader, &rcpt_to, tool_name, 200).await?;

    let data_cmd = smtp_data_command();
    smtp_send_cmd(&mut write_half, &mut reader, &data_cmd, tool_name, 300).await?;

    let message = smtp_message_body(username, to, subject, body);
    smtp_send_cmd(&mut write_half, &mut reader, &message, tool_name, 200).await?;

    let quit = smtp_quit_command();
    let _ = write_half.write_all(quit.as_bytes()).await;

    Ok(())
}
