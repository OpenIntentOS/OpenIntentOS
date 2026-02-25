//! CLI entry point for OpenIntentOS.
//!
//! Provides the `openintent` command with subcommands for running the AI agent
//! REPL, launching a web server, running setup, and checking system status.
//!
//! Heavy subcommands are split into their own modules:
//! - [`repl`] — `openintent run` interactive REPL
//! - [`bot`] — `openintent bot` Telegram gateway

mod adapters;
mod bot;
mod bridge;
mod cli;
mod dev_commands;
mod dev_worker;
mod helpers;
mod intent_classifier;
mod messages;
mod repl;
mod self_repair;

use std::path::Path;
use std::sync::Arc;

use anyhow::{Context, Result};
use chrono::{TimeZone, Utc};
use clap::Parser;
use tracing::info;

use openintent_agent::{AgentConfig, LlmClient};
use openintent_store::SessionStore;

use crate::adapters::init_adapters;
use crate::cli::{Cli, Commands, SessionAction, SkillAction, UserAction};
use crate::helpers::{
    env_non_empty, init_tracing, load_system_prompt, read_claude_code_keychain_token,
    resolve_llm_config,
};

// ---------------------------------------------------------------------------
// Main
// ---------------------------------------------------------------------------

#[tokio::main]
async fn main() -> Result<()> {
    // Load .env file if present (silently ignore if missing).
    dotenvy::dotenv().ok();

    let cli = Cli::parse();

    match cli.command {
        Commands::Run { session } => repl::cmd_run(session).await,
        Commands::Serve { bind, port } => cmd_serve(bind, port).await,
        Commands::Setup => cmd_setup().await,
        Commands::Status => cmd_status().await,
        Commands::Sessions { action } => cmd_sessions(action).await,
        Commands::Tui { session } => cmd_tui(session).await,
        Commands::Gui => cmd_gui().await,
        Commands::Users { action } => cmd_users(action).await,
        Commands::Skills { action } => cmd_skills(action).await,
        Commands::Bot {
            poll_timeout,
            allowed_users,
        } => bot::cmd_bot(poll_timeout, allowed_users).await,
    }
}

// ---------------------------------------------------------------------------
// Subcommand: tui
// ---------------------------------------------------------------------------

async fn cmd_tui(_session_name: Option<String>) -> Result<()> {
    init_tracing("info");

    info!("starting OpenIntentOS TUI");

    let data_dir = Path::new("data");
    if !data_dir.exists() {
        std::fs::create_dir_all(data_dir).context("failed to create data directory")?;
    }

    let db_path = data_dir.join("openintent.db");
    let db = openintent_store::Database::open_and_migrate(db_path.clone())
        .await
        .context("failed to open database")?;
    info!(path = %db_path.display(), "store initialized");

    let llm_config = resolve_llm_config();
    let model = llm_config.default_model.clone();
    let llm = Arc::new(LlmClient::new(llm_config).context("failed to create LLM client")?);
    info!(model = %model, "LLM client ready");

    let cwd = std::env::current_dir().context("failed to get current directory")?;
    let initialized = init_adapters(cwd, db, false).await?;
    let adapters = initialized.tool_adapters;

    let system_prompt = load_system_prompt();

    let config = AgentConfig {
        max_turns: 20,
        model,
        temperature: Some(0.0),
        max_tokens: Some(4096),
        ..AgentConfig::default()
    };

    openintent_tui::run_tui(llm, adapters, config, system_prompt)
        .await
        .map_err(|e| anyhow::anyhow!("{e}"))?;

    Ok(())
}

// ---------------------------------------------------------------------------
// Subcommand: sessions
// ---------------------------------------------------------------------------

async fn cmd_sessions(action: SessionAction) -> Result<()> {
    init_tracing("warn");

    let data_dir = Path::new("data");
    let db_path = data_dir.join("openintent.db");

    if !db_path.exists() {
        eprintln!("  Error: Database not found. Run `openintent setup` first.");
        std::process::exit(1);
    }

    let db = openintent_store::Database::open_and_migrate(db_path)
        .await
        .context("failed to open database")?;
    let sessions = SessionStore::new(db);

    match action {
        SessionAction::List => {
            let all = sessions
                .list(100, 0)
                .await
                .context("failed to list sessions")?;

            if all.is_empty() {
                println!("  No sessions found.");
                return Ok(());
            }

            println!();
            println!("  {:<30} {:>8} {:>20}", "NAME", "MESSAGES", "LAST UPDATED");
            println!("  {}", "-".repeat(62));

            for s in &all {
                let updated = Utc
                    .timestamp_opt(s.updated_at, 0)
                    .single()
                    .map(|dt| dt.format("%Y-%m-%d %H:%M:%S").to_string())
                    .unwrap_or_else(|| "unknown".to_owned());

                println!("  {:<30} {:>8} {:>20}", s.name, s.message_count, updated);
            }
            println!();
        }

        SessionAction::Show { name } => {
            let all = sessions
                .list(1000, 0)
                .await
                .context("failed to list sessions")?;
            let session = all.into_iter().find(|s| s.name == name);

            let session = match session {
                Some(s) => s,
                None => {
                    eprintln!("  Error: Session '{}' not found.", name);
                    std::process::exit(1);
                }
            };

            let messages = sessions
                .get_messages(&session.id, None)
                .await
                .context("failed to load session messages")?;

            if messages.is_empty() {
                println!("  Session '{}' has no messages.", name);
                return Ok(());
            }

            println!();
            println!("  Session: {} ({} messages)", session.name, messages.len());
            println!("  {}", "-".repeat(50));

            for msg in &messages {
                let ts = Utc
                    .timestamp_opt(msg.created_at, 0)
                    .single()
                    .map(|dt| dt.format("%H:%M:%S").to_string())
                    .unwrap_or_else(|| "??:??:??".to_owned());

                let role_label = match msg.role.as_str() {
                    "user" => "You",
                    "assistant" => "Assistant",
                    "system" => "System",
                    other => other,
                };

                println!("  [{}] {}:", ts, role_label);

                for line in msg.content.lines() {
                    println!("    {line}");
                }
                println!();
            }
        }

        SessionAction::Delete { name } => {
            let all = sessions
                .list(1000, 0)
                .await
                .context("failed to list sessions")?;
            let session = all.into_iter().find(|s| s.name == name);

            let session = match session {
                Some(s) => s,
                None => {
                    eprintln!("  Error: Session '{}' not found.", name);
                    std::process::exit(1);
                }
            };

            sessions
                .delete(&session.id)
                .await
                .context("failed to delete session")?;

            println!("  Deleted session: {}", name);
        }
    }

    Ok(())
}

// ---------------------------------------------------------------------------
// Subcommand: serve
// ---------------------------------------------------------------------------

async fn cmd_serve(bind: String, port: u16) -> Result<()> {
    init_tracing("info");

    info!("starting OpenIntentOS web server");

    let data_dir = Path::new("data");
    if !data_dir.exists() {
        std::fs::create_dir_all(data_dir).context("failed to create data directory")?;
    }

    let db_path = data_dir.join("openintent.db");
    let db = openintent_store::Database::open_and_migrate(db_path.clone())
        .await
        .context("failed to open database")?;
    info!(path = %db_path.display(), "store initialized");

    let llm_config = resolve_llm_config();
    let provider_label = format!("{:?}", llm_config.provider);
    let model = llm_config.default_model.clone();
    let llm = Arc::new(LlmClient::new(llm_config).context("failed to create LLM client")?);
    info!(model = %model, provider = %provider_label, "LLM client ready");

    let cwd = std::env::current_dir().context("failed to get current directory")?;
    let initialized = init_adapters(cwd, db.clone(), false).await?;
    let raw_adapters = initialized.raw_adapters;

    info!(
        "adapters initialized (filesystem, shell, web_search, web_fetch, http_request, cron, memory, github, email, browser, feishu, calendar)"
    );

    let web_config = openintent_web::WebConfig {
        bind_addr: bind,
        port,
    };

    println!();
    println!("  OpenIntentOS v{}", env!("CARGO_PKG_VERSION"));
    println!("  Provider: {provider_label}");
    println!("  Model: {model}");
    println!(
        "  Web UI:  http://{}:{}",
        web_config.bind_addr, web_config.port
    );
    println!(
        "  MCP:     http://{}:{}/mcp",
        web_config.bind_addr, web_config.port
    );
    println!("  Adapters: filesystem, shell, web_search, web_fetch, http_request, cron, memory,");
    println!("            github, email, browser, feishu, calendar");
    println!();

    let server = openintent_web::WebServer::new(web_config, llm, raw_adapters, db);
    server.start().await.map_err(|e| anyhow::anyhow!("{e}"))?;

    Ok(())
}

// ---------------------------------------------------------------------------
// Subcommand: setup
// ---------------------------------------------------------------------------

async fn cmd_setup() -> Result<()> {
    init_tracing("info");

    println!();
    println!("  OpenIntentOS Setup Wizard");
    println!("  ========================");
    println!();

    let data_dir = Path::new("data");
    if !data_dir.exists() {
        std::fs::create_dir_all(data_dir)?;
        println!("  [+] Created data directory");
    } else {
        println!("  [=] Data directory already exists");
    }

    let db_path = data_dir.join("openintent.db");
    let display_path = db_path.display().to_string();
    openintent_store::Database::open_and_migrate(db_path)
        .await
        .context("failed to initialize database")?;
    println!("  [+] Database initialized at {display_path}");

    let has_anthropic = env_non_empty("ANTHROPIC_API_KEY").is_some();
    let has_openai = env_non_empty("OPENAI_API_KEY").is_some();
    let has_deepseek = env_non_empty("DEEPSEEK_API_KEY").is_some();
    let has_keychain = read_claude_code_keychain_token().is_some();

    if has_anthropic {
        println!("  [+] ANTHROPIC_API_KEY is set");
    }
    if has_openai {
        println!("  [+] OPENAI_API_KEY is set");
    }
    if has_deepseek {
        println!("  [+] DEEPSEEK_API_KEY is set");
    }
    if has_keychain {
        println!("  [+] Claude Code OAuth token found in Keychain");
    }

    if !has_anthropic && !has_openai && !has_deepseek && !has_keychain {
        println!("  [!] No LLM API key found. Set one of:");
        println!("      export ANTHROPIC_API_KEY=sk-ant-...");
        println!("      export OPENAI_API_KEY=sk-...");
        println!("      export DEEPSEEK_API_KEY=sk-...");
        println!("      Or install Claude Code for automatic OAuth.");
        println!("      (Ollama at localhost:11434 will be used as fallback)");
    }

    println!();
    println!("  Setup complete! Run `openintent run` to start.");
    println!();

    Ok(())
}

// ---------------------------------------------------------------------------
// Subcommand: status
// ---------------------------------------------------------------------------

async fn cmd_status() -> Result<()> {
    init_tracing("warn");

    println!();
    println!("  OpenIntentOS Status");
    println!("  ===================");
    println!();

    let data_dir = Path::new("data");
    if data_dir.exists() {
        println!("  Data directory:   OK");
    } else {
        println!("  Data directory:   MISSING (run `openintent setup`)");
    }

    let db_path = data_dir.join("openintent.db");
    if db_path.exists() {
        println!("  Database:         OK ({})", db_path.display());
    } else {
        println!("  Database:         NOT INITIALIZED (run `openintent setup`)");
    }

    let providers: Vec<&str> = [
        env_non_empty("ANTHROPIC_API_KEY").map(|_| "Anthropic"),
        env_non_empty("OPENAI_API_KEY").map(|_| "OpenAI"),
        env_non_empty("DEEPSEEK_API_KEY").map(|_| "DeepSeek"),
        read_claude_code_keychain_token().map(|_| "Claude Code OAuth"),
    ]
    .into_iter()
    .flatten()
    .collect();

    if providers.is_empty() {
        println!("  LLM providers:    NONE (will fall back to Ollama)");
    } else {
        println!("  LLM providers:    {}", providers.join(", "));
    }

    let config_path = Path::new("config/default.toml");
    if config_path.exists() {
        println!("  Config:           OK ({})", config_path.display());
    } else {
        println!("  Config:           MISSING");
    }

    println!();

    Ok(())
}

// ---------------------------------------------------------------------------
// Subcommand: gui
// ---------------------------------------------------------------------------

async fn cmd_gui() -> Result<()> {
    init_tracing("info");

    info!("starting OpenIntentOS desktop GUI");

    println!();
    println!("  OpenIntentOS Desktop v{}", env!("CARGO_PKG_VERSION"));
    println!("  Launching iced GUI...");
    println!();

    openintent_ui::run_desktop_ui().map_err(|e| anyhow::anyhow!("GUI error: {e}"))?;

    Ok(())
}

// ---------------------------------------------------------------------------
// Subcommand: users
// ---------------------------------------------------------------------------

async fn cmd_users(action: UserAction) -> Result<()> {
    init_tracing("warn");

    let data_dir = Path::new("data");
    let db_path = data_dir.join("openintent.db");

    if !db_path.exists() {
        eprintln!("  Error: Database not found. Run `openintent setup` first.");
        std::process::exit(1);
    }

    let db = openintent_store::Database::open_and_migrate(db_path)
        .await
        .context("failed to open database")?;
    let users = openintent_store::UserStore::new(db);

    match action {
        UserAction::List => {
            let all = users.list(1000, 0).await.context("failed to list users")?;

            if all.is_empty() {
                println!("  No users found.");
                return Ok(());
            }

            println!();
            println!(
                "  {:<36} {:<20} {:<20} {:<8} {:<8}",
                "ID", "USERNAME", "DISPLAY NAME", "ROLE", "ACTIVE"
            );
            println!("  {}", "-".repeat(96));

            for u in &all {
                let display = u.display_name.as_deref().unwrap_or("-");
                println!(
                    "  {:<36} {:<20} {:<20} {:<8} {:<8}",
                    u.id, u.username, display, u.role, u.active
                );
            }
            println!();
        }

        UserAction::Create {
            username,
            password,
            display_name,
            role,
        } => {
            let role = match role.as_str() {
                "admin" => openintent_store::UserRole::Admin,
                "user" => openintent_store::UserRole::User,
                "viewer" => openintent_store::UserRole::Viewer,
                other => {
                    eprintln!("  Error: Unknown role '{other}'. Use 'admin', 'user', or 'viewer'.");
                    std::process::exit(1);
                }
            };

            let user = users
                .create(&username, display_name.as_deref(), &password, role)
                .await
                .context("failed to create user")?;

            println!("  Created user: {} (id: {})", user.username, user.id);
        }

        UserAction::Delete { username } => {
            let user = users
                .get_by_username(&username)
                .await
                .context("failed to look up user")?;

            let user = match user {
                Some(u) => u,
                None => {
                    eprintln!("  Error: User '{}' not found.", username);
                    std::process::exit(1);
                }
            };

            users
                .delete(&user.id)
                .await
                .context("failed to delete user")?;

            println!("  Deleted user: {}", username);
        }
    }

    Ok(())
}

// ---------------------------------------------------------------------------
// Subcommand: skills
// ---------------------------------------------------------------------------

async fn cmd_skills(action: SkillAction) -> Result<()> {
    init_tracing("warn");

    let skills_dir = openintent_skills::default_skills_dir();

    match action {
        SkillAction::List => {
            let mut mgr = openintent_skills::SkillManager::new(skills_dir);
            mgr.load_all().context("failed to load skills")?;

            let skills_with_status = mgr.list_with_status();
            if skills_with_status.is_empty() {
                println!("  No skills installed.");
                println!();
                println!("  Install from ClawHub:  openintent skills install <slug>");
                println!("  Install from URL:      openintent skills install github:owner/repo");
                println!("  Search registry:       openintent skills search <query>");
                return Ok(());
            }

            println!();
            println!("  Installed skills ({}):", skills_with_status.len());
            println!();
            for (skill, status) in &skills_with_status {
                let status_label = match status {
                    openintent_skills::SkillStatus::Ready => "ready",
                    openintent_skills::SkillStatus::Degraded => "degraded",
                    openintent_skills::SkillStatus::Unavailable => "unavailable",
                };
                let version = skill.version.as_deref().unwrap_or("-");
                let scripts = skill.scripts.len();
                println!(
                    "  {:<24} v{:<8} scripts:{:<3} [{}]  {}",
                    skill.name, version, scripts, status_label, skill.description
                );
            }
            println!();
        }

        SkillAction::Install { source } => {
            let mut mgr = openintent_skills::SkillManager::new(skills_dir);
            mgr.ensure_dir()
                .context("failed to create skills directory")?;
            mgr.load_all().context("failed to load skills")?;

            let is_url = source.starts_with("http://")
                || source.starts_with("https://")
                || source.starts_with("github:");

            let skill = if is_url {
                println!("  Installing skill from URL: {source}");
                mgr.install_from_url(&source)
                    .await
                    .context("failed to install skill from URL")?
            } else {
                println!("  Installing skill from ClawHub: {source}");
                mgr.install_from_registry(&source)
                    .await
                    .context("failed to install skill from registry")?
            };

            let script_count = skill.scripts.len();
            println!();
            println!("  Installed: {}", skill.name);
            println!("  Description: {}", skill.description);
            if let Some(ref v) = skill.version {
                println!("  Version: {v}");
            }
            if script_count > 0 {
                println!("  Script tools: {script_count}");
            }
            if !skill.metadata.requires.env.is_empty() {
                println!(
                    "  Required env vars: {}",
                    skill.metadata.requires.env.join(", ")
                );
            }
            println!();
        }

        SkillAction::Remove { name } => {
            let mut mgr = openintent_skills::SkillManager::new(skills_dir);
            mgr.load_all().context("failed to load skills")?;

            mgr.remove(&name).context("failed to remove skill")?;
            println!("  Removed skill: {name}");
        }

        SkillAction::Search { query, limit } => {
            println!("  Searching ClawHub for: {query}");
            println!();

            let mgr = openintent_skills::SkillManager::new(skills_dir);
            match mgr.search(&query, limit).await {
                Ok(results) => {
                    if results.is_empty() {
                        println!("  No results found.");
                    } else {
                        println!("  Results ({}):", results.len());
                        println!();
                        for skill in &results {
                            let installs = skill
                                .installs
                                .map(|n| format!("{n} installs"))
                                .unwrap_or_default();
                            println!("  {:<28} {}  {}", skill.slug, skill.description, installs);
                        }
                    }
                    println!();
                    println!("  Install with: openintent skills install <slug>");
                }
                Err(e) => {
                    eprintln!("  Error: Failed to search registry: {e}");
                    eprintln!("  (ClawHub registry may be unreachable)");
                }
            }
            println!();
        }

        SkillAction::Info { name } => {
            let mut mgr = openintent_skills::SkillManager::new(skills_dir);
            mgr.load_all().context("failed to load skills")?;

            let skill = mgr.get(&name);
            match skill {
                Some(skill) => {
                    let status = openintent_skills::check_requirements(skill);
                    let status_label = match status {
                        openintent_skills::SkillStatus::Ready => "ready",
                        openintent_skills::SkillStatus::Degraded => "degraded",
                        openintent_skills::SkillStatus::Unavailable => "unavailable",
                    };
                    println!();
                    println!("  Skill: {}", skill.name);
                    println!("  Description: {}", skill.description);
                    if let Some(ref v) = skill.version {
                        println!("  Version: {v}");
                    }
                    if let Some(ref author) = skill.metadata.author {
                        println!("  Author: {author}");
                    }
                    if let Some(ref homepage) = skill.metadata.homepage {
                        println!("  Homepage: {homepage}");
                    }
                    println!("  Status: {status_label}");
                    if !skill.metadata.tags.is_empty() {
                        println!("  Tags: {}", skill.metadata.tags.join(", "));
                    }
                    if !skill.metadata.requires.env.is_empty() {
                        println!("  Required env: {}", skill.metadata.requires.env.join(", "));
                    }
                    if !skill.metadata.requires.bins.is_empty() {
                        println!(
                            "  Required bins: {}",
                            skill.metadata.requires.bins.join(", ")
                        );
                    }
                    if !skill.scripts.is_empty() {
                        println!("  Scripts:");
                        for s in &skill.scripts {
                            println!("    {} ({:?})", s.filename, s.interpreter);
                        }
                    }
                    println!();
                    println!("  --- Instructions ---");
                    println!("{}", skill.instructions);
                    println!();
                }
                None => {
                    eprintln!("  Error: Skill '{name}' is not installed.");
                    std::process::exit(1);
                }
            }
        }
    }

    Ok(())
}
