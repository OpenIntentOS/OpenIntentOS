//! Bilibili Open Live Platform types.

use serde::{Deserialize, Serialize};

/// Basic live room information from the public API.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BiliLiveRoom {
    pub room_id: u64,
    pub uid: u64,
    pub title: String,
    pub live_status: u8,
    pub online: u64,
}

/// A danmaku (弹幕) entry.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BiliDanmaku {
    pub uid: u64,
    pub uname: String,
    pub msg: String,
    pub timestamp: u64,
}
