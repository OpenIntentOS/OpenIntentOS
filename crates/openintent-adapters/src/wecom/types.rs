//! WeCom (企业微信) message types.

use serde::{Deserialize, Serialize};

/// Text message body for WeCom API.
#[derive(Debug, Serialize, Deserialize)]
pub struct WeComTextMsg {
    pub msgtype: &'static str,
    pub text: WeComTextContent,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct WeComTextContent {
    pub content: String,
}

/// Markdown message body for WeCom API.
#[derive(Debug, Serialize, Deserialize)]
pub struct WeComMarkdownMsg {
    pub msgtype: &'static str,
    pub markdown: WeComMarkdownContent,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct WeComMarkdownContent {
    pub content: String,
}

/// App-mode text message (includes touser/agentid).
#[derive(Debug, Serialize, Deserialize)]
pub struct WeComAppTextMsg {
    pub touser: String,
    pub msgtype: &'static str,
    pub agentid: u64,
    pub text: WeComTextContent,
}

/// App-mode markdown message.
#[derive(Debug, Serialize, Deserialize)]
pub struct WeComAppMarkdownMsg {
    pub touser: String,
    pub msgtype: &'static str,
    pub agentid: u64,
    pub markdown: WeComMarkdownContent,
}

/// News card article item.
#[derive(Debug, Serialize, Deserialize)]
pub struct WeComNewsArticle {
    pub title: String,
    pub description: String,
    pub url: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub picurl: Option<String>,
}

/// News card message.
#[derive(Debug, Serialize, Deserialize)]
pub struct WeComNewsMsg {
    pub msgtype: &'static str,
    pub news: WeComNewsContent,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct WeComNewsContent {
    pub articles: Vec<WeComNewsArticle>,
}
