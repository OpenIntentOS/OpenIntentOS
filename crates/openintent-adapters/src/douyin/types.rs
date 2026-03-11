//! Douyin API response types.

/// Response from Douyin OAuth token endpoints.
#[derive(Debug, serde::Deserialize)]
pub struct DouyinTokenResponse {
    pub access_token: String,
    pub open_id: String,
    pub refresh_token: String,
    pub expires_in: u64,
    pub refresh_expires_in: u64,
    pub scope: String,
}

/// Generic Douyin API wrapper.
#[derive(Debug, serde::Deserialize)]
pub struct DouyinApiResponse<T> {
    pub data: Option<T>,
    pub message: String,
}
