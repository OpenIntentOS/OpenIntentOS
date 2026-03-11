//! DingTalk message types.

use serde::Serialize;

/// Webhook text message.
#[derive(Debug, Serialize)]
pub struct WebhookTextMsg<'a> {
    pub msgtype: &'static str,
    pub text: TextContent<'a>,
    pub at: AtConfig<'a>,
}

#[derive(Debug, Serialize)]
pub struct TextContent<'a> {
    pub content: &'a str,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AtConfig<'a> {
    pub at_mobiles: &'a [String],
    pub is_at_all: bool,
}

/// Webhook markdown message.
#[derive(Debug, Serialize)]
pub struct WebhookMarkdownMsg<'a> {
    pub msgtype: &'static str,
    pub markdown: MarkdownContent<'a>,
    pub at: AtConfig<'a>,
}

#[derive(Debug, Serialize)]
pub struct MarkdownContent<'a> {
    pub title: &'a str,
    pub text: &'a str,
}

/// Webhook action card message.
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct WebhookActionCardMsg<'a> {
    pub msgtype: &'static str,
    pub action_card: ActionCardContent<'a>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ActionCardContent<'a> {
    pub title: &'a str,
    pub text: &'a str,
    pub btn_orientation: &'static str,
    pub btns: Vec<ActionButton>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ActionButton {
    pub title: String,
    pub action_url: String,
}
