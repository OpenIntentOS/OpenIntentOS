//! CLI entry point for OpenIntentOS.
//!
//! This binary provides the `openintent` command with subcommands for
//! starting the OS, running setup, and checking system status.

use std::io::{self, BufRead};

use anyhow::{Context, Result};
use clap::{Parser, Subcommand};
use openintent_adapters::Adapter;
use tracing::{error, info};
use tracing_subscriber::EnvFilter;

// ---------------------------------------------------------------------------
// CLI definition
// ---------------------------------------------------------------------------

/// OpenIntentOS — an AI-powered operating system.
#[derive(Parser)]
#[command(
    name = "openintent",
    version,
    about = "OpenIntentOS — AI-powered operating system",
    long_about = "An AI operating system that understands your intents and executes tasks \
                  using available tools and adapters."
)]
struct Cli {
    #[command(subcommand)]
    command: Commands,
}

#[derive(Subcommand)]
enum Commands {
    /// Start the OpenIntentOS runtime and enter the REPL.
    Run,

    /// Run the interactive setup wizard.
    Setup,

    /// Show current system status.
    Status,
}

// ---------------------------------------------------------------------------
// Main
// ---------------------------------------------------------------------------

#[tokio::main]
async fn main() -> Result<()> {
    let cli = Cli::parse();

    match cli.command {
        Commands::Run => cmd_run().await,
        Commands::Setup => cmd_setup().await,
        Commands::Status => cmd_status().await,
    }
}

// ---------------------------------------------------------------------------
// Subcommand: run
// ---------------------------------------------------------------------------

async fn cmd_run() -> Result<()> {
    // 1. Initialize tracing subscriber.
    init_tracing("info");

    info!("starting OpenIntentOS");

    // 2. Load config.
    // TODO: Load config/default.toml and merge with environment overrides.
    // For now we use hardcoded defaults.
    info!("configuration loaded (using defaults)");

    // 3. Initialize the store (SQLite).
    let data_dir = std::path::Path::new("data");
    if !data_dir.exists() {
        std::fs::create_dir_all(data_dir)
            .context("failed to create data directory")?;
    }

    let db_path = data_dir.join("openintent.db");
    let _db = openintent_store::Database::open_and_migrate(db_path.clone())
        .await
        .context("failed to open database")?;
    info!(path = %db_path.display(), "store initialized");

    // 4. Initialize the intent parser.
    let parser = openintent_intent::IntentParser::new(0.6);
    info!("intent parser ready");

    // 5. Initialize adapters.
    let mut fs_adapter =
        openintent_adapters::FilesystemAdapter::new("filesystem", std::env::current_dir()?);
    fs_adapter.connect().await?;

    let mut shell_adapter =
        openintent_adapters::ShellAdapter::new("shell", std::env::current_dir()?);
    shell_adapter.connect().await?;

    info!("adapters initialized (filesystem, shell)");

    // 6. Enter the REPL loop.
    println!();
    println!("  OpenIntentOS v{}", env!("CARGO_PKG_VERSION"));
    println!("  Type your intent, or 'quit' to exit.");
    println!();

    let stdin = io::stdin();
    let reader = stdin.lock();

    for line in reader.lines() {
        let line = line.context("failed to read input")?;
        let trimmed = line.trim();

        if trimmed.is_empty() {
            continue;
        }

        if trimmed == "quit" || trimmed == "exit" {
            info!("user requested exit");
            break;
        }

        // Parse the intent.
        match parser.parse(trimmed).await {
            Ok(intent) => {
                info!(
                    action = %intent.action,
                    confidence = intent.confidence,
                    source = ?intent.source,
                    "parsed intent"
                );

                // Dispatch to the appropriate adapter.
                let result = match intent.action.as_str() {
                    "fs_read_file" | "fs_write_file" | "fs_list_directory"
                    | "fs_create_directory" | "fs_delete" | "fs_file_info" => {
                        let params = serde_json::to_value(&intent.entities)
                            .unwrap_or(serde_json::Value::Null);
                        openintent_adapters::Adapter::execute_tool(
                            &fs_adapter,
                            &intent.action,
                            params,
                        )
                        .await
                    }
                    "shell_execute" => {
                        let params = serde_json::to_value(&intent.entities)
                            .unwrap_or(serde_json::Value::Null);
                        openintent_adapters::Adapter::execute_tool(
                            &shell_adapter,
                            "shell_execute",
                            params,
                        )
                        .await
                    }
                    "help" => {
                        println!();
                        println!("  Available commands:");
                        println!("    read <path>       - Read a file");
                        println!("    write <path>      - Write to a file");
                        println!("    ls [path]         - List directory contents");
                        println!("    run <command>     - Execute a shell command");
                        println!("    delete <path>     - Delete a file or directory");
                        println!("    status            - Show system status");
                        println!("    help              - Show this help");
                        println!("    quit / exit       - Exit OpenIntentOS");
                        println!();
                        continue;
                    }
                    "system_status" => {
                        let fs_health = fs_adapter.health_check().await?;
                        let shell_health = shell_adapter.health_check().await?;
                        println!();
                        println!("  System Status:");
                        println!("    Filesystem adapter: {fs_health}");
                        println!("    Shell adapter:      {shell_health}");
                        println!("    Database:           connected");
                        println!();
                        continue;
                    }
                    _ => {
                        println!(
                            "  I understood your intent as '{}' (confidence: {:.0}%), \
                             but I don't have a handler for it yet.",
                            intent.action,
                            intent.confidence * 100.0,
                        );
                        continue;
                    }
                };

                match result {
                    Ok(output) => {
                        let formatted =
                            serde_json::to_string_pretty(&output).unwrap_or_default();
                        println!("{formatted}");
                    }
                    Err(e) => {
                        error!(error = %e, "tool execution failed");
                        println!("  Error: {e}");
                    }
                }
            }
            Err(e) => {
                error!(error = %e, "intent parsing failed");
                println!(
                    "  I couldn't understand that. Try 'help' for available commands."
                );
            }
        }
    }

    // Cleanup.
    info!("shutting down");
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
    let data_dir = std::path::Path::new("data");
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
    let data_dir = std::path::Path::new("data");
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
    let config_path = std::path::Path::new("config/default.toml");
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
    let filter = EnvFilter::try_from_default_env()
        .unwrap_or_else(|_| EnvFilter::new(default_level));

    tracing_subscriber::fmt()
        .with_env_filter(filter)
        .with_target(false)
        .compact()
        .init();
}
