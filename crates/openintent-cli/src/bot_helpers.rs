//! Helper functions for the Telegram bot gateway.
//!
//! Extracted from `bot.rs` to keep that module under the 1000-line limit.

use std::sync::Arc;

use openintent_agent::{
    AgentContext, ChatRequest, LlmClient, LlmResponse, Message, react_loop,
};
use openintent_store::{BotStateStore, SessionStore};
use tracing::info;

use crate::failover::{self, FailoverManager};
use crate::messages::{self, Messages, keys};

/// Send the /start welcome message to a chat.
pub async fn send_start_message(
    http: &reqwest::Client,
    telegram_api: &str,
    chat_id: i64,
    msgs: &Messages,
    llm: &Arc<LlmClient>,
    model: &str,
    user_lang: &str,
) {
    let text = msgs
        .get_translated(keys::BOT_START, &[], user_lang, llm, model)
        .await;
    let _ = http
        .post(format!("{telegram_api}/sendMessage"))
        .json(&serde_json::json!({ "chat_id": chat_id, "text": text }))
        .send()
        .await;
}

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

// ---------------------------------------------------------------------------
// Task recovery notifications
// ---------------------------------------------------------------------------

/// Notify users about dev tasks that were in-progress or pending at restart.
pub async fn notify_recovered_tasks(
    http: &reqwest::Client,
    telegram_api: &str,
    dev_task_store: &openintent_store::DevTaskStore,
) {
    use crate::messages::safe_prefix;

    if let Ok(recoverable) = dev_task_store.list_recoverable().await {
        for task in &recoverable {
            if let Some(cid) = task.chat_id {
                let short_id = safe_prefix(&task.id, 8);
                let _ = http
                    .post(format!("{telegram_api}/sendMessage"))
                    .json(&serde_json::json!({
                        "chat_id": cid,
                        "text": format!(
                            "Bot restarted. Resuming your task [{short_id}]...\n\
                             Intent: {}\nStatus: {}",
                            task.intent, task.status
                        ),
                    }))
                    .send()
                    .await;
            }
        }
        if let Ok(pending) = dev_task_store.list_by_status("pending", 50, 0).await {
            for task in &pending {
                if let Some(cid) = task.chat_id {
                    let short_id = safe_prefix(&task.id, 8);
                    let _ = http
                        .post(format!("{telegram_api}/sendMessage"))
                        .json(&serde_json::json!({
                            "chat_id": cid,
                            "text": format!(
                                "Bot restarted. Your pending task [{short_id}] will be processed shortly.\n\
                                 Intent: {}",
                                task.intent
                            ),
                        }))
                        .send()
                        .await;
                }
            }
        }
    }
}

// ---------------------------------------------------------------------------
// Post-update restart handling
// ---------------------------------------------------------------------------

/// If the `system_self_update` tool set the restart signal during this
/// message cycle, persist the notification state and exit so the process
/// manager (systemd / launchd) restarts with the new binary.
pub async fn check_restart_signal(
    restart_signal: &crate::self_update_adapter::RestartSignal,
    bot_state: &BotStateStore,
    chat_id: i64,
) {
    let Some(new_version) = restart_signal.lock().unwrap().take() else {
        return;
    };
    let _ = bot_state
        .set("update_from_version", env!("CARGO_PKG_VERSION"))
        .await;
    let _ = bot_state.set("update_to_version", &new_version).await;
    let _ = bot_state
        .set("update_notify_chat_ids", &chat_id.to_string())
        .await;
    tracing::info!(
        from = env!("CARGO_PKG_VERSION"),
        to = %new_version,
        "restarting after self-update"
    );
    tokio::time::sleep(std::time::Duration::from_millis(300)).await;
    std::process::exit(0);
}

// Keys used in BotStateStore for the post-restart update notification.
const KEY_UPDATE_FROM: &str = "update_from_version";
const KEY_UPDATE_TO: &str = "update_to_version";
const KEY_UPDATE_CHATS: &str = "update_notify_chat_ids";

/// On startup, check if a pending update notification was stored before the
/// last restart and, if so, send the confirmation to the affected chat(s).
pub async fn send_pending_update_notification(
    http: &reqwest::Client,
    telegram_api: &str,
    bot_state: &BotStateStore,
    msgs: &Messages,
    llm: &Arc<LlmClient>,
    model: &str,
) {
    let from = match bot_state.get(KEY_UPDATE_FROM).await.ok().flatten() {
        Some(v) => v,
        None => return,
    };
    let to = match bot_state.get(KEY_UPDATE_TO).await.ok().flatten() {
        Some(v) => v,
        None => return,
    };
    let chats_raw = match bot_state.get(KEY_UPDATE_CHATS).await.ok().flatten() {
        Some(v) => v,
        None => return,
    };

    // Clear the pending flags before sending so a crash mid-send doesn't loop.
    let _ = bot_state.delete(KEY_UPDATE_FROM).await;
    let _ = bot_state.delete(KEY_UPDATE_TO).await;
    let _ = bot_state.delete(KEY_UPDATE_CHATS).await;

    for chat_str in chats_raw.split(',') {
        if let Ok(cid) = chat_str.trim().parse::<i64>() {
            // Determine each chat's language (defaults to "en" at startup).
            let user_lang = get_chat_language(http, telegram_api, cid).await;
            let message = msgs
                .get_translated(
                    keys::UPDATE_CONFIRMED,
                    &[("from", &from), ("to", &to)],
                    &user_lang,
                    llm,
                    model,
                )
                .await;
            let _ = http
                .post(format!("{telegram_api}/sendMessage"))
                .json(&serde_json::json!({
                    "chat_id": cid,
                    "text": message,
                }))
                .send()
                .await;
        }
    }

    info!(from = %from, to = %to, "sent post-update notification");
}

// ---------------------------------------------------------------------------
// Token usage stats
// ---------------------------------------------------------------------------

/// Send a token usage stats message to a Telegram chat when enabled.
pub async fn send_token_stats(
    http: &reqwest::Client,
    telegram_api: &str,
    chat_id: i64,
    input: u32,
    output: u32,
    msgs: &crate::messages::Messages,
) {
    use crate::messages::keys;

    if input + output == 0 {
        return;
    }
    let total = input + output;
    let stats_msg = msgs.get_with(
        keys::BOT_TOKEN_USAGE,
        &[
            ("input", &input.to_string()),
            ("output", &output.to_string()),
            ("total", &total.to_string()),
        ],
    );
    let _ = http
        .post(format!("{telegram_api}/sendMessage"))
        .json(&serde_json::json!({
            "chat_id": chat_id,
            "text": stats_msg,
        }))
        .send()
        .await;
}
