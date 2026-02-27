//! Subcommand: `openintent bot` -- Telegram bot gateway.
//!
//! Polls Telegram for incoming messages, runs each through the ReAct loop,
//! and sends responses back. Supports per-chat conversation history, access
//! control, session persistence, self-evolution, and self-development tasks.

use std::collections::HashMap;
use std::path::Path;
use std::sync::Arc;

use anyhow::{Context, Result};
use tracing::info;

use openintent_agent::{
    AgentConfig, AgentContext, EvolutionEngine, LlmClient, Message, react_loop,
};
use openintent_store::{BotStateStore, DevTaskStore, SessionStore};

use crate::adapters::init_adapters;
use crate::bot_config::{load_bot_config, select_model_for_query};
use crate::bot_helpers::{
    check_restart_signal, notify_recovered_tasks, send_pending_update_notification,
    send_start_message, send_startup_notification, send_token_stats, split_telegram_message,
};
use crate::dev_commands;
use crate::dev_worker::{DevWorker, ProgressCallback};
use crate::failover::{self, FailoverManager};
use crate::helpers::{env_non_empty, init_tracing, load_system_prompt, resolve_llm_config};
use crate::messages::{self, Messages, keys};

/// Run the Telegram bot gateway.
pub async fn cmd_bot(poll_timeout: u64, allowed_users: Option<String>) -> Result<()> {
    init_tracing("info");
    info!("starting Telegram bot gateway");

    // Install a global panic hook that LOGS instead of crashing.
    // The default hook prints to stderr and aborts; ours logs and continues.
    std::panic::set_hook(Box::new(|info| {
        let location = info
            .location()
            .map(|l| format!("{}:{}:{}", l.file(), l.line(), l.column()))
            .unwrap_or_else(|| "unknown".to_string());
        let payload = if let Some(s) = info.payload().downcast_ref::<&str>() {
            s.to_string()
        } else if let Some(s) = info.payload().downcast_ref::<String>() {
            s.clone()
        } else {
            "unknown panic".to_string()
        };
        tracing::error!(location = %location, payload = %payload, "PANIC caught (non-fatal)");
    }));

    // Load user-facing message templates from config.
    let msgs = Messages::load();

    // Parse allowed user IDs (if provided).
    let allowed_user_ids: Option<Vec<i64>> = allowed_users.map(|s| {
        s.split(',')
            .filter_map(|id| id.trim().parse::<i64>().ok())
            .collect()
    });

    // Resolve Telegram bot token.
    let bot_token = env_non_empty("TELEGRAM_BOT_TOKEN").ok_or_else(|| {
        anyhow::anyhow!("TELEGRAM_BOT_TOKEN is required. Create a bot at https://t.me/BotFather")
    })?;

    let telegram_api = format!("https://api.telegram.org/bot{bot_token}");

    // Verify the token by calling getMe.
    let http = reqwest::Client::new();
    let me: serde_json::Value = http
        .get(format!("{telegram_api}/getMe"))
        .send()
        .await
        .context("failed to reach Telegram API")?
        .json()
        .await
        .context("failed to parse getMe response")?;

    let bot_name = me
        .pointer("/result/username")
        .and_then(|v| v.as_str())
        .unwrap_or("unknown");

    if me.get("ok").and_then(|v| v.as_bool()) != Some(true) {
        anyhow::bail!("Telegram getMe failed: {me}");
    }

    // Database, LLM, adapters -- shared initialization.
    let data_dir = Path::new("data");
    if !data_dir.exists() {
        std::fs::create_dir_all(data_dir).context("failed to create data directory")?;
    }

    let db_path = data_dir.join("openintent.db");
    let db = openintent_store::Database::open_and_migrate(db_path.clone())
        .await
        .context("failed to open database")?;

    let llm_config = resolve_llm_config();
    let provider_label = format!("{:?}", llm_config.provider);
    let primary_model = llm_config.default_model.clone();
    let mut model = primary_model.clone();
    let llm = Arc::new(LlmClient::new(llm_config).context("failed to create LLM client")?);

    // Load [bot] configuration from config/default.toml.
    let bot_cfg = load_bot_config();
    let history_window: u32 = bot_cfg.history_window;
    let global_show_token_usage: bool = bot_cfg.show_token_usage;
    let simple_query_threshold: usize = bot_cfg.simple_query_threshold;

    // Per-chat model alias. Maps chat_id → alias string (e.g. "gemini-flash").
    // Used to re-apply model switches after failover resets.
    let mut chat_model_alias: HashMap<i64, String> = HashMap::new();

    let sessions = SessionStore::new(db.clone());

    // Initialize the dev task store and bot state store.
    let dev_task_store = DevTaskStore::new(db.clone());
    let bot_state = BotStateStore::new(db.clone());

    // Initialize adapters.
    let cwd = std::env::current_dir().context("failed to get current directory")?;
    let initialized = init_adapters(cwd.clone(), db, true).await?;
    let mut adapters = initialized.tool_adapters;
    let skill_prompt_ext = initialized.skill_prompt_ext;

    let restart_signal = crate::self_update_adapter::RestartSignal::default();
    adapters.push(std::sync::Arc::new(crate::self_update_adapter::SelfUpdateAdapter::new(restart_signal.clone())));

    // Initialize the self-evolution engine.
    let evolution = EvolutionEngine::from_env();
    let evolution_status = if evolution.is_some() {
        "enabled"
    } else {
        "disabled (set GITHUB_TOKEN to enable)"
    };

    // Spawn the DevWorker as a background task.
    let dev_worker_store = dev_task_store.clone();
    let dev_worker_llm = llm.clone();
    let dev_worker_adapters = adapters.clone();
    let dev_worker_model = model.clone();
    let repo_path = cwd.clone();
    let dev_worker_repo_path = cwd;

    let progress_cb: ProgressCallback = {
        let http = http.clone();
        let telegram_api = telegram_api.clone();
        Arc::new(move |chat_id: i64, message: &str| {
            let http = http.clone();
            let api = telegram_api.clone();
            let msg = message.to_string();
            Box::pin(async move {
                let _ = http
                    .post(format!("{api}/sendMessage"))
                    .json(&serde_json::json!({
                        "chat_id": chat_id,
                        "text": msg,
                    }))
                    .send()
                    .await;
            })
        })
    };

    tokio::spawn(async move {
        let worker = DevWorker::new(
            dev_worker_store,
            dev_worker_llm,
            dev_worker_adapters,
            dev_worker_model,
            dev_worker_repo_path,
        )
        .with_progress_callback(progress_cb);

        worker.run().await;
    });

    info!("DevWorker spawned as background task");

    // Notify users about recovered tasks.
    notify_recovered_tasks(&http, &telegram_api, &dev_task_store).await;

    // Print banner.
    println!();
    println!(
        "  OpenIntentOS Telegram Bot Gateway v{}",
        env!("CARGO_PKG_VERSION")
    );
    println!("  Bot: @{bot_name}");
    println!("  Provider: {provider_label}");
    println!("  Model: {model}");
    println!("  Evolution: {evolution_status}");
    println!("  DevWorker: enabled");
    if let Some(ref ids) = allowed_user_ids {
        println!("  Allowed users: {:?}", ids);
    } else {
        println!("  Allowed users: everyone");
    }
    println!("  Long-poll timeout: {poll_timeout}s");
    println!();
    println!("  Bot is running. Send messages to @{bot_name} on Telegram.");
    println!("  Press Ctrl+C to stop.");
    println!();

    // Confirm upgrade to the user who triggered it (if bot restarted after update).
    send_pending_update_notification(&http, &telegram_api, &bot_state, &msgs, &llm, &model).await;

    // Send startup notification with latest changes to all recent active chats.
    send_startup_notification(&http, &telegram_api, &sessions, &llm, &model, &msgs).await;

    // Per-chat state maps.
    let mut chat_histories: HashMap<i64, Vec<Message>> = HashMap::new();
    let mut user_languages: HashMap<i64, String> = HashMap::new();
    let mut chat_show_tokens: HashMap<i64, bool> = HashMap::new();
    let mut failover_mgr = FailoverManager::new();

    // Polling loop -- restore offset from persistent state.
    let mut offset: i64 = bot_state
        .get_i64("telegram_offset")
        .await
        .unwrap_or(None)
        .unwrap_or(0);
    if offset > 0 {
        info!(offset, "restored Telegram polling offset from database");
    }

    loop {
        let updates_resp: std::result::Result<reqwest::Response, reqwest::Error> = http
            .post(format!("{telegram_api}/getUpdates"))
            .json(&serde_json::json!({
                "offset": offset,
                "timeout": poll_timeout,
                "allowed_updates": ["message", "callback_query"],
            }))
            .send()
            .await;

        let updates: serde_json::Value = match updates_resp {
            Ok(resp) => match resp.json().await {
                Ok(j) => j,
                Err(e) => {
                    tracing::warn!(error = %e, "failed to parse Telegram response");
                    tokio::time::sleep(std::time::Duration::from_secs(2)).await;
                    continue;
                }
            },
            Err(e) => {
                tracing::warn!(error = %e, "Telegram poll failed, retrying...");
                tokio::time::sleep(std::time::Duration::from_secs(5)).await;
                continue;
            }
        };

        let results = updates
            .get("result")
            .and_then(|v| v.as_array())
            .cloned()
            .unwrap_or_default();

        for update in &results {
            let update_id = update
                .get("update_id")
                .and_then(|v| v.as_i64())
                .unwrap_or(0);
            offset = update_id + 1;

            // Persist offset so we don't reprocess messages after a restart.
            let _ = bot_state.set_i64("telegram_offset", offset).await;

            // Handle inline keyboard callback queries (e.g. model selection).
            if let Some(cb) = update.get("callback_query") {
                let cb_id = cb.get("id").and_then(|v| v.as_str()).unwrap_or("");
                let cb_data = cb.get("data").and_then(|v| v.as_str()).unwrap_or("");
                let cb_chat_id = cb
                    .pointer("/message/chat/id")
                    .and_then(|v| v.as_i64())
                    .unwrap_or(0);

                // Answer the callback to dismiss the loading spinner.
                let _ = http
                    .post(format!("{telegram_api}/answerCallbackQuery"))
                    .json(&serde_json::json!({ "callback_query_id": cb_id }))
                    .send()
                    .await;

                if let Some(alias) = cb_data.strip_prefix("model:") {
                    if alias == "noop" {
                        continue;
                    }
                    // Synthesize a /model command.
                    let synthetic = format!("/model {alias}");
                    if let Some(switch_result) =
                        crate::model_switch::try_switch_model(&synthetic, &llm)
                    {
                        model = switch_result.model.clone();
                        chat_model_alias.insert(cb_chat_id, synthetic);
                        let reply = format!(
                            "Switched to **{}** (`{}`)",
                            switch_result.provider_name, switch_result.model
                        );
                        info!(
                            chat_id = cb_chat_id,
                            provider = %switch_result.provider_name,
                            model = %switch_result.model,
                            "user switched model via keyboard"
                        );
                        let _ = http
                            .post(format!("{telegram_api}/sendMessage"))
                            .json(&serde_json::json!({
                                "chat_id": cb_chat_id,
                                "text": reply,
                                "parse_mode": "Markdown",
                            }))
                            .send()
                            .await;
                    } else {
                        let _ = http
                            .post(format!("{telegram_api}/sendMessage"))
                            .json(&serde_json::json!({
                                "chat_id": cb_chat_id,
                                "text": format!("Failed to switch to `{alias}`. Check that the API key is configured."),
                                "parse_mode": "Markdown",
                            }))
                            .send()
                            .await;
                    }
                }
                continue;
            }

            let message = match update.get("message") {
                Some(m) => m,
                None => continue,
            };

            let raw_text = match message.get("text").and_then(|v| v.as_str()) {
                Some(t) => t,
                None => continue,
            };

            // Extract quoted/replied-to message context (if user is replying).
            let reply_context = message
                .get("reply_to_message")
                .and_then(|reply| {
                    let reply_text = reply.get("text").and_then(|v| v.as_str())?;
                    let reply_from = reply
                        .pointer("/from/first_name")
                        .and_then(|v| v.as_str())
                        .unwrap_or("someone");
                    let reply_is_bot = reply
                        .pointer("/from/is_bot")
                        .and_then(|v| v.as_bool())
                        .unwrap_or(false);
                    let who = if reply_is_bot { "you (the bot)" } else { reply_from };
                    Some(format!(
                        "[Replying to {who}'s message: \"{reply_text}\"]\n\n"
                    ))
                });

            // Combine reply context with the user's message.
            let text = if let Some(ref ctx) = reply_context {
                format!("{ctx}{raw_text}")
            } else {
                raw_text.to_string()
            };
            let text = text.as_str();

            let chat_id = message
                .pointer("/chat/id")
                .and_then(|v| v.as_i64())
                .unwrap_or(0);
            let user_id = message
                .pointer("/from/id")
                .and_then(|v| v.as_i64())
                .unwrap_or(0);
            let user_name = message
                .pointer("/from/first_name")
                .and_then(|v| v.as_str())
                .unwrap_or("unknown");

            // Extract user's language from Telegram (e.g., "en", "zh-hans", "ja").
            let user_lang = message
                .pointer("/from/language_code")
                .and_then(|v| v.as_str())
                .unwrap_or("en")
                .to_string();
            user_languages.insert(chat_id, user_lang.clone());

            info!(
                chat_id,
                user_id, user_name, lang = %user_lang, text, "incoming Telegram message"
            );

            // Access control.
            if let Some(ref ids) = allowed_user_ids
                && !ids.contains(&user_id)
            {
                tracing::warn!(user_id, "user not in allowed list, ignoring");
                let _ = http
                    .post(format!("{telegram_api}/sendMessage"))
                    .json(&serde_json::json!({
                        "chat_id": chat_id,
                        "text": "Sorry, you are not authorized to use this bot.",
                    }))
                    .send()
                    .await;
                continue;
            }

            // Handle /start command.
            if text == "/start" {
                send_start_message(&http, &telegram_api, chat_id, &msgs, &llm, &model, &user_lang).await;
                continue;
            }

            // Handle /clear command.
            if text == "/clear" {
                chat_histories.remove(&chat_id);
                let _ = http
                    .post(format!("{telegram_api}/sendMessage"))
                    .json(&serde_json::json!({
                        "chat_id": chat_id,
                        "text": "Conversation cleared. Send a new message to start fresh.",
                    }))
                    .send()
                    .await;
                continue;
            }

            // Handle /tokens on|off command.
            if text == "/tokens on" {
                chat_show_tokens.insert(chat_id, true);
                let reply = msgs.get(keys::BOT_TOKENS_ON);
                let _ = http
                    .post(format!("{telegram_api}/sendMessage"))
                    .json(&serde_json::json!({
                        "chat_id": chat_id,
                        "text": reply,
                    }))
                    .send()
                    .await;
                continue;
            }

            if text == "/tokens off" {
                chat_show_tokens.insert(chat_id, false);
                let reply = msgs.get(keys::BOT_TOKENS_OFF);
                let _ = http
                    .post(format!("{telegram_api}/sendMessage"))
                    .json(&serde_json::json!({
                        "chat_id": chat_id,
                        "text": reply,
                    }))
                    .send()
                    .await;
                continue;
            }

            // Handle /dev command.
            if text.starts_with("/dev ") {
                let instruction = text.trim_start_matches("/dev ").trim();
                let reply =
                    dev_commands::handle_dev_command(&dev_task_store, chat_id, instruction).await;
                let _ = http
                    .post(format!("{telegram_api}/sendMessage"))
                    .json(&serde_json::json!({
                        "chat_id": chat_id,
                        "text": reply,
                    }))
                    .send()
                    .await;
                continue;
            }

            // Handle /tasks command.
            if text == "/tasks" {
                let reply = dev_commands::handle_tasks_command(&dev_task_store, chat_id).await;
                let chunks = split_telegram_message(&reply, 4000);
                for chunk in &chunks {
                    let _ = http
                        .post(format!("{telegram_api}/sendMessage"))
                        .json(&serde_json::json!({
                            "chat_id": chat_id,
                            "text": chunk,
                        }))
                        .send()
                        .await;
                }
                continue;
            }

            // Handle /merge <task_id> command.
            if text.starts_with("/merge ") {
                let task_id = text.trim_start_matches("/merge ").trim();
                let reply =
                    dev_commands::handle_merge_command(&dev_task_store, task_id, chat_id).await;
                let _ = http
                    .post(format!("{telegram_api}/sendMessage"))
                    .json(&serde_json::json!({
                        "chat_id": chat_id,
                        "text": reply,
                    }))
                    .send()
                    .await;
                continue;
            }

            // Handle /cancel <task_id> command.
            if text.starts_with("/cancel ") {
                let task_id = text.trim_start_matches("/cancel ").trim();
                let reply =
                    dev_commands::handle_cancel_command(&dev_task_store, task_id, chat_id).await;
                let _ = http
                    .post(format!("{telegram_api}/sendMessage"))
                    .json(&serde_json::json!({
                        "chat_id": chat_id,
                        "text": reply,
                    }))
                    .send()
                    .await;
                continue;
            }

            // Handle /taskstatus <task_id> command.
            if text.starts_with("/taskstatus ") {
                let task_id = text.trim_start_matches("/taskstatus ").trim();
                let reply =
                    dev_commands::handle_task_status_command(&dev_task_store, task_id).await;
                let chunks = split_telegram_message(&reply, 4000);
                for chunk in &chunks {
                    let _ = http
                        .post(format!("{telegram_api}/sendMessage"))
                        .json(&serde_json::json!({
                            "chat_id": chat_id,
                            "text": chunk,
                        }))
                        .send()
                        .await;
                }
                continue;
            }

            // Handle /models command — show inline keyboard with available models.
            if text == "/models" {
                let keyboard = crate::model_switch::build_models_keyboard();
                let _ = http
                    .post(format!("{telegram_api}/sendMessage"))
                    .json(&serde_json::json!({
                        "chat_id": chat_id,
                        "text": "Select a model:",
                        "reply_markup": keyboard,
                    }))
                    .send()
                    .await;
                continue;
            }

            // Handle model switching (natural language or /model command).
            if let Some(switch_result) = crate::model_switch::try_switch_model(text, &llm) {
                model = switch_result.model.clone();
                // Store the alias so we can re-apply after failover resets.
                chat_model_alias.insert(chat_id, text.to_string());
                let reply = format!(
                    "Switched to **{}** (`{}`)",
                    switch_result.provider_name, switch_result.model
                );
                info!(
                    chat_id,
                    provider = %switch_result.provider_name,
                    model = %switch_result.model,
                    "user switched model"
                );
                let _ = http
                    .post(format!("{telegram_api}/sendMessage"))
                    .json(&serde_json::json!({
                        "chat_id": chat_id,
                        "text": reply,
                        "parse_mode": "Markdown",
                    }))
                    .send()
                    .await;
                continue;
            }

            // Re-apply per-chat model override. This ensures the global LLM
            // client points to the right provider even if a previous failover
            // cascade reset it to defaults.
            if let Some(alias_cmd) = chat_model_alias.get(&chat_id) {
                if let Some(sw) = crate::model_switch::try_switch_model(alias_cmd, &llm) {
                    model = sw.model.clone();
                }
            } else {
                // No per-chat override: ensure we're on the primary provider.
                llm.restore_defaults();
                model = primary_model.clone();
            }

            // Check for mid-task message injection (non-blocking).
            dev_commands::try_inject_mid_task_message(&dev_task_store, chat_id, text).await;

            // Send "typing" indicator.
            let _ = http
                .post(format!("{telegram_api}/sendChatAction"))
                .json(&serde_json::json!({
                    "chat_id": chat_id,
                    "action": "typing",
                }))
                .send()
                .await;

            // Persist user message.
            let session_key = format!("telegram-{chat_id}");
            {
                let all = sessions.list(10000, 0).await.unwrap_or_default();
                if !all.iter().any(|s| s.name == session_key) {
                    let _ = sessions.create(&session_key, &model).await;
                }
            }
            let _ = sessions
                .append_message(&session_key, "user", text, None, None)
                .await;

            // Build agent context with chat history.
            let mut system_prompt = load_system_prompt();

            // Tell the agent to match the user's language.
            let lang_name = messages::lang_display_name(&user_lang);
            system_prompt.push_str(&format!(
                "\n\n## Channel Context\n\n\
                 You are communicating via Telegram with **{user_name}** (user_id: {user_id}, chat_id: {chat_id}).\n\
                 The user's language is **{lang_name}** (code: {user_lang}). ALWAYS respond in {lang_name}.\n\n\
                 Telegram formatting rules:\n\
                 - Use RICH formatting: bold (**text**), tables, bullet points, numbered lists, headings.\n\
                 - Structure complex responses with clear sections, categories, and tables.\n\
                 - Tables are great for comparisons and lists of items.\n\
                 - Use emoji to mark categories.\n\
                 - For research results, present them in well-organized tables with columns.\n\
                 - You can use Telegram tools to send photos, documents, or additional messages.\n\
                 - Do NOT simplify or shorten your response just because it's Telegram. Give full, rich answers.\n\n\
                 **IMPORTANT: Multi-task handling:**\n\
                 When the user sends multiple tasks or requirements in a single message:\n\
                 1. Process each task/requirement one by one.\n\
                 2. After completing EACH task, immediately send the result to the user using \
                 `telegram_send_message` with chat_id=\"{chat_id}\".\n\
                 3. Do NOT wait until all tasks are done to reply. Send incremental results.\n\
                 4. Label each result clearly (e.g. \"✅ 需求 1 — Clip 完成\" or \"✅ Task 1 — Lead Done\").\n",
            ));

            if !skill_prompt_ext.is_empty() {
                system_prompt.push_str(&skill_prompt_ext);
            }

            // Simple query routing: short, non-tool messages use a cheaper model.
            let effective_model =
                select_model_for_query(raw_text, &model, simple_query_threshold, chat_id);
            let agent_config = AgentConfig {
                max_turns: 100,
                model: effective_model,
                temperature: Some(0.5),
                max_tokens: Some(8192),
                ..AgentConfig::default()
            };
            let mut ctx = AgentContext::new(llm.clone(), adapters.clone(), agent_config)
                .with_system_prompt(&system_prompt);

            // Restore conversation history.
            if !chat_histories.contains_key(&chat_id) {
                if let Ok(db_msgs) = sessions.get_messages(&session_key, Some(history_window)).await {
                    let restored: Vec<Message> = db_msgs
                        .into_iter()
                        .filter(|m| m.role == "user" || m.role == "assistant")
                        .map(|m| match m.role.as_str() {
                            "user" => Message::user(&m.content),
                            _ => Message::assistant(&m.content),
                        })
                        .collect();
                    if !restored.is_empty() {
                        info!(
                            chat_id,
                            count = restored.len(),
                            "restored conversation history from database"
                        );
                    }
                    chat_histories.insert(chat_id, restored);
                }
            }

            let history = chat_histories.entry(chat_id).or_default();
            for msg in history.iter() {
                ctx.messages.push(msg.clone());
            }
            ctx = ctx.with_user_message(text);

            // Tool-start callback: send status messages to Telegram.
            let status_http = http.clone();
            let status_api = telegram_api.clone();
            let sent_statuses: Arc<std::sync::Mutex<std::collections::HashSet<String>>> =
                Arc::new(std::sync::Mutex::new(std::collections::HashSet::new()));

            // Pre-translate tool status messages for this user's language.
            let status_msgs = msgs
                .batch_translate(
                    &[
                        keys::STATUS_RESEARCHING,
                        keys::STATUS_SEARCHING,
                        keys::STATUS_READING_PAGE,
                        keys::STATUS_READING_FILES,
                        keys::STATUS_RUNNING_COMMAND,
                        keys::STATUS_ACCESSING_MEMORY,
                        keys::STATUS_GITHUB,
                    ],
                    &user_lang,
                    &llm,
                    &model,
                )
                .await;
            let status_map: Arc<HashMap<String, String>> = Arc::new(status_msgs);

            ctx.on_tool_start = Some(Arc::new(move |tool_name: &str, _args: &serde_json::Value| {
                let key = match tool_name {
                    "web_research" => Some(keys::STATUS_RESEARCHING),
                    "web_search" => Some(keys::STATUS_SEARCHING),
                    "web_fetch" => Some(keys::STATUS_READING_PAGE),
                    "fs_read_file" | "fs_list_directory" => Some(keys::STATUS_READING_FILES),
                    "shell_execute" => Some(keys::STATUS_RUNNING_COMMAND),
                    "memory_search" | "memory_save" => Some(keys::STATUS_ACCESSING_MEMORY),
                    "github_create_issue" | "github_list_repos" => Some(keys::STATUS_GITHUB),
                    _ => None,
                };
                if let Some(msg_key) = key {
                    let msg = status_map
                        .get(msg_key)
                        .cloned()
                        .unwrap_or_else(|| msg_key.to_string());
                    let already_sent = {
                        let mut set = sent_statuses.lock().unwrap_or_else(|e| e.into_inner());
                        !set.insert(msg_key.to_string())
                    };
                    if already_sent {
                        return;
                    }
                    let client = status_http.clone();
                    let api = status_api.clone();
                    tokio::spawn(async move {
                        let _ = client
                            .post(format!("{api}/sendMessage"))
                            .json(&serde_json::json!({
                                "chat_id": chat_id,
                                "text": msg,
                            }))
                            .send()
                            .await;
                    });
                }
            }));

            // Spawn a background task to send periodic "typing" indicators.
            let typing_http = http.clone();
            let typing_api = telegram_api.clone();
            let typing_cancel = Arc::new(tokio::sync::Notify::new());
            let typing_cancel_clone = typing_cancel.clone();
            let typing_handle = tokio::spawn(async move {
                loop {
                    tokio::select! {
                        _ = typing_cancel_clone.notified() => break,
                        _ = tokio::time::sleep(std::time::Duration::from_secs(4)) => {
                            let _ = typing_http
                                .post(format!("{typing_api}/sendChatAction"))
                                .json(&serde_json::json!({
                                    "chat_id": chat_id,
                                    "action": "typing",
                                }))
                                .send()
                                .await;
                        }
                    }
                }
            });

            // Run the ReAct loop.
            let (reply_text, token_stats) = match react_loop(&mut ctx).await {
                Ok(response) => {
                    let tokens = (response.input_tokens, response.output_tokens);
                    info!(
                        chat_id,
                        turns = response.turns_used,
                        input_tokens = response.input_tokens,
                        output_tokens = response.output_tokens,
                        "agent completed"
                    );
                    // Always log token usage.
                    if response.input_tokens + response.output_tokens > 0 {
                        tracing::info!(
                            chat_id,
                            input_tokens = response.input_tokens,
                            output_tokens = response.output_tokens,
                            "token usage"
                        );
                    }

                    let text_out = if let Some(ref evo) = evolution {
                        let mut evo = evo.lock().await;
                        if let Some(issue_url) = evo
                            .analyze_response(text, &response.text, "telegram", response.turns_used)
                            .await
                        {
                            format!(
                                "{}\n\n---\nI noticed I couldn't fully handle this. \
                                 A feature request has been auto-filed: {}",
                                response.text, issue_url
                            )
                        } else {
                            response.text
                        }
                    } else {
                        response.text
                    };

                    (text_out, Some(tokens))
                }
                Err(e) => {
                    let err_str = e.to_string();

                    // -------------------------------------------------------
                    // Handle provider-level errors (429 rate limit, 404 model
                    // not found, 502/503, etc.).  Cascade through all fallback
                    // providers until one succeeds or all are exhausted.
                    // -------------------------------------------------------
                    let err_text = if failover::is_provider_error(&err_str) {
                        tracing::warn!(error = %err_str, "provider error, attempting cascading failover");

                        // For auth errors, try Keychain refresh first.
                        if err_str.contains("401") || err_str.contains("authentication_error") {
                            if let Some(new_token) = crate::helpers::read_claude_code_keychain_token() {
                                llm.update_api_key(new_token);
                                tracing::info!("refreshed OAuth token from Keychain");
                            }
                        }

                        failover_mgr.mark_rate_limited(&model);

                        let cr = crate::bot_helpers::handle_cascade_failover(
                            &ctx, &llm, &adapters, &mut failover_mgr,
                            &model, chat_id, &http, &telegram_api,
                        )
                        .await;
                        model = cr.final_model;

                        if let Some(text) = cr.reply {
                            text
                        } else {
                            // All providers exhausted — restore primary so
                            // subsequent messages don't stay stuck.
                            llm.restore_defaults();
                            model = primary_model.clone();
                            chat_model_alias.remove(&chat_id);
                            msgs.get_translated(
                                keys::ERROR_GENERAL,
                                &[],
                                &user_lang,
                                &llm,
                                &model,
                            )
                            .await
                        }

                    // -------------------------------------------------------
                    // All other errors: attempt self-repair for code bugs.
                    // -------------------------------------------------------
                    } else {
                        tracing::error!(error = %e, "agent error");

                        let notifier = crate::self_repair::TelegramNotifier::new(
                            http.clone(),
                            telegram_api.clone(),
                            chat_id,
                            user_lang.clone(),
                            msgs.clone(),
                            llm.clone(),
                            model.clone(),
                        );
                        let repair_outcome = crate::self_repair::attempt_repair(
                            &e,
                            text,
                            &notifier,
                            &llm,
                            &adapters,
                            &model,
                            &repo_path,
                        )
                        .await;

                        match repair_outcome {
                            crate::self_repair::RepairOutcome::Fixed {
                                commit_hash,
                                summary,
                            } => {
                                let _ = commit_hash;
                                let msg = msgs
                                    .get_translated(
                                        keys::REPAIR_SUCCESS,
                                        &[("summary", &summary)],
                                        &user_lang,
                                        &llm,
                                        &model,
                                    )
                                    .await;
                                notifier.send_raw(&msg).await;

                                // Persist history before restart.
                                let _ = sessions
                                    .append_message(&session_key, "assistant", &msg, None, None)
                                    .await;

                                // Restart the process with the new binary.
                                crate::self_repair::restart_process();
                            }
                            crate::self_repair::RepairOutcome::NotACodeBug => {
                                tracing::debug!(error = %e, "not a code bug, skipping self-repair");
                                if let Some(ref evo) = evolution {
                                    let mut evo = evo.lock().await;
                                    let _ = evo.report_error(text, "telegram", &e).await;
                                }
                                msgs.get_translated(
                                    keys::ERROR_GENERAL,
                                    &[],
                                    &user_lang,
                                    &llm,
                                    &model,
                                )
                                .await
                            }
                            crate::self_repair::RepairOutcome::Failed { reason } => {
                                tracing::warn!(reason = %reason, "self-repair failed");
                                if let Some(ref evo) = evolution {
                                    let mut evo = evo.lock().await;
                                    let _ = evo.report_error(text, "telegram", &e).await;
                                }
                                msgs.get_translated(
                                    keys::ERROR_REPAIR_FAILED,
                                    &[],
                                    &user_lang,
                                    &llm,
                                    &model,
                                )
                                .await
                            }
                        }
                    };
                    (err_text, None)
                }
            };

            // Stop the periodic typing indicator.
            typing_cancel.notify_one();
            let _ = typing_handle.await;

            // Update chat history (keep last 50 exchanges = 100 messages).
            history.push(Message::user(text));
            history.push(Message::assistant(&reply_text));
            if history.len() > 100 {
                let drain_count = history.len() - 100;
                history.drain(..drain_count);
            }

            // Persist assistant response.
            let _ = sessions
                .append_message(&session_key, "assistant", &reply_text, None, None)
                .await;

            // Send the response back to Telegram.
            let chunks = split_telegram_message(&reply_text, 4000);
            for chunk in &chunks {
                let send_result = http
                    .post(format!("{telegram_api}/sendMessage"))
                    .json(&serde_json::json!({
                        "chat_id": chat_id,
                        "text": chunk,
                    }))
                    .send()
                    .await;

                if let Err(e) = send_result {
                    tracing::error!(error = %e, "failed to send Telegram reply");
                }
            }

            // Send token usage stats if enabled for this chat.
            let show_tokens = chat_show_tokens
                .get(&chat_id)
                .copied()
                .unwrap_or(global_show_token_usage);

            if show_tokens {
                if let Some((input, output)) = token_stats {
                    send_token_stats(&http, &telegram_api, chat_id, input, output, &msgs).await;
                }
            }

            // Restart if the self-update tool replaced the binary this cycle.
            check_restart_signal(&restart_signal, &bot_state, chat_id).await;
        }
    }
}
