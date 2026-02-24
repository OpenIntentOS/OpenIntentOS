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
use clap::{Parser, Subcommand};
use openintent_adapters::Adapter;
use openintent_agent::runtime::ToolAdapter;
use openintent_agent::{AgentConfig, AgentContext, LlmClient, LlmClientConfig, react_loop};
use serde_json::Value;
use tracing::info;
use tracing_subscriber::EnvFilter;

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
    Run,

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
        Commands::Run => cmd_run().await,
        Commands::Serve { bind, port } => cmd_serve(bind, port).await,
        Commands::Setup => cmd_setup().await,
        Commands::Status => cmd_status().await,
    }
}

// ---------------------------------------------------------------------------
// Subcommand: run
// ---------------------------------------------------------------------------

async fn cmd_run() -> Result<()> {
    // 1. Initialize tracing.
    init_tracing("info");

    info!("starting OpenIntentOS");

    // 2. Create data directory and initialize SQLite.
    let data_dir = Path::new("data");
    if !data_dir.exists() {
        std::fs::create_dir_all(data_dir).context("failed to create data directory")?;
    }

    let db_path = data_dir.join("openintent.db");
    let _db = openintent_store::Database::open_and_migrate(db_path.clone())
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

    let memory = Arc::new(openintent_store::SemanticMemory::new(_db.clone()));
    let mut memory_adapter =
        openintent_adapters::MemoryToolsAdapter::new("memory", Arc::clone(&memory));
    memory_adapter.connect().await?;

    info!(
        "adapters initialized (filesystem, shell, web_search, web_fetch, http_request, cron, memory)"
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
    ];

    // 8. Load system prompt.
    let system_prompt = load_system_prompt();

    // 9. Print startup banner.
    println!();
    println!("  OpenIntentOS v{}", env!("CARGO_PKG_VERSION"));
    println!("  Model: {model}");
    println!("  Adapters: filesystem, shell, web_search, web_fetch, http_request, cron, memory");
    println!("  Type your request, or 'quit' to exit.");
    println!();

    // 10. Set up Ctrl+C handler.
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

    // 11. REPL loop.
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

        // Show a thinking indicator.
        print!("  Thinking...");
        io::stdout().flush().ok();

        // Build agent context for this request.
        let agent_config = AgentConfig {
            max_turns: 20,
            model: model.clone(),
            temperature: Some(0.0),
            max_tokens: Some(4096),
        };

        let mut ctx = AgentContext::new(llm.clone(), adapters.clone(), agent_config)
            .with_system_prompt(&system_prompt)
            .with_user_message(trimmed);

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

    let adapters: Vec<Arc<dyn openintent_adapters::Adapter>> = vec![
        Arc::new(fs_adapter),
        Arc::new(shell_adapter),
        Arc::new(web_search_adapter),
        Arc::new(web_fetch_adapter),
        Arc::new(http_adapter),
        Arc::new(cron_adapter),
        Arc::new(memory_adapter),
    ];

    info!(
        "adapters initialized (filesystem, shell, web_search, web_fetch, http_request, cron, memory)"
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
        "  Web UI: http://{}:{}",
        web_config.bind_addr, web_config.port
    );
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
