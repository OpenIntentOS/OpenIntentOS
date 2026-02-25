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
    AgentConfig, AgentContext, ChatRequest, EvolutionEngine, LlmClient, LlmResponse, Message,
    react_loop,
};
use openintent_store::{BotStateStore, DevTaskStore, SessionStore};

use crate::adapters::init_adapters;
use crate::dev_commands;
use crate::dev_worker::{DevWorker, ProgressCallback};
use crate::helpers::{env_non_empty, init_tracing, load_system_prompt, resolve_llm_config};
use crate::messages::{self, Messages, keys, safe_prefix};

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
    let model = llm_config.default_model.clone();
    let llm = Arc::new(LlmClient::new(llm_config).context("failed to create LLM client")?);

    let sessions = SessionStore::new(db.clone());

    // Initialize the dev task store and bot state store.
    let dev_task_store = DevTaskStore::new(db.clone());
    let bot_state = BotStateStore::new(db.clone());

    // Initialize adapters.
    let cwd = std::env::current_dir().context("failed to get current directory")?;
    let initialized = init_adapters(cwd.clone(), db, true).await?;
    let adapters = initialized.tool_adapters;
    let skill_prompt_ext = initialized.skill_prompt_ext;

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
        // Also check pending tasks that haven't started yet.
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

    // Send startup notification with latest changes to all recent active chats.
    send_startup_notification(&http, &telegram_api, &sessions, &llm, &model, &msgs).await;

    // Per-chat conversation history (in-memory, keyed by chat_id).
    let mut chat_histories: HashMap<i64, Vec<Message>> = HashMap::new();

    // Per-chat user language (from Telegram's language_code).
    let mut user_languages: HashMap<i64, String> = HashMap::new();

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
                "allowed_updates": ["message"],
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
                let _ = http
                    .post(format!("{telegram_api}/sendMessage"))
                    .json(&serde_json::json!({
                        "chat_id": chat_id,
                        "text": "Hello! I'm OpenIntentOS. Send me any message and I'll help you. I have access to filesystem, shell, web search, email, GitHub, and more.\n\nDev commands:\n/dev <instruction> - Create a self-development task\n/tasks - List your dev tasks\n/taskstatus <id> - Check task status\n/merge <id> - Merge a completed task\n/cancel <id> - Cancel a task",
                    }))
                    .send()
                    .await;
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
                 - Do NOT simplify or shorten your response just because it's Telegram. Give full, rich answers.\n",
            ));

            if !skill_prompt_ext.is_empty() {
                system_prompt.push_str(&skill_prompt_ext);
            }

            let agent_config = AgentConfig {
                max_turns: 100,
                model: model.clone(),
                temperature: Some(0.5),
                max_tokens: Some(8192),
                ..AgentConfig::default()
            };

            let mut ctx = AgentContext::new(llm.clone(), adapters.clone(), agent_config)
                .with_system_prompt(&system_prompt);

            // Restore conversation history.
            if !chat_histories.contains_key(&chat_id) {
                if let Ok(db_msgs) = sessions.get_messages(&session_key, Some(100)).await {
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
            let reply_text = match react_loop(&mut ctx).await {
                Ok(response) => {
                    info!(chat_id, turns = response.turns_used, "agent completed");

                    if let Some(ref evo) = evolution {
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
                    }
                }
                Err(e) => {
                    tracing::error!(error = %e, "agent error");

                    // Attempt self-repair for code bugs.
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
        }
    }
}

/// Split a message into chunks that fit within Telegram's character limit.
///
/// Respects UTF-8 char boundaries to avoid panics on multi-byte characters.
fn split_telegram_message(text: &str, max_len: usize) -> Vec<String> {
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
async fn send_startup_notification(
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
