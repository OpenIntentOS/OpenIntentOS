//! QQ Official Bot webhook verification and event parsing helpers.

use hmac::{Hmac, Mac};
use sha2::Sha256;

use super::types::QQBotEvent;

type HmacSha256 = Hmac<Sha256>;

/// Encode bytes as lowercase hex without external crates.
fn to_hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

/// Compute the webhook signature for request verification.
///
/// The QQ Bot platform sends an `X-Signature` header. This function computes
/// `HMAC-SHA256(app_secret, timestamp + nonce + body)` and returns the hex
/// result for comparison against the header value.
///
/// Returns the raw hex string (without any prefix).
pub fn verify_signature(app_secret: &str, timestamp: &str, nonce: &str, body: &str) -> String {
    let message = format!("{timestamp}{nonce}{body}");
    let mut mac = HmacSha256::new_from_slice(app_secret.as_bytes())
        .expect("HMAC key length is always valid");
    mac.update(message.as_bytes());
    to_hex(&mac.finalize().into_bytes())
}

/// Parse a raw webhook payload into a [`QQBotEvent`], if recognised.
pub fn parse_webhook_event(body: &serde_json::Value) -> Option<QQBotEvent> {
    serde_json::from_value(body.clone()).ok()
}
