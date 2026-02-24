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
use crate::dev_commands;
use crate::dev_worker::{DevWorker, ProgressCallback};
use crate::helpers::{env_non_empty, init_tracing, load_system_prompt, resolve_llm_config};

/// Run the Telegram bot gateway.
pub async fn cmd_bot(poll_timeout: u64, allowed_users: Option<String>) -> Result<()> {
    init_tracing("info");
    info!("starting Telegram bot gateway");

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
                let short_id = &task.id[..8.min(task.id.len())];
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
                    let short_id = &task.id[..8.min(task.id.len())];
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

    // Per-chat conversation history (in-memory, keyed by chat_id).
    let mut chat_histories: HashMap<i64, Vec<Message>> = HashMap::new();

    // Polling loop — restore offset from persistent state.
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

            let text = match message.get("text").and_then(|v| v.as_str()) {
                Some(t) => t,
                None => continue,
            };

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

            info!(
                chat_id,
                user_id, user_name, text, "incoming Telegram message"
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
            system_prompt.push_str(&format!(
                "\n\nYou are communicating via Telegram with user {} (id: {}). \
                 Keep responses concise and suitable for chat. \
                 You can use Telegram tools to send photos or additional messages if needed.",
                user_name, user_id
            ));
            if !skill_prompt_ext.is_empty() {
                system_prompt.push_str(&skill_prompt_ext);
            }

            let agent_config = AgentConfig {
                max_turns: 20,
                model: model.clone(),
                temperature: Some(0.0),
                max_tokens: Some(4096),
                ..AgentConfig::default()
            };

            let mut ctx = AgentContext::new(llm.clone(), adapters.clone(), agent_config)
                .with_system_prompt(&system_prompt);

            let history = chat_histories.entry(chat_id).or_default();
            for msg in history.iter() {
                ctx.messages.push(msg.clone());
            }
            ctx = ctx.with_user_message(text);

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

                    if let Some(ref evo) = evolution {
                        let mut evo = evo.lock().await;
                        if let Some(issue_url) = evo.report_error(text, "telegram", &e).await {
                            format!(
                                "Error: {e}\n\nA feature request has been auto-filed: {issue_url}"
                            )
                        } else {
                            format!("Error: {e}")
                        }
                    } else {
                        format!("Error: {e}")
                    }
                }
            };

            // Update chat history (keep last 20 exchanges).
            history.push(Message::user(text));
            history.push(Message::assistant(&reply_text));
            if history.len() > 40 {
                let drain_count = history.len() - 40;
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

        let split_at = remaining[..max_len]
            .rfind('\n')
            .unwrap_or_else(|| remaining[..max_len].rfind(' ').unwrap_or(max_len));

        chunks.push(remaining[..split_at].to_owned());
        remaining = remaining[split_at..].trim_start();
    }

    chunks
}
