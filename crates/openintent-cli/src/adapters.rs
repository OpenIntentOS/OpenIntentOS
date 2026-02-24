//! Shared adapter initialization.
//!
//! Multiple subcommands (run, bot, tui, serve) need the same set of adapters.
//! This module provides a single function to initialize them all, eliminating
//! code duplication.

use std::path::PathBuf;
use std::sync::Arc;

use anyhow::Result;
use openintent_adapters::Adapter;
use openintent_agent::runtime::ToolAdapter;
use openintent_store::Database;

use crate::bridge::AdapterBridge;

/// The result of initializing all adapters.
pub struct InitializedAdapters {
    /// Agent-side tool adapters (wrapped in `AdapterBridge`).
    pub tool_adapters: Vec<Arc<dyn ToolAdapter>>,

    /// Adapter-side adapters (for the web server which needs `Adapter` directly).
    pub raw_adapters: Vec<Arc<dyn openintent_adapters::Adapter>>,

    /// Number of skill-based script tools loaded.
    pub skill_count: usize,

    /// Prompt extension text from loaded skills.
    pub skill_prompt_ext: String,
}

/// Initialize and connect all adapters.
///
/// This is the single source of truth for adapter setup. All subcommands
/// that need adapters should call this function.
pub async fn init_adapters(
    cwd: PathBuf,
    db: Database,
    include_telegram_discord: bool,
) -> Result<InitializedAdapters> {
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

    let memory = Arc::new(openintent_store::SemanticMemory::new(db));
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

    // Build raw adapter list (for web server).
    let mut raw_adapters: Vec<Arc<dyn openintent_adapters::Adapter>> = vec![
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

    // Optionally add Telegram and Discord adapters.
    if include_telegram_discord {
        let mut telegram_adapter = openintent_adapters::TelegramAdapter::new("telegram");
        telegram_adapter.connect().await?;

        let mut discord_adapter = openintent_adapters::DiscordAdapter::new("discord");
        discord_adapter.connect().await?;

        raw_adapters.push(Arc::new(telegram_adapter));
        raw_adapters.push(Arc::new(discord_adapter));
    }

    // Load skills.
    let skills_dir = openintent_skills::default_skills_dir();
    let mut skill_manager = openintent_skills::SkillManager::new(skills_dir);
    let _ = skill_manager.load_all();
    let skill_count = skill_manager.skills().len();
    let skill_prompt_ext = skill_manager.build_prompt_extension();

    let mut skill_adapter = openintent_skills::SkillAdapter::new("skills", skill_manager.skills());
    skill_adapter.connect().await?;
    let skill_tool_count = skill_adapter.tools().len();

    if skill_count > 0 {
        tracing::info!(
            skills = skill_count,
            script_tools = skill_tool_count,
            "skills loaded"
        );
    }

    // Wrap raw adapters in the bridge for the agent side.
    let mut tool_adapters: Vec<Arc<dyn ToolAdapter>> = raw_adapters
        .iter()
        .map(|a| -> Arc<dyn ToolAdapter> {
            Arc::new(AdapterBridge::new(RawAdapterRef(Arc::clone(a))))
        })
        .collect();

    // Add skill adapter if it has any script tools.
    if skill_tool_count > 0 {
        tool_adapters.push(Arc::new(AdapterBridge::new(skill_adapter)));
    }

    Ok(InitializedAdapters {
        tool_adapters,
        raw_adapters,
        skill_count,
        skill_prompt_ext,
    })
}

/// Wrapper that implements `Adapter` by delegating to an `Arc<dyn Adapter>`.
///
/// Needed so we can create `AdapterBridge` from already-`Arc`'d adapters without
/// double-wrapping.
struct RawAdapterRef(Arc<dyn openintent_adapters::Adapter>);

#[async_trait::async_trait]
impl openintent_adapters::Adapter for RawAdapterRef {
    fn id(&self) -> &str {
        self.0.id()
    }

    fn adapter_type(&self) -> openintent_adapters::AdapterType {
        self.0.adapter_type()
    }

    fn tools(&self) -> Vec<openintent_adapters::ToolDefinition> {
        self.0.tools()
    }

    async fn connect(&mut self) -> openintent_adapters::Result<()> {
        // Already connected.
        Ok(())
    }

    async fn disconnect(&mut self) -> openintent_adapters::Result<()> {
        Ok(())
    }

    async fn health_check(&self) -> openintent_adapters::Result<openintent_adapters::HealthStatus> {
        self.0.health_check().await
    }

    async fn execute_tool(
        &self,
        tool_name: &str,
        arguments: serde_json::Value,
    ) -> openintent_adapters::Result<serde_json::Value> {
        self.0.execute_tool(tool_name, arguments).await
    }

    fn required_auth(&self) -> Option<openintent_adapters::AuthRequirement> {
        self.0.required_auth()
    }
}
