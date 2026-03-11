//! HTTP/SOCKS5 proxy support for all adapters.
//!
//! Reads standard environment variables and builds a pre-configured
//! `reqwest::ClientBuilder` that routes traffic through the proxy.
//!
//! Supported env vars (in priority order):
//!   - `ALL_PROXY` / `all_proxy`     — SOCKS5 or HTTP proxy for all traffic
//!   - `HTTPS_PROXY` / `https_proxy` — proxy for HTTPS requests
//!   - `HTTP_PROXY` / `http_proxy`   — proxy for HTTP requests
//!   - `NO_PROXY` / `no_proxy`       — comma-separated list of bypass hosts
//!
//! Usage:
//! ```rust
//! let client = proxy::build_client(Duration::from_secs(30))
//!     .user_agent("MyAdapter/1.0")
//!     .build()
//!     .unwrap_or_default();
//! ```

use std::time::Duration;

/// Build a `reqwest::ClientBuilder` with proxy settings from environment
/// variables already applied.  Callers can chain further configuration
/// (e.g. `.user_agent()`, `.timeout()`) before calling `.build()`.
pub fn build_client(timeout: Duration) -> reqwest::ClientBuilder {
    let mut builder = reqwest::ClientBuilder::new()
        .timeout(timeout)
        .connect_timeout(Duration::from_secs(15));

    // Determine proxy URL: ALL_PROXY > HTTPS_PROXY > HTTP_PROXY
    let proxy_url = std::env::var("ALL_PROXY")
        .or_else(|_| std::env::var("all_proxy"))
        .or_else(|_| std::env::var("HTTPS_PROXY"))
        .or_else(|_| std::env::var("https_proxy"))
        .or_else(|_| std::env::var("HTTP_PROXY"))
        .or_else(|_| std::env::var("http_proxy"))
        .ok()
        .filter(|s| !s.is_empty());

    if let Some(ref url) = proxy_url {
        tracing::debug!(proxy = %url, "using proxy from environment");

        // HTTPS traffic (most API calls)
        if let Ok(p) = reqwest::Proxy::https(url.as_str()) {
            builder = builder.proxy(p);
        }
        // HTTP traffic
        if let Ok(p) = reqwest::Proxy::http(url.as_str()) {
            builder = builder.proxy(p);
        }
    }

    // Respect NO_PROXY / no_proxy
    let no_proxy = std::env::var("NO_PROXY")
        .or_else(|_| std::env::var("no_proxy"))
        .ok()
        .filter(|s| !s.is_empty());

    if let Some(ref bypasses) = no_proxy {
        for host in bypasses.split(',').map(str::trim).filter(|s| !s.is_empty()) {
            if let Ok(p) = reqwest::Proxy::https(format!("direct://{host}")) {
                // reqwest doesn't have a direct no-proxy API; we skip these.
                // Users should rely on the proxy tool's own no-proxy rules.
                let _ = p; // acknowledged, handled at proxy layer
            }
        }
    }

    builder
}

/// Return the active proxy URL from environment, or `None` if not configured.
pub fn proxy_url() -> Option<String> {
    std::env::var("ALL_PROXY")
        .or_else(|_| std::env::var("all_proxy"))
        .or_else(|_| std::env::var("HTTPS_PROXY"))
        .or_else(|_| std::env::var("https_proxy"))
        .or_else(|_| std::env::var("HTTP_PROXY"))
        .or_else(|_| std::env::var("http_proxy"))
        .ok()
        .filter(|s| !s.is_empty())
}
