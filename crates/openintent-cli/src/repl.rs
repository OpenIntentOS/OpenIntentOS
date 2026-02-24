//! Subcommand: `openintent run` — interactive REPL.
//!
//! Runs the full ReAct (Reason + Act) loop in a terminal REPL with session
//! persistence, streaming output, and self-evolution support.

use std::io::{self, Write as _};
use std::path::Path;
use std::sync::Arc;

use anyhow::{Context, Result};
use tracing::info;

use openintent_agent::{
    AgentConfig, AgentContext, EvolutionEngine, LlmClient, Message, react_loop,
};
use openintent_store::SessionStore;

use crate::adapters::init_adapters;
use crate::helpers::{init_tracing, load_system_prompt, resolve_llm_config};

/// Run the interactive REPL.
pub async fn cmd_run(session_name: Option<String>) -> Result<()> {
    // 1. Initialize tracing.
    init_tracing("info");

    info!("starting OpenIntentOS");

    // 2. Create data directory and initialize SQLite.
    let data_dir = Path::new("data");
    if !data_dir.exists() {
        std::fs::create_dir_all(data_dir).context("failed to create data directory")?;
    }

    let db_path = data_dir.join("openintent.db");
    let db = openintent_store::Database::open_and_migrate(db_path.clone())
        .await
        .context("failed to open database")?;
    info!(path = %db_path.display(), "store initialized");

    // 3. Resolve LLM provider, API key, and model.
    let llm_config = resolve_llm_config();
    let provider_label = format!("{:?}", llm_config.provider);
    let model = llm_config.default_model.clone();
    let llm = Arc::new(LlmClient::new(llm_config).context("failed to create LLM client")?);
    info!(model = %model, provider = %provider_label, "LLM client ready");

    // 4. Set up session persistence.
    let sessions = SessionStore::new(db.clone());

    let active_session = if let Some(ref name) = session_name {
        let all = sessions
            .list(1000, 0)
            .await
            .context("failed to list sessions")?;
        let existing = all.into_iter().find(|s| s.name == *name);

        match existing {
            Some(session) => {
                let msg_count = session.message_count;
                println!(
                    "  Resuming session: {} ({} messages)",
                    session.name, msg_count
                );
                Some(session)
            }
            None => {
                let session = sessions
                    .create(name, &model)
                    .await
                    .context("failed to create session")?;
                println!("  Created new session: {}", session.name);
                Some(session)
            }
        }
    } else {
        None
    };

    let session_id = active_session.as_ref().map(|s| s.id.clone());

    // 5. Initialize adapters.
    let cwd = std::env::current_dir().context("failed to get current directory")?;
    let initialized = init_adapters(cwd, db, true).await?;
    let adapters = initialized.tool_adapters;
    let skill_prompt_ext = initialized.skill_prompt_ext;
    let skill_count = initialized.skill_count;

    info!(
        "adapters initialized (filesystem, shell, web_search, web_fetch, http_request, cron, memory, github, email, browser, feishu, calendar, telegram, discord)"
    );

    // 6. Load session history if resuming.
    let mut history_messages: Vec<Message> = Vec::new();
    if let Some(ref sid) = session_id {
        let stored = sessions
            .get_messages(sid, Some(20))
            .await
            .context("failed to load session messages")?;
        for msg in &stored {
            let message = match msg.role.as_str() {
                "user" => Message::user(&msg.content),
                "assistant" => Message::assistant(&msg.content),
                "system" => Message::system(&msg.content),
                _ => continue,
            };
            history_messages.push(message);
        }
        if !history_messages.is_empty() {
            info!(
                count = history_messages.len(),
                "loaded session history messages"
            );
        }
    }

    // 7. Initialize evolution engine.
    let evolution = EvolutionEngine::from_env();
    let evolution_status = if evolution.is_some() {
        "enabled"
    } else {
        "disabled (set GITHUB_TOKEN to enable)"
    };

    // 8. Print startup banner.
    println!();
    println!("  OpenIntentOS v{}", env!("CARGO_PKG_VERSION"));
    println!("  Provider: {provider_label}");
    println!("  Model: {model}");
    println!("  Evolution: {evolution_status}");
    println!("  Adapters: filesystem, shell, web_search, web_fetch, http_request, cron, memory,");
    println!("            github, email, browser, feishu, calendar, telegram, discord");
    if skill_count > 0 {
        println!("  Skills: {skill_count}");
    }
    if let Some(ref name) = session_name {
        println!("  Session: {name}");
    }
    println!("  Type your request, or 'quit' to exit.");
    println!();

    // 9. Set up Ctrl+C handler.
    let running = Arc::new(std::sync::atomic::AtomicBool::new(true));
    {
        let running = running.clone();
        tokio::spawn(async move {
            if tokio::signal::ctrl_c().await.is_ok() {
                running.store(false, std::sync::atomic::Ordering::SeqCst);
                eprintln!("\n  Interrupted. Goodbye!");
                std::process::exit(0);
            }
        });
    }

    // 10. REPL loop.
    let stdin = io::stdin();
    let mut line_buf = String::new();

    loop {
        print!("> ");
        io::stdout().flush().ok();

        line_buf.clear();
        let bytes_read = stdin.read_line(&mut line_buf);
        match bytes_read {
            Ok(0) => {
                println!();
                info!("EOF received, exiting");
                break;
            }
            Ok(_) => {}
            Err(e) => {
                eprintln!("  Error reading input: {e}");
                continue;
            }
        }

        let trimmed = line_buf.trim();

        if trimmed.is_empty() {
            continue;
        }

        if trimmed == "quit" || trimmed == "exit" {
            info!("user requested exit");
            break;
        }

        // Persist user message to session.
        if let Some(ref sid) = session_id
            && let Err(e) = sessions
                .append_message(sid, "user", trimmed, None, None)
                .await
        {
            tracing::warn!(error = %e, "failed to persist user message");
        }

        // Build agent context for this request.
        let agent_config = AgentConfig {
            max_turns: 20,
            model: model.clone(),
            temperature: Some(0.0),
            max_tokens: Some(4096),
            ..AgentConfig::default()
        };

        let mut system_prompt = load_system_prompt();
        if !skill_prompt_ext.is_empty() {
            system_prompt.push_str(&skill_prompt_ext);
        }
        let mut ctx = AgentContext::new(llm.clone(), adapters.clone(), agent_config)
            .with_system_prompt(&system_prompt);

        // Enable real-time streaming.
        let streaming_started = Arc::new(std::sync::atomic::AtomicBool::new(false));
        let streaming_flag = streaming_started.clone();
        ctx.on_text_delta = Some(Arc::new(std::sync::Mutex::new(move |delta: &str| {
            if !streaming_flag.load(std::sync::atomic::Ordering::Relaxed) {
                streaming_flag.store(true, std::sync::atomic::Ordering::Relaxed);
            }
            print!("{delta}");
            io::stdout().flush().ok();
        })));

        // Inject session history.
        for msg in &history_messages {
            ctx.messages.push(msg.clone());
        }
        ctx = ctx.with_user_message(trimmed);

        // Run the ReAct loop.
        match react_loop(&mut ctx).await {
            Ok(response) => {
                if streaming_started.load(std::sync::atomic::Ordering::Relaxed) {
                    println!();
                } else {
                    println!("{}", response.text);
                }

                if response.turns_used > 1 {
                    println!(
                        "  ({} tool turn{} used)",
                        response.turns_used - 1,
                        if response.turns_used - 1 == 1 {
                            ""
                        } else {
                            "s"
                        }
                    );
                }

                // Evolution: analyze response for signs of inability.
                if let Some(ref evo) = evolution {
                    let mut evo = evo.lock().await;
                    if let Some(issue_url) = evo
                        .analyze_response(trimmed, &response.text, "cli", response.turns_used)
                        .await
                    {
                        println!("  [Evolution] Feature request auto-filed: {issue_url}");
                    }
                }
                println!();

                // Persist assistant message to session.
                if let Some(ref sid) = session_id
                    && let Err(e) = sessions
                        .append_message(sid, "assistant", &response.text, None, None)
                        .await
                {
                    tracing::warn!(error = %e, "failed to persist assistant message");
                }

                // Update rolling history.
                history_messages.push(Message::user(trimmed));
                history_messages.push(Message::assistant(&response.text));
            }
            Err(e) => {
                eprintln!("\n  Error: {e}");

                // Evolution: report errors as unhandled intents.
                if let Some(ref evo) = evolution {
                    let mut evo = evo.lock().await;
                    if let Some(issue_url) = evo.report_error(trimmed, "cli", &e).await {
                        eprintln!("  [Evolution] Feature request auto-filed: {issue_url}");
                    }
                }
                eprintln!();
            }
        }

        if !running.load(std::sync::atomic::Ordering::SeqCst) {
            break;
        }
    }

    info!("shutting down");
    Ok(())
}
