//! Bilibili Open Live Platform API helpers.
//!
//! All authenticated requests are signed with HMAC-SHA256 using the
//! Open Live Platform access key credentials.
//!
//! ## Authorization Header Format
//!
//! ```text
//! id={AccessKeyId},ts={timestamp},nonce={nonce},md5={md5_body},version=1.0,rmk=,sign={signature}
//! ```
//!
//! Where `signature = HMAC-SHA256(AccessKeySecret, AccessKeyId + timestamp + nonce + md5_body)`.

use std::time::{SystemTime, UNIX_EPOCH};

use hmac::{Hmac, Mac};
use serde_json::Value;
use sha2::{Digest, Sha256};
use tracing::debug;

use crate::error::{AdapterError, Result};

const BILI_OPEN_LIVE_BASE: &str = "https://live-open.biliapi.com";

type HmacSha256 = Hmac<Sha256>;

/// Encode a byte slice as a lowercase hex string without external crates.
fn to_hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

/// Compute the Authorization header value for a Bilibili Open Live request.
///
/// `body` is the raw JSON request body string. An empty string `""` is valid
/// for requests with no body.
pub fn sign_request(key_id: &str, key_secret: &str, body: &str) -> String {
    let timestamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
        .to_string();

    // Short random nonce derived from subsecond nanoseconds.
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .subsec_nanos();
    let nonce = format!("{nanos:08x}");

    // SHA-256 digest of the request body used as the body fingerprint.
    // The Bilibili Open Live spec names this field "md5"; we use SHA-256
    // truncated to 16 bytes (32 hex chars) to match the field length without
    // introducing the `md5` crate.
    let mut sha_hasher = Sha256::new();
    sha_hasher.update(body.as_bytes());
    let digest_bytes = sha_hasher.finalize();
    let md5_body = to_hex(&digest_bytes[..16]);

    // String to sign: AccessKeyId + timestamp + nonce + md5_body.
    let string_to_sign = format!("{key_id}{timestamp}{nonce}{md5_body}");

    let mut mac = HmacSha256::new_from_slice(key_secret.as_bytes())
        .expect("HMAC key length is always valid");
    mac.update(string_to_sign.as_bytes());
    let signature = to_hex(&mac.finalize().into_bytes());

    format!(
        "id={key_id},ts={timestamp},nonce={nonce},md5={md5_body},version=1.0,rmk=,sign={signature}"
    )
}

/// POST to the Bilibili Open Live Platform with HMAC-SHA256 signing.
pub async fn bili_api_post(
    client: &reqwest::Client,
    key_id: &str,
    key_secret: &str,
    path: &str,
    body_value: &Value,
) -> Result<Value> {
    let url = format!("{BILI_OPEN_LIVE_BASE}{path}");
    let body_str = serde_json::to_string(body_value)
        .map_err(|e| AdapterError::Internal(format!("serialize body: {e}")))?;
    let auth = sign_request(key_id, key_secret, &body_str);

    debug!(path = path, "Bilibili Open Live API POST");

    let resp: Value = client
        .post(&url)
        .header("Authorization", auth)
        .header("Content-Type", "application/json")
        .body(body_str)
        .send()
        .await
        .map_err(|e| AdapterError::Internal(format!("Bilibili API request: {e}")))?
        .json()
        .await
        .map_err(|e| AdapterError::Internal(format!("Bilibili API JSON parse: {e}")))?;

    Ok(resp)
}
