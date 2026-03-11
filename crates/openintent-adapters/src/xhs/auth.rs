//! XiaoHongShu (小红书) HMAC-SHA256 request signing.
//!
//! Every API request must include signed headers constructed from the request
//! path, sorted query string, body, timestamp, and app_secret.
//!
//! Signature algorithm:
//! ```text
//! string_to_sign = path + sorted_query_string + body + timestamp + app_secret
//! x-sign = HMAC-SHA256(key=app_secret, message=string_to_sign) -> hex lowercase
//! ```

use std::collections::HashMap;
use std::time::{SystemTime, UNIX_EPOCH};

use hmac::{Hmac, Mac};
use sha2::Sha256;

type HmacSha256 = Hmac<Sha256>;

/// Compute an HMAC-SHA256 signature for an XHS API request.
///
/// # Arguments
/// - `app_secret`: the XHS application secret (used as both HMAC key and suffix)
/// - `path`: URL path, e.g. `/v2/notes/`
/// - `sorted_query`: query params sorted by key and joined as `k1=v1&k2=v2`
/// - `body`: raw request body string
/// - `timestamp`: Unix timestamp in seconds
pub fn compute_sign(
    app_secret: &str,
    path: &str,
    sorted_query: &str,
    body: &str,
    timestamp: u64,
) -> String {
    let string_to_sign = format!("{path}{sorted_query}{body}{timestamp}{app_secret}");

    let mut mac = HmacSha256::new_from_slice(app_secret.as_bytes())
        .expect("HMAC can take key of any size");
    mac.update(string_to_sign.as_bytes());
    let result = mac.finalize();
    let bytes = result.into_bytes();

    bytes.iter().map(|b| format!("{b:02x}")).collect::<String>()
}

/// Build signed headers for an XHS API request.
///
/// Returns a `HashMap` containing:
/// - `x-timestamp`
/// - `x-app-key`
/// - `x-sign`
/// - `Content-Type`
pub fn build_signed_headers(
    app_key: &str,
    app_secret: &str,
    path: &str,
    query_params: &[(String, String)],
    body: &str,
) -> HashMap<String, String> {
    let timestamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs();

    // Sort query params by key and join as k=v pairs.
    let mut sorted = query_params.to_vec();
    sorted.sort_by(|a, b| a.0.cmp(&b.0));
    let sorted_query = sorted
        .iter()
        .map(|(k, v)| format!("{k}={v}"))
        .collect::<Vec<_>>()
        .join("&");

    let sign = compute_sign(app_secret, path, &sorted_query, body, timestamp);

    let mut headers = HashMap::new();
    headers.insert("x-timestamp".to_string(), timestamp.to_string());
    headers.insert("x-app-key".to_string(), app_key.to_string());
    headers.insert("x-sign".to_string(), sign);
    headers.insert(
        "Content-Type".to_string(),
        "application/json;charset=utf-8".to_string(),
    );

    headers
}
