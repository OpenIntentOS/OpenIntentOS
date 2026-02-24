//! Local HTTP callback server for OAuth redirect.
//!
//! When an OAuth flow completes in the browser, the authorization server
//! redirects to a local URL with `?code=xxx&state=yyy`. This module provides
//! a minimal TCP server that listens for that single request, extracts the
//! code and state, returns a success page, and shuts down.
//!
//! No external HTTP server framework is needed — this uses raw
//! [`tokio::net::TcpListener`] to minimize dependencies.

use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpListener;

use crate::error::{AuthEngineError, Result};

/// The HTML page returned to the browser after a successful callback.
const SUCCESS_HTML: &str = r#"<!DOCTYPE html>
<html>
<head>
    <meta charset="utf-8">
    <title>Authorization Successful</title>
    <style>
        body {
            font-family: -apple-system, BlinkMacSystemFont, "Segoe UI", Roboto, sans-serif;
            display: flex;
            justify-content: center;
            align-items: center;
            height: 100vh;
            margin: 0;
            background: #f5f5f5;
            color: #333;
        }
        .card {
            text-align: center;
            padding: 3rem;
            background: white;
            border-radius: 12px;
            box-shadow: 0 2px 10px rgba(0,0,0,0.08);
        }
        h1 { color: #22c55e; margin-bottom: 0.5rem; }
        p { color: #666; }
    </style>
</head>
<body>
    <div class="card">
        <h1>Authorization Successful</h1>
        <p>You can close this tab and return to OpenIntentOS.</p>
    </div>
</body>
</html>"#;

/// A minimal HTTP callback server that listens for a single OAuth redirect.
pub struct CallbackServer;

impl CallbackServer {
    /// Start the callback server and wait for the OAuth redirect.
    ///
    /// Binds to `127.0.0.1:{port}`, waits for a single GET request with
    /// `?code=xxx&state=yyy` query parameters, returns a success HTML page
    /// to the browser, and returns the extracted `(code, state)` tuple.
    ///
    /// # Errors
    ///
    /// - [`AuthEngineError::CallbackTimeout`] if `timeout_secs` elapse
    ///   before a request arrives.
    /// - [`AuthEngineError::Io`] if the TCP listener cannot bind.
    /// - [`AuthEngineError::FlowFailed`] if the request is missing required
    ///   query parameters.
    pub async fn start(port: u16, timeout_secs: u64) -> Result<(String, String)> {
        let addr = format!("127.0.0.1:{port}");
        let listener = TcpListener::bind(&addr).await?;

        tracing::info!(addr = %addr, "callback server listening for OAuth redirect");

        let timeout = tokio::time::Duration::from_secs(timeout_secs);
        let result = tokio::time::timeout(timeout, Self::accept_one(&listener)).await;

        match result {
            Ok(inner) => inner,
            Err(_) => Err(AuthEngineError::CallbackTimeout { timeout_secs }),
        }
    }

    /// Accept a single connection, parse the request, send a response.
    async fn accept_one(listener: &TcpListener) -> Result<(String, String)> {
        let (mut stream, peer) = listener.accept().await?;

        tracing::debug!(peer = %peer, "accepted callback connection");

        // Read the HTTP request. OAuth redirects are small GET requests,
        // so 4KB is more than enough.
        let mut buf = [0u8; 4096];
        let n = stream.read(&mut buf).await?;
        let request = String::from_utf8_lossy(&buf[..n]);

        // Parse the first line: "GET /callback?code=xxx&state=yyy HTTP/1.1"
        let (code, state) = Self::parse_callback_request(&request)?;

        // Send an HTTP response with the success page.
        let response_body = SUCCESS_HTML;
        let response = format!(
            "HTTP/1.1 200 OK\r\nContent-Type: text/html; charset=utf-8\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
            response_body.len(),
            response_body
        );

        stream.write_all(response.as_bytes()).await?;
        stream.flush().await?;

        tracing::info!("callback received, authorization code extracted");

        Ok((code, state))
    }

    /// Parse the query parameters from the first line of an HTTP GET request.
    ///
    /// Expected format: `GET /some/path?code=xxx&state=yyy HTTP/1.1`
    fn parse_callback_request(request: &str) -> Result<(String, String)> {
        // Extract the request line.
        let request_line = request
            .lines()
            .next()
            .ok_or_else(|| AuthEngineError::FlowFailed {
                reason: "empty HTTP request".to_string(),
            })?;

        // Split into method, path, and version.
        let parts: Vec<&str> = request_line.split_whitespace().collect();
        if parts.len() < 2 {
            return Err(AuthEngineError::FlowFailed {
                reason: format!("malformed HTTP request line: {request_line}"),
            });
        }

        let path = parts[1];

        // Parse the query string.
        // The path may be "/callback?code=xxx&state=yyy" or just "/?code=...".
        let query =
            path.split_once('?')
                .map(|(_, q)| q)
                .ok_or_else(|| AuthEngineError::FlowFailed {
                    reason: "callback request has no query string".to_string(),
                })?;

        let mut code: Option<String> = None;
        let mut state: Option<String> = None;

        // Check for an error parameter first.
        for param in query.split('&') {
            if let Some((key, value)) = param.split_once('=') {
                let decoded = Self::percent_decode(value);
                match key {
                    "code" => code = Some(decoded),
                    "state" => state = Some(decoded),
                    "error" => {
                        return Err(AuthEngineError::FlowFailed {
                            reason: format!("authorization server returned error: {decoded}"),
                        });
                    }
                    _ => {}
                }
            }
        }

        let code = code.ok_or_else(|| AuthEngineError::FlowFailed {
            reason: "callback missing 'code' parameter".to_string(),
        })?;

        let state = state.ok_or_else(|| AuthEngineError::FlowFailed {
            reason: "callback missing 'state' parameter".to_string(),
        })?;

        Ok((code, state))
    }

    /// Minimal percent-decoding for query parameter values.
    ///
    /// Handles `%XX` sequences and `+` as space.
    fn percent_decode(input: &str) -> String {
        let mut output = String::with_capacity(input.len());
        let mut chars = input.bytes();

        while let Some(b) = chars.next() {
            match b {
                b'%' => {
                    let hi = chars.next();
                    let lo = chars.next();
                    if let (Some(h), Some(l)) = (hi, lo) {
                        let hex = [h, l];
                        if let Ok(s) = std::str::from_utf8(&hex)
                            && let Ok(byte) = u8::from_str_radix(s, 16)
                        {
                            output.push(byte as char);
                            continue;
                        }
                        // If decoding fails, output the literal characters.
                        output.push('%');
                        output.push(h as char);
                        output.push(l as char);
                    }
                }
                b'+' => output.push(' '),
                _ => output.push(b as char),
            }
        }

        output
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_callback_request_standard() {
        let request =
            "GET /callback?code=abc123&state=xyz789 HTTP/1.1\r\nHost: 127.0.0.1:8400\r\n\r\n";
        let (code, state) = CallbackServer::parse_callback_request(request).unwrap();
        assert_eq!(code, "abc123");
        assert_eq!(state, "xyz789");
    }

    #[test]
    fn parse_callback_request_root_path() {
        let request = "GET /?code=mycode&state=mystate HTTP/1.1\r\n\r\n";
        let (code, state) = CallbackServer::parse_callback_request(request).unwrap();
        assert_eq!(code, "mycode");
        assert_eq!(state, "mystate");
    }

    #[test]
    fn parse_callback_request_with_extra_params() {
        let request = "GET /cb?code=c123&state=s456&session_state=abcdef HTTP/1.1\r\n\r\n";
        let (code, state) = CallbackServer::parse_callback_request(request).unwrap();
        assert_eq!(code, "c123");
        assert_eq!(state, "s456");
    }

    #[test]
    fn parse_callback_request_percent_encoded() {
        let request = "GET /cb?code=abc%20def&state=123%2B456 HTTP/1.1\r\n\r\n";
        let (code, state) = CallbackServer::parse_callback_request(request).unwrap();
        assert_eq!(code, "abc def");
        assert_eq!(state, "123+456");
    }

    #[test]
    fn parse_callback_request_plus_as_space() {
        let request = "GET /cb?code=hello+world&state=foo+bar HTTP/1.1\r\n\r\n";
        let (code, state) = CallbackServer::parse_callback_request(request).unwrap();
        assert_eq!(code, "hello world");
        assert_eq!(state, "foo bar");
    }

    #[test]
    fn parse_callback_request_missing_code() {
        let request = "GET /cb?state=xyz HTTP/1.1\r\n\r\n";
        let result = CallbackServer::parse_callback_request(request);
        assert!(result.is_err());
        let err = result.unwrap_err().to_string();
        assert!(err.contains("missing 'code' parameter"));
    }

    #[test]
    fn parse_callback_request_missing_state() {
        let request = "GET /cb?code=abc HTTP/1.1\r\n\r\n";
        let result = CallbackServer::parse_callback_request(request);
        assert!(result.is_err());
        let err = result.unwrap_err().to_string();
        assert!(err.contains("missing 'state' parameter"));
    }

    #[test]
    fn parse_callback_request_no_query() {
        let request = "GET /cb HTTP/1.1\r\n\r\n";
        let result = CallbackServer::parse_callback_request(request);
        assert!(result.is_err());
        let err = result.unwrap_err().to_string();
        assert!(err.contains("no query string"));
    }

    #[test]
    fn parse_callback_request_error_param() {
        let request = "GET /cb?error=access_denied&state=xyz HTTP/1.1\r\n\r\n";
        let result = CallbackServer::parse_callback_request(request);
        assert!(result.is_err());
        let err = result.unwrap_err().to_string();
        assert!(err.contains("access_denied"));
    }

    #[test]
    fn parse_callback_request_empty() {
        let result = CallbackServer::parse_callback_request("");
        assert!(result.is_err());
    }

    #[test]
    fn parse_callback_request_malformed() {
        let result = CallbackServer::parse_callback_request("NOTHTTP");
        assert!(result.is_err());
    }

    #[test]
    fn percent_decode_basic() {
        assert_eq!(CallbackServer::percent_decode("hello"), "hello");
        assert_eq!(
            CallbackServer::percent_decode("hello%20world"),
            "hello world"
        );
        assert_eq!(CallbackServer::percent_decode("a%2Fb"), "a/b");
    }

    #[test]
    fn percent_decode_plus() {
        assert_eq!(CallbackServer::percent_decode("a+b"), "a b");
    }

    #[test]
    fn percent_decode_empty() {
        assert_eq!(CallbackServer::percent_decode(""), "");
    }

    #[tokio::test]
    async fn callback_server_receives_request() {
        // Start the server on an ephemeral port.
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = listener.local_addr().unwrap().port();

        // Spawn a task to send a mock request.
        let client_task = tokio::spawn(async move {
            // Give the server a moment to start accepting.
            tokio::time::sleep(tokio::time::Duration::from_millis(50)).await;

            let mut stream = tokio::net::TcpStream::connect(format!("127.0.0.1:{port}"))
                .await
                .unwrap();

            let request = format!(
                "GET /callback?code=test_code_42&state=test_state_99 HTTP/1.1\r\nHost: 127.0.0.1:{port}\r\nConnection: close\r\n\r\n"
            );

            stream.write_all(request.as_bytes()).await.unwrap();

            // Read the response.
            let mut buf = vec![0u8; 4096];
            let n = stream.read(&mut buf).await.unwrap();
            let response = String::from_utf8_lossy(&buf[..n]);
            assert!(response.contains("200 OK"));
            assert!(response.contains("Authorization Successful"));
        });

        // Accept the connection on the server side.
        let result = CallbackServer::accept_one(&listener).await;

        // Wait for the client to finish.
        client_task.await.unwrap();

        let (code, state) = result.unwrap();
        assert_eq!(code, "test_code_42");
        assert_eq!(state, "test_state_99");
    }

    #[tokio::test]
    async fn callback_server_timeout() {
        // Use a very short timeout to test the timeout behavior.
        let result = CallbackServer::start(0, 1).await;

        // On port 0 the OS picks an ephemeral port, but nobody connects.
        // The error depends on whether the bind succeeds (it should).
        // Since nobody connects, we expect a timeout.
        match result {
            Err(AuthEngineError::CallbackTimeout { timeout_secs }) => {
                assert_eq!(timeout_secs, 1);
            }
            // If bind fails on port 0 (unlikely), that is also acceptable.
            Err(AuthEngineError::Io(_)) => {}
            other => panic!("expected timeout or io error, got: {other:?}"),
        }
    }

    #[test]
    fn callback_server_is_send_sync() {
        fn assert_send_sync<T: Send + Sync>() {}
        assert_send_sync::<CallbackServer>();
    }
}
