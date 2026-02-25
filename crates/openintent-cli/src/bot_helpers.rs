//! Helper functions for the Telegram bot gateway.
//!
//! Extracted from `bot.rs` to keep that module under the 1000-line limit.

use std::sync::Arc;

use openintent_agent::{ChatRequest, LlmClient, LlmResponse, Message};
use openintent_store::SessionStore;
use tracing::info;

use crate::messages::{self, Messages, keys};

/// Split a message into chunks that fit within Telegram's character limit.
///
/// Respects UTF-8 char boundaries to avoid panics on multi-byte characters.
pub fn split_telegram_message(text: &str, max_len: usize) -> Vec<String> {
    if text.len() <= max_len {
        return vec![text.to_owned()];
    }

    let mut chunks = Vec::new();
    let mut remaining = text;

    while !remaining.is_empty() {
        if remaining.len() <= max_len {
            chunks.push(remaining.to_owned());
            break;
        }

        // Find the last char boundary at or before max_len.
        let mut boundary = max_len;
        while boundary > 0 && !remaining.is_char_boundary(boundary) {
            boundary -= 1;
        }

        let mut split_at = remaining[..boundary]
            .rfind('\n')
            .unwrap_or_else(|| remaining[..boundary].rfind(' ').unwrap_or(boundary));

        // Guard against infinite loop: if split_at is 0, force it to boundary.
        if split_at == 0 {
            split_at = boundary;
        }

        chunks.push(remaining[..split_at].to_owned());
        remaining = remaining[split_at..].trim_start();
    }

    chunks
}

/// Send a startup notification to all recently active Telegram chats,
/// informing users about the latest changes after a restart.
pub async fn send_startup_notification(
    http: &reqwest::Client,
    telegram_api: &str,
    sessions: &SessionStore,
    llm: &Arc<LlmClient>,
    model: &str,
    msgs: &Messages,
) {
    // Get the latest commit messages for context.
    let commit_info = match std::process::Command::new("git")
        .args(["log", "--oneline", "-5"])
        .output()
    {
        Ok(output) if output.status.success() => {
            String::from_utf8(output.stdout)
                .ok()
                .map(|s| s.trim().to_string())
        }
        _ => None,
    };

    // Find all recently active Telegram sessions.
    let chat_ids = match sessions.list(100, 0).await {
        Ok(all_sessions) => {
            all_sessions
                .iter()
                .filter_map(|s| {
                    if s.name.starts_with("telegram-") {
                        s.name.strip_prefix("telegram-")?.parse::<i64>().ok()
                    } else {
                        None
                    }
                })
                .collect::<Vec<_>>()
        }
        Err(_) => Vec::new(),
    };

    for chat_id in chat_ids {
        // Try to get the user's language from their most recent message.
        // Default to "en" if unknown.
        let user_lang = get_chat_language(http, telegram_api, chat_id).await;

        // Generate a human-readable summary using the LLM in the user's language.
        let message = if let Some(ref commits) = commit_info {
            match summarize_commits_for_user(llm, model, commits, &user_lang).await {
                Some(summary) => {
                    msgs.get_translated(
                        keys::STARTUP_WITH_UPDATES,
                        &[("summary", &summary)],
                        &user_lang,
                        llm,
                        model,
                    )
                    .await
                }
                None => {
                    msgs.get_translated(keys::STARTUP_SIMPLE, &[], &user_lang, llm, model)
                        .await
                }
            }
        } else {
            msgs.get_translated(keys::STARTUP_SIMPLE, &[], &user_lang, llm, model)
                .await
        };

        let _ = http
            .post(format!("{telegram_api}/sendMessage"))
            .json(&serde_json::json!({
                "chat_id": chat_id,
                "text": message,
            }))
            .send()
            .await;
    }

    if commit_info.is_some() {
        info!("sent startup notification to active chats");
    }
}

/// Use the LLM to translate raw git commit messages into a short,
/// human-readable summary in the user's language.
async fn summarize_commits_for_user(
    llm: &Arc<LlmClient>,
    model: &str,
    commits: &str,
    user_lang: &str,
) -> Option<String> {
    let lang_name = messages::lang_display_name(user_lang);

    let system = format!(
        "You are an AI assistant. Translate the git commit log below into a short, \
         friendly summary in {lang_name} that a non-technical user can understand. \
         Describe what features were added or bugs were fixed. \
         Do NOT mention commit hashes, branch names, or file names. \
         Use 2-4 bullet points, each one sentence, starting with an emoji."
    );

    let request = ChatRequest {
        model: model.to_owned(),
        messages: vec![
            Message::system(&system),
            Message::user(format!(
                "Summarize these updates in {lang_name}:\n{commits}"
            )),
        ],
        tools: vec![],
        temperature: Some(0.3),
        max_tokens: Some(512),
        stream: false,
    };

    match llm.chat(&request).await {
        Ok(LlmResponse::Text(text)) => Some(text),
        _ => None,
    }
}

/// Try to determine a chat's language. Returns "en" as default.
async fn get_chat_language(
    _http: &reqwest::Client,
    _telegram_api: &str,
    _chat_id: i64,
) -> String {
    // Telegram doesn't provide language_code for chats directly,
    // only in message updates. We default to "en" for startup
    // notifications. Once the user sends a message, their language
    // is detected and used for subsequent messages.
    "en".to_string()
}
