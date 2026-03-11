//! Weibo API response types.

/// Response from the Weibo OAuth2 token endpoint.
#[derive(Debug, serde::Deserialize)]
pub struct WeiboTokenResponse {
    pub access_token: String,
    pub expires_in: u64,
    pub uid: String,
}

/// A Weibo status (post/tweet).
#[derive(Debug, serde::Deserialize)]
pub struct WeiboStatus {
    pub id: u64,
    pub text: String,
    pub user: Option<WeiboUser>,
}

/// A Weibo user profile embedded in status responses.
#[derive(Debug, serde::Deserialize)]
pub struct WeiboUser {
    pub id: u64,
    pub name: String,
    pub screen_name: String,
}
