//! WeChat OA request/response types.

use serde::{Deserialize, Serialize};

/// Outbound text message body.
#[derive(Debug, Serialize)]
pub struct TextMessage<'a> {
    pub touser: &'a str,
    pub msgtype: &'static str,
    pub text: TextContent<'a>,
}

#[derive(Debug, Serialize)]
pub struct TextContent<'a> {
    pub content: &'a str,
}

/// Outbound image message body.
#[derive(Debug, Serialize)]
pub struct ImageMessage<'a> {
    pub touser: &'a str,
    pub msgtype: &'static str,
    pub image: ImageContent<'a>,
}

#[derive(Debug, Serialize)]
pub struct ImageContent<'a> {
    pub media_id: &'a str,
}

/// Follower list response.
#[derive(Debug, Deserialize)]
pub struct FollowerListResponse {
    pub total: u64,
    pub count: u64,
    pub data: Option<FollowerData>,
    pub next_openid: Option<String>,
}

#[derive(Debug, Deserialize)]
pub struct FollowerData {
    pub openid: Vec<String>,
}

/// User info response.
#[derive(Debug, Deserialize)]
pub struct UserInfoResponse {
    pub openid: String,
    pub nickname: Option<String>,
    pub sex: Option<u8>,
    pub province: Option<String>,
    pub city: Option<String>,
    pub country: Option<String>,
    pub headimgurl: Option<String>,
    pub subscribe: Option<u8>,
    pub subscribe_time: Option<u64>,
}
