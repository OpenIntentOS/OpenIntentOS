//! Helper functions for the Telegram bot gateway.
//!
//! Extracted from `bot.rs` to keep that module under the 1000-line limit.

use std::sync::Arc;

use openintent_agent::{
    AgentContext, ChatRequest, LlmClient, LlmResponse, Message, react_loop,
};
use openintent_store::SessionStore;
use tracing::info;

use crate::failover::{self, FailoverManager};
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

/// Handle an expired OAuth token: refresh from Keychain, fall back to
/// DeepSeek, and retry the agent loop.
///
/// Returns the reply text from the retry, or a translated error message.
pub async fn handle_auth_error(
    ctx: &AgentContext,
    llm: &Arc<LlmClient>,
    adapters: &[Arc<dyn openintent_agent::ToolAdapter>],
    user_lang: &str,
    msgs: &Messages,
    model: &str,
    chat_id: i64,
) -> String {
    tracing::warn!("OAuth token expired, attempting refresh from Keychain");

    // Try 1: refresh from Keychain.
    let refreshed =
        if let Some(new_token) = crate::helpers::read_claude_code_keychain_token() {
            llm.update_api_key(new_token);
            tracing::info!("OAuth token refreshed from Keychain, retrying");
            true
        } else {
            false
        };

    // Try 2: if Keychain failed, fall back to DeepSeek.
    if !refreshed {
        if let Some(ds_key) = crate::helpers::env_non_empty("DEEPSEEK_API_KEY") {
            tracing::warn!("Keychain refresh failed, falling back to DeepSeek");
            llm.update_api_key(ds_key);
            llm.switch_provider(
                openintent_agent::LlmProvider::OpenAI,
                "https://api.deepseek.com/v1".to_owned(),
                "deepseek-chat".to_owned(),
            );
        } else {
            tracing::error!(
                "no fallback: Keychain refresh failed and DEEPSEEK_API_KEY not set"
            );
            return msgs
                .get_translated(keys::ERROR_GENERAL, &[], user_lang, llm, model)
                .await;
        }
    }

    // Retry with refreshed/fallback credentials.
    let retry_config = ctx.config.clone();
    let mut retry_ctx = AgentContext::new(llm.clone(), adapters.to_vec(), retry_config);
    retry_ctx.messages = ctx.messages.clone();

    match react_loop(&mut retry_ctx).await {
        Ok(response) => {
            info!(
                chat_id,
                turns = response.turns_used,
                "agent completed (after token refresh)"
            );
            response.text
        }
        Err(retry_err) => {
            tracing::error!(error = %retry_err, "agent error after token refresh");
            msgs.get_translated(keys::ERROR_GENERAL, &[], user_lang, llm, model)
                .await
        }
    }
}

/// Result of a cascading failover attempt.
pub struct CascadeResult {
    /// The reply text (if a provider succeeded), or None if all exhausted.
    pub reply: Option<String>,
    /// The model that ended up being used (may differ from the starting model).
    pub final_model: String,
    /// Whether all providers were exhausted (need to restore primary).
    pub all_exhausted: bool,
}

/// Cascade through failover providers, retrying the agent loop with each
/// until one succeeds or all are exhausted.
///
/// Sends Telegram notifications to the user for each automatic switch.
pub async fn handle_cascade_failover(
    ctx: &AgentContext,
    llm: &Arc<LlmClient>,
    adapters: &[Arc<dyn openintent_agent::ToolAdapter>],
    failover_mgr: &mut FailoverManager,
    current_model: &str,
    chat_id: i64,
    http: &reqwest::Client,
    telegram_api: &str,
) -> CascadeResult {
    let mut model = current_model.to_string();

    while let Some(fo) = failover_mgr.try_failover(&model, llm) {
        let old_model = model.clone();
        model = fo.model.clone();

        // Notify user about the automatic switch.
        let notice = format!(
            "Rate limit on `{}`. Auto-switched to **{}** (`{}`).",
            old_model, fo.provider_name, fo.model
        );
        let _ = http
            .post(format!("{telegram_api}/sendMessage"))
            .json(&serde_json::json!({
                "chat_id": chat_id,
                "text": notice,
                "parse_mode": "Markdown",
            }))
            .send()
            .await;

        // Retry with the new provider.
        let mut retry_config = ctx.config.clone();
        retry_config.model = model.clone();
        let mut retry_ctx =
            AgentContext::new(llm.clone(), adapters.to_vec(), retry_config);
        retry_ctx.messages = ctx.messages.clone();

        match react_loop(&mut retry_ctx).await {
            Ok(response) => {
                info!(
                    chat_id,
                    turns = response.turns_used,
                    new_model = %model,
                    "agent completed (after cascading failover)"
                );
                return CascadeResult {
                    reply: Some(response.text),
                    final_model: model,
                    all_exhausted: false,
                };
            }
            Err(retry_err) => {
                let retry_str = retry_err.to_string();
                tracing::warn!(
                    error = %retry_str,
                    provider = fo.provider_name,
                    "failover provider also failed, trying next"
                );
                failover_mgr.mark_rate_limited(&fo.model);
            }
        }
    }

    tracing::error!("all fallback providers exhausted");
    CascadeResult {
        reply: None,
        final_model: model,
        all_exhausted: true,
    }
}
