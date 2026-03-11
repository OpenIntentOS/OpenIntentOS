//! QQ Official Bot message and event types.

use serde::{Deserialize, Serialize};

/// A parsed QQ Bot event received via webhook.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct QQBotEvent {
    /// The event type string (e.g. "GROUP_AT_MESSAGE_CREATE").
    pub op: u32,
    #[serde(rename = "t")]
    pub event_type: Option<String>,
    #[serde(rename = "d")]
    pub data: Option<serde_json::Value>,
}

/// Text message body for sending to a channel.
#[derive(Debug, Serialize, Deserialize)]
pub struct QQChannelTextMsg {
    pub content: String,
}

/// Image message body for sending to a channel.
#[derive(Debug, Serialize, Deserialize)]
pub struct QQChannelImageMsg {
    /// Public URL of the image.
    pub image: String,
    pub msg_type: u32,
}

/// C2C text message body.
#[derive(Debug, Serialize, Deserialize)]
pub struct QQC2CTextMsg {
    pub content: String,
    pub msg_type: u32,
}

/// C2C image message body.
#[derive(Debug, Serialize, Deserialize)]
pub struct QQC2CImageMsg {
    pub image: String,
    pub msg_type: u32,
}
