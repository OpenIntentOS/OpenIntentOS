//! CLI entry point for OpenIntentOS.
//!
//! Provides the `openintent` command with subcommands for running the AI agent
//! REPL, launching a web server, running setup, and checking system status.
//!
//! The REPL uses the full ReAct (Reason + Act) loop: user input is sent to the
//! LLM, which can invoke tools through adapters, and the results are fed back
//! until the LLM produces a final text response.

use std::io::{self, Write as _};
use std::path::Path;
use std::sync::Arc;

use anyhow::{Context, Result};
use async_trait::async_trait;
use chrono::{TimeZone, Utc};
use clap::{Parser, Subcommand};
use openintent_adapters::Adapter;
use openintent_agent::runtime::ToolAdapter;
use openintent_agent::{
    AgentConfig, AgentContext, LlmClient, LlmClientConfig, Message, react_loop,
};
use serde_json::Value;
use tracing::info;
use tracing_subscriber::EnvFilter;

use openintent_store::SessionStore;

// ---------------------------------------------------------------------------
// CLI definition
// ---------------------------------------------------------------------------

/// OpenIntentOS -- an AI-powered operating system.
#[derive(Parser)]
#[command(
    name = "openintent",
    version,
    about = "OpenIntentOS -- AI-powered operating system",
    long_about = "An AI operating system that understands your intents and executes tasks \
                  using available tools and adapters."
)]
struct Cli {
    #[command(subcommand)]
    command: Commands,
}

#[derive(Subcommand)]
enum Commands {
    /// Start the OpenIntentOS agent REPL.
    Run {
        /// Resume or create a named session for conversation persistence.
        #[arg(long, short)]
        session: Option<String>,
    },

    /// Start the web server with embedded chat UI.
    Serve {
        /// Address to bind the HTTP server to.
        #[arg(long, default_value = "0.0.0.0")]
        bind: String,

        /// Port to listen on.
        #[arg(long, short, default_value_t = 3000)]
        port: u16,
    },

    /// Run the interactive setup wizard.
    Setup,

    /// Show current system status.
    Status,

    /// Manage conversation sessions.
    Sessions {
        #[command(subcommand)]
        action: SessionAction,
    },

    /// Start the terminal UI (ratatui).
    Tui {
        /// Resume or create a named session.
        #[arg(long, short)]
        session: Option<String>,
    },

    /// Start the desktop GUI (iced).
    Gui,

    /// Manage user accounts.
    Users {
        #[command(subcommand)]
        action: UserAction,
    },
}

/// Actions for managing conversation sessions.
#[derive(Subcommand)]
enum SessionAction {
    /// List all saved sessions.
    List,
    /// Show messages from a session.
    Show {
        /// The session name to display.
        name: String,
    },
    /// Delete a session.
    Delete {
        /// The session name to delete.
        name: String,
    },
}

/// Actions for managing user accounts.
#[derive(Subcommand)]
enum UserAction {
    /// List all users.
    List,
    /// Create a new user.
    Create {
        /// The username for the new account.
        username: String,
        /// The password for the new account.
        #[arg(long, short)]
        password: String,
        /// Optional display name.
        #[arg(long, short)]
        display_name: Option<String>,
        /// Role: admin, user, or viewer.
        #[arg(long, short, default_value = "user")]
        role: String,
    },
    /// Delete a user by username.
    Delete {
        /// The username to delete.
        username: String,
    },
}

// ---------------------------------------------------------------------------
// Adapter bridge
// ---------------------------------------------------------------------------

/// Bridges [`openintent_adapters::Adapter`] to [`openintent_agent::runtime::ToolAdapter`].
///
/// The two traits have slightly different signatures:
/// - `Adapter::tools()` returns `ToolDefinition` with a `parameters` field.
/// - `ToolAdapter::tool_definitions()` returns `ToolDefinition` with an `input_schema` field.
/// - `Adapter::execute_tool()` returns `Result<Value>`.
/// - `ToolAdapter::execute()` returns `Result<String>`.
///
/// This struct wraps an adapter and handles the conversions.
struct AdapterBridge {
    adapter: Arc<dyn openintent_adapters::Adapter>,
}

impl AdapterBridge {
    fn new(adapter: impl openintent_adapters::Adapter + 'static) -> Self {
        Self {
            adapter: Arc::new(adapter),
        }
    }

    /// Convert an adapter-side `ToolDefinition` to an agent-side `ToolDefinition`.
    fn convert_tool_def(
        td: &openintent_adapters::ToolDefinition,
    ) -> openintent_agent::ToolDefinition {
        openintent_agent::ToolDefinition {
            name: td.name.clone(),
            description: td.description.clone(),
            input_schema: td.parameters.clone(),
        }
    }
}

#[async_trait]
impl ToolAdapter for AdapterBridge {
    fn adapter_id(&self) -> &str {
        self.adapter.id()
    }

    fn tool_definitions(&self) -> Vec<openintent_agent::ToolDefinition> {
        self.adapter
            .tools()
            .iter()
            .map(Self::convert_tool_def)
            .collect()
    }

    async fn execute(&self, tool_name: &str, arguments: Value) -> openintent_agent::Result<String> {
        let result = self
            .adapter
            .execute_tool(tool_name, arguments)
            .await
            .map_err(|e| openintent_agent::AgentError::ToolExecutionFailed {
                tool_name: tool_name.to_owned(),
                reason: e.to_string(),
            })?;

        // Serialize the JSON Value result to a string for the LLM.
        let text = serde_json::to_string_pretty(&result).unwrap_or_else(|_| result.to_string());
        Ok(text)
    }
}

// ---------------------------------------------------------------------------
// Main
// ---------------------------------------------------------------------------

#[tokio::main]
async fn main() -> Result<()> {
    let cli = Cli::parse();

    match cli.command {
        Commands::Run { session } => cmd_run(session).await,
        Commands::Serve { bind, port } => cmd_serve(bind, port).await,
        Commands::Setup => cmd_setup().await,
        Commands::Status => cmd_status().await,
        Commands::Sessions { action } => cmd_sessions(action).await,
        Commands::Tui { session } => cmd_tui(session).await,
        Commands::Gui => cmd_gui().await,
        Commands::Users { action } => cmd_users(action).await,
    }
}

// ---------------------------------------------------------------------------
// Subcommand: run
// ---------------------------------------------------------------------------

async fn cmd_run(session_name: Option<String>) -> Result<()> {
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

    // 3. Set up session persistence.
    let sessions = SessionStore::new(db.clone());

    let active_session = if let Some(ref name) = session_name {
        // Try to find an existing session with the given name.
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
                let model_name = std::env::var("OPENINTENT_MODEL")
                    .unwrap_or_else(|_| "claude-sonnet-4-20250514".to_owned());
                let session = sessions
                    .create(name, &model_name)
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

    // 4. Check for the API key.
    let api_key = match std::env::var("ANTHROPIC_API_KEY") {
        Ok(key) if !key.is_empty() => key,
        _ => {
            eprintln!();
            eprintln!("  Error: ANTHROPIC_API_KEY environment variable is not set.");
            eprintln!();
            eprintln!("  The OpenIntentOS agent requires an Anthropic API key to function.");
            eprintln!("  Set it in your environment before running:");
            eprintln!();
            eprintln!("    export ANTHROPIC_API_KEY=sk-ant-...");
            eprintln!();
            std::process::exit(1);
        }
    };

    // 5. Determine the model to use.
    let model =
        std::env::var("OPENINTENT_MODEL").unwrap_or_else(|_| "claude-sonnet-4-20250514".to_owned());

    // 6. Create the LLM client.
    let llm_config = LlmClientConfig::anthropic(&api_key, &model);
    let llm = Arc::new(LlmClient::new(llm_config).context("failed to create LLM client")?);
    info!(model = %model, "LLM client ready");

    // 7. Initialize and connect adapters.
    let cwd = std::env::current_dir().context("failed to get current directory")?;

    let mut fs_adapter = openintent_adapters::FilesystemAdapter::new("filesystem", cwd.clone());
    fs_adapter.connect().await?;

    let mut shell_adapter = openintent_adapters::ShellAdapter::new("shell", cwd);
    shell_adapter.connect().await?;

    let mut web_search_adapter = openintent_adapters::WebSearchAdapter::new("web_search");
    web_search_adapter.connect().await?;

    let mut web_fetch_adapter = openintent_adapters::WebFetchAdapter::new("web_fetch");
    web_fetch_adapter.connect().await?;

    let mut http_adapter = openintent_adapters::HttpRequestAdapter::new("http_request");
    http_adapter.connect().await?;

    let mut cron_adapter = openintent_adapters::CronAdapter::new("cron");
    cron_adapter.connect().await?;

    let memory = Arc::new(openintent_store::SemanticMemory::new(db.clone()));
    let mut memory_adapter =
        openintent_adapters::MemoryToolsAdapter::new("memory", Arc::clone(&memory));
    memory_adapter.connect().await?;

    let mut github_adapter = openintent_adapters::GitHubAdapter::new("github");
    github_adapter.connect().await?;

    let mut email_adapter = openintent_adapters::EmailAdapter::new("email");
    email_adapter.connect().await?;

    let mut browser_adapter = openintent_adapters::BrowserAdapter::new("browser");
    if let Err(e) = browser_adapter.connect().await {
        tracing::warn!(error = %e, "browser adapter failed to connect (Chrome may not be running)");
    }

    let mut feishu_adapter = openintent_adapters::FeishuAdapter::new("feishu");
    feishu_adapter.connect().await?;

    let mut calendar_adapter = openintent_adapters::CalendarAdapter::new("calendar");
    calendar_adapter.connect().await?;

    info!(
        "adapters initialized (filesystem, shell, web_search, web_fetch, http_request, cron, memory, github, email, browser, feishu, calendar)"
    );

    // 8. Wrap adapters in the bridge.
    let adapters: Vec<Arc<dyn ToolAdapter>> = vec![
        Arc::new(AdapterBridge::new(fs_adapter)),
        Arc::new(AdapterBridge::new(shell_adapter)),
        Arc::new(AdapterBridge::new(web_search_adapter)),
        Arc::new(AdapterBridge::new(web_fetch_adapter)),
        Arc::new(AdapterBridge::new(http_adapter)),
        Arc::new(AdapterBridge::new(cron_adapter)),
        Arc::new(AdapterBridge::new(memory_adapter)),
        Arc::new(AdapterBridge::new(github_adapter)),
        Arc::new(AdapterBridge::new(email_adapter)),
        Arc::new(AdapterBridge::new(browser_adapter)),
        Arc::new(AdapterBridge::new(feishu_adapter)),
        Arc::new(AdapterBridge::new(calendar_adapter)),
    ];

    // 9. Load system prompt.
    let system_prompt = load_system_prompt();

    // 10. Load session history if resuming.
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

    // 11. Print startup banner.
    println!();
    println!("  OpenIntentOS v{}", env!("CARGO_PKG_VERSION"));
    println!("  Model: {model}");
    println!("  Adapters: filesystem, shell, web_search, web_fetch, http_request, cron, memory,");
    println!("            github, email, browser, feishu, calendar");
    if let Some(ref name) = session_name {
        println!("  Session: {name}");
    }
    println!("  Type your request, or 'quit' to exit.");
    println!();

    // 12. Set up Ctrl+C handler.
    let running = Arc::new(std::sync::atomic::AtomicBool::new(true));
    {
        let running = running.clone();
        tokio::spawn(async move {
            if tokio::signal::ctrl_c().await.is_ok() {
                running.store(false, std::sync::atomic::Ordering::SeqCst);
                // Print a clean exit message.
                eprintln!("\n  Interrupted. Goodbye!");
                std::process::exit(0);
            }
        });
    }

    // 13. REPL loop.
    let stdin = io::stdin();
    let mut line_buf = String::new();

    loop {
        // Print prompt and flush.
        print!("> ");
        io::stdout().flush().ok();

        // Read a line.
        line_buf.clear();
        let bytes_read = stdin.read_line(&mut line_buf);
        match bytes_read {
            Ok(0) => {
                // EOF (Ctrl+D).
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

        // Show a thinking indicator.
        print!("  Thinking...");
        io::stdout().flush().ok();

        // Build agent context for this request.
        let agent_config = AgentConfig {
            max_turns: 20,
            model: model.clone(),
            temperature: Some(0.0),
            max_tokens: Some(4096),
            ..AgentConfig::default()
        };

        let mut ctx = AgentContext::new(llm.clone(), adapters.clone(), agent_config)
            .with_system_prompt(&system_prompt);

        // Inject session history before the current user message.
        for msg in &history_messages {
            ctx.messages.push(msg.clone());
        }

        // Add the current user message.
        ctx = ctx.with_user_message(trimmed);

        // Run the ReAct loop.
        match react_loop(&mut ctx).await {
            Ok(response) => {
                // Clear the "Thinking..." line.
                print!("\r                    \r");
                io::stdout().flush().ok();

                // Print the final response.
                println!("{}", response.text);

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
                println!();

                // Persist assistant message to session.
                if let Some(ref sid) = session_id
                    && let Err(e) = sessions
                        .append_message(sid, "assistant", &response.text, None, None)
                        .await
                {
                    tracing::warn!(error = %e, "failed to persist assistant message");
                }

                // Update rolling history for future turns in this REPL session.
                history_messages.push(Message::user(trimmed));
                history_messages.push(Message::assistant(&response.text));
            }
            Err(e) => {
                // Clear the "Thinking..." line.
                print!("\r                    \r");
                io::stdout().flush().ok();

                eprintln!("  Error: {e}");
                eprintln!();
            }
        }

        // Check if we should still be running.
        if !running.load(std::sync::atomic::Ordering::SeqCst) {
            break;
        }
    }

    info!("shutting down");
    Ok(())
}

// ---------------------------------------------------------------------------
// Subcommand: tui
// ---------------------------------------------------------------------------

async fn cmd_tui(_session_name: Option<String>) -> Result<()> {
    // 1. Initialize tracing (to file so it does not interfere with the TUI).
    init_tracing("info");

    info!("starting OpenIntentOS TUI");

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

    // 3. Check for the API key.
    let api_key = match std::env::var("ANTHROPIC_API_KEY") {
        Ok(key) if !key.is_empty() => key,
        _ => {
            eprintln!();
            eprintln!("  Error: ANTHROPIC_API_KEY environment variable is not set.");
            eprintln!();
            eprintln!("  The OpenIntentOS agent requires an Anthropic API key to function.");
            eprintln!("  Set it in your environment before running:");
            eprintln!();
            eprintln!("    export ANTHROPIC_API_KEY=sk-ant-...");
            eprintln!();
            std::process::exit(1);
        }
    };

    // 4. Determine the model to use.
    let model =
        std::env::var("OPENINTENT_MODEL").unwrap_or_else(|_| "claude-sonnet-4-20250514".to_owned());

    // 5. Create the LLM client.
    let llm_config = LlmClientConfig::anthropic(&api_key, &model);
    let llm = Arc::new(LlmClient::new(llm_config).context("failed to create LLM client")?);
    info!(model = %model, "LLM client ready");

    // 6. Initialize and connect adapters.
    let cwd = std::env::current_dir().context("failed to get current directory")?;

    let mut fs_adapter = openintent_adapters::FilesystemAdapter::new("filesystem", cwd.clone());
    fs_adapter.connect().await?;

    let mut shell_adapter = openintent_adapters::ShellAdapter::new("shell", cwd);
    shell_adapter.connect().await?;

    let mut web_search_adapter = openintent_adapters::WebSearchAdapter::new("web_search");
    web_search_adapter.connect().await?;

    let mut web_fetch_adapter = openintent_adapters::WebFetchAdapter::new("web_fetch");
    web_fetch_adapter.connect().await?;

    let mut http_adapter = openintent_adapters::HttpRequestAdapter::new("http_request");
    http_adapter.connect().await?;

    let mut cron_adapter = openintent_adapters::CronAdapter::new("cron");
    cron_adapter.connect().await?;

    let memory = Arc::new(openintent_store::SemanticMemory::new(db.clone()));
    let mut memory_adapter =
        openintent_adapters::MemoryToolsAdapter::new("memory", Arc::clone(&memory));
    memory_adapter.connect().await?;

    let mut github_adapter = openintent_adapters::GitHubAdapter::new("github");
    github_adapter.connect().await?;

    let mut email_adapter = openintent_adapters::EmailAdapter::new("email");
    email_adapter.connect().await?;

    let mut browser_adapter = openintent_adapters::BrowserAdapter::new("browser");
    if let Err(e) = browser_adapter.connect().await {
        tracing::warn!(error = %e, "browser adapter failed to connect (Chrome may not be running)");
    }

    let mut feishu_adapter = openintent_adapters::FeishuAdapter::new("feishu");
    feishu_adapter.connect().await?;

    let mut calendar_adapter = openintent_adapters::CalendarAdapter::new("calendar");
    calendar_adapter.connect().await?;

    info!(
        "adapters initialized (filesystem, shell, web_search, web_fetch, http_request, cron, memory, github, email, browser, feishu, calendar)"
    );

    // 7. Wrap adapters in the bridge.
    let adapters: Vec<Arc<dyn ToolAdapter>> = vec![
        Arc::new(AdapterBridge::new(fs_adapter)),
        Arc::new(AdapterBridge::new(shell_adapter)),
        Arc::new(AdapterBridge::new(web_search_adapter)),
        Arc::new(AdapterBridge::new(web_fetch_adapter)),
        Arc::new(AdapterBridge::new(http_adapter)),
        Arc::new(AdapterBridge::new(cron_adapter)),
        Arc::new(AdapterBridge::new(memory_adapter)),
        Arc::new(AdapterBridge::new(github_adapter)),
        Arc::new(AdapterBridge::new(email_adapter)),
        Arc::new(AdapterBridge::new(browser_adapter)),
        Arc::new(AdapterBridge::new(feishu_adapter)),
        Arc::new(AdapterBridge::new(calendar_adapter)),
    ];

    // 8. Load system prompt.
    let system_prompt = load_system_prompt();

    // 9. Build agent config.
    let config = AgentConfig {
        max_turns: 20,
        model,
        temperature: Some(0.0),
        max_tokens: Some(4096),
        ..AgentConfig::default()
    };

    // 10. Launch the TUI.
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

    // Open the database.
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

                // Indent message content for readability.
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

    // 1. Create data directory and initialize SQLite.
    let data_dir = Path::new("data");
    if !data_dir.exists() {
        std::fs::create_dir_all(data_dir).context("failed to create data directory")?;
    }

    let db_path = data_dir.join("openintent.db");
    let db = openintent_store::Database::open_and_migrate(db_path.clone())
        .await
        .context("failed to open database")?;
    info!(path = %db_path.display(), "store initialized");

    // 2. Check for the API key.
    let api_key = match std::env::var("ANTHROPIC_API_KEY") {
        Ok(key) if !key.is_empty() => key,
        _ => {
            eprintln!();
            eprintln!("  Error: ANTHROPIC_API_KEY environment variable is not set.");
            eprintln!();
            eprintln!("  Set it in your environment before running:");
            eprintln!("    export ANTHROPIC_API_KEY=sk-ant-...");
            eprintln!();
            std::process::exit(1);
        }
    };

    // 3. Create the LLM client.
    let model =
        std::env::var("OPENINTENT_MODEL").unwrap_or_else(|_| "claude-sonnet-4-20250514".to_owned());
    let llm_config = LlmClientConfig::anthropic(&api_key, &model);
    let llm = Arc::new(LlmClient::new(llm_config).context("failed to create LLM client")?);
    info!(model = %model, "LLM client ready");

    // 4. Initialize and connect adapters.
    let cwd = std::env::current_dir().context("failed to get current directory")?;

    let mut fs_adapter = openintent_adapters::FilesystemAdapter::new("filesystem", cwd.clone());
    fs_adapter.connect().await?;

    let mut shell_adapter = openintent_adapters::ShellAdapter::new("shell", cwd);
    shell_adapter.connect().await?;

    let mut web_search_adapter = openintent_adapters::WebSearchAdapter::new("web_search");
    web_search_adapter.connect().await?;

    let mut web_fetch_adapter = openintent_adapters::WebFetchAdapter::new("web_fetch");
    web_fetch_adapter.connect().await?;

    let mut http_adapter = openintent_adapters::HttpRequestAdapter::new("http_request");
    http_adapter.connect().await?;

    let mut cron_adapter = openintent_adapters::CronAdapter::new("cron");
    cron_adapter.connect().await?;

    let memory = Arc::new(openintent_store::SemanticMemory::new(db.clone()));
    let mut memory_adapter =
        openintent_adapters::MemoryToolsAdapter::new("memory", Arc::clone(&memory));
    memory_adapter.connect().await?;

    let mut github_adapter = openintent_adapters::GitHubAdapter::new("github");
    github_adapter.connect().await?;

    let mut email_adapter = openintent_adapters::EmailAdapter::new("email");
    email_adapter.connect().await?;

    let mut browser_adapter = openintent_adapters::BrowserAdapter::new("browser");
    if let Err(e) = browser_adapter.connect().await {
        tracing::warn!(error = %e, "browser adapter failed to connect (Chrome may not be running)");
    }

    let mut feishu_adapter = openintent_adapters::FeishuAdapter::new("feishu");
    feishu_adapter.connect().await?;

    let mut calendar_adapter = openintent_adapters::CalendarAdapter::new("calendar");
    calendar_adapter.connect().await?;

    let adapters: Vec<Arc<dyn openintent_adapters::Adapter>> = vec![
        Arc::new(fs_adapter),
        Arc::new(shell_adapter),
        Arc::new(web_search_adapter),
        Arc::new(web_fetch_adapter),
        Arc::new(http_adapter),
        Arc::new(cron_adapter),
        Arc::new(memory_adapter),
        Arc::new(github_adapter),
        Arc::new(email_adapter),
        Arc::new(browser_adapter),
        Arc::new(feishu_adapter),
        Arc::new(calendar_adapter),
    ];

    info!(
        "adapters initialized (filesystem, shell, web_search, web_fetch, http_request, cron, memory, github, email, browser, feishu, calendar)"
    );

    // 5. Configure and start the web server.
    let web_config = openintent_web::WebConfig {
        bind_addr: bind,
        port,
    };

    println!();
    println!("  OpenIntentOS v{}", env!("CARGO_PKG_VERSION"));
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

    let server = openintent_web::WebServer::new(web_config, llm, adapters, db);
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

    // Step 1: Create data directory.
    let data_dir = Path::new("data");
    if !data_dir.exists() {
        std::fs::create_dir_all(data_dir)?;
        println!("  [+] Created data directory");
    } else {
        println!("  [=] Data directory already exists");
    }

    // Step 2: Initialize the database.
    let db_path = data_dir.join("openintent.db");
    let display_path = db_path.display().to_string();
    openintent_store::Database::open_and_migrate(db_path)
        .await
        .context("failed to initialize database")?;
    println!("  [+] Database initialized at {display_path}");

    // Step 3: Check for API key.
    match std::env::var("ANTHROPIC_API_KEY") {
        Ok(_) => println!("  [+] ANTHROPIC_API_KEY is set"),
        Err(_) => {
            println!("  [!] ANTHROPIC_API_KEY is not set");
            println!("      Set it in your environment to enable LLM features:");
            println!("      export ANTHROPIC_API_KEY=sk-ant-...");
        }
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

    // Check data directory.
    let data_dir = Path::new("data");
    if data_dir.exists() {
        println!("  Data directory:   OK");
    } else {
        println!("  Data directory:   MISSING (run `openintent setup`)");
    }

    // Check database.
    let db_path = data_dir.join("openintent.db");
    if db_path.exists() {
        println!("  Database:         OK ({})", db_path.display());
    } else {
        println!("  Database:         NOT INITIALIZED (run `openintent setup`)");
    }

    // Check API key.
    match std::env::var("ANTHROPIC_API_KEY") {
        Ok(_) => println!("  Anthropic API:    CONFIGURED"),
        Err(_) => println!("  Anthropic API:    NOT SET"),
    }

    // Check config.
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

    // Open the database.
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
// Helpers
// ---------------------------------------------------------------------------

/// Initialize the tracing subscriber with the given default log level.
fn init_tracing(default_level: &str) {
    let filter =
        EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new(default_level));

    tracing_subscriber::fmt()
        .with_env_filter(filter)
        .with_target(false)
        .compact()
        .init();
}

/// Load the system prompt from `config/IDENTITY.md` if it exists, otherwise
/// return a sensible default.
fn load_system_prompt() -> String {
    let identity_path = Path::new("config/IDENTITY.md");

    if identity_path.exists() {
        std::fs::read_to_string(identity_path).unwrap_or_else(|_| default_system_prompt())
    } else {
        default_system_prompt()
    }
}

/// The fallback system prompt used when no IDENTITY.md is found.
fn default_system_prompt() -> String {
    "You are OpenIntentOS, an AI-powered operating system assistant. \
     Your role is to understand user intents and execute tasks using available tools. \
     Be concise, accurate, and proactive. Always confirm before destructive actions."
        .to_owned()
}
