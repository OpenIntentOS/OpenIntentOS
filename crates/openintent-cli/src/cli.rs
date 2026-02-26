//! CLI argument definitions for OpenIntentOS.
//!
//! All `clap` structures live here so that `main.rs` stays focused on
//! dispatching subcommands.

use clap::{Parser, Subcommand};

/// OpenIntentOS -- an AI-powered operating system.
#[derive(Parser)]
#[command(
    name = "openintent",
    version,
    about = "OpenIntentOS -- AI-powered operating system",
    long_about = "An AI operating system that understands your intents and executes tasks \
                  using available tools and adapters."
)]
pub struct Cli {
    #[command(subcommand)]
    pub command: Commands,
}

#[derive(Subcommand)]
pub enum Commands {
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
        #[arg(long, short, default_value_t = 23517)]
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

    /// Manage skills (OpenClaw-compatible SKILL.md).
    Skills {
        #[command(subcommand)]
        action: SkillAction,
    },

    /// Start the Telegram bot gateway (receive messages from Telegram, run the
    /// agent, send responses back).
    Bot {
        /// Telegram long-polling timeout in seconds.
        #[arg(long, default_value_t = 30)]
        poll_timeout: u64,

        /// Restrict the bot to specific Telegram user IDs (comma-separated).
        /// If omitted, all users are allowed.
        #[arg(long)]
        allowed_users: Option<String>,
    },

    /// Check for updates or update the binary to the latest release.
    Update {
        /// Only check whether an update is available; do not download.
        #[arg(long, short)]
        check: bool,
    },
}

/// Actions for managing conversation sessions.
#[derive(Subcommand)]
pub enum SessionAction {
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
pub enum UserAction {
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

/// Actions for managing skills.
#[derive(Subcommand)]
pub enum SkillAction {
    /// List installed skills.
    List,
    /// Install a skill from ClawHub registry or URL.
    Install {
        /// Skill slug (from ClawHub) or URL (github:owner/repo, or full URL).
        source: String,
    },
    /// Remove an installed skill.
    Remove {
        /// The skill name to remove.
        name: String,
    },
    /// Search the ClawHub registry for skills.
    Search {
        /// Search query.
        query: String,
        /// Maximum number of results.
        #[arg(long, short, default_value_t = 10)]
        limit: usize,
    },
    /// Show details of an installed skill.
    Info {
        /// Skill name.
        name: String,
    },
}
