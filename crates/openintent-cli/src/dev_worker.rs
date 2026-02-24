//! Background worker that processes self-development tasks.
//!
//! The DevWorker runs as a background tokio task, polling for pending dev tasks
//! and processing them through a pipeline: branch creation, agent-driven code
//! writing, testing, and pull request creation. Progress updates are reported
//! via an optional callback (e.g., to send Telegram messages).

use std::path::PathBuf;
use std::sync::Arc;

use anyhow::{Context, Result};
use tracing::{error, info, warn};

use openintent_agent::runtime::ToolAdapter;
use openintent_agent::{AgentConfig, AgentContext, LlmClient, react_loop};
use openintent_store::DevTaskStore;

use crate::intent_classifier::{TaskKind, classify_intent};

// ═══════════════════════════════════════════════════════════════════════
//  Types
// ═══════════════════════════════════════════════════════════════════════

/// Callback for sending progress updates (to Telegram, CLI, etc.).
///
/// Takes the chat_id and a message string, returns a boxed future.
pub type ProgressCallback =
    Arc<dyn Fn(i64, &str) -> futures::future::BoxFuture<'static, ()> + Send + Sync>;

/// Background worker that processes self-development tasks.
pub struct DevWorker {
    task_store: DevTaskStore,
    llm: Arc<LlmClient>,
    adapters: Vec<Arc<dyn ToolAdapter>>,
    model: String,
    repo_path: PathBuf,
    progress_cb: Option<ProgressCallback>,
}

// ═══════════════════════════════════════════════════════════════════════
//  Implementation
// ═══════════════════════════════════════════════════════════════════════

impl DevWorker {
    /// Create a new DevWorker.
    pub fn new(
        task_store: DevTaskStore,
        llm: Arc<LlmClient>,
        adapters: Vec<Arc<dyn ToolAdapter>>,
        model: String,
        repo_path: PathBuf,
    ) -> Self {
        Self {
            task_store,
            llm,
            adapters,
            model,
            repo_path,
            progress_cb: None,
        }
    }

    /// Set callback for progress updates (e.g., send to Telegram).
    pub fn with_progress_callback(mut self, cb: ProgressCallback) -> Self {
        self.progress_cb = Some(cb);
        self
    }

    /// Start the worker. Recovers incomplete tasks, then polls for pending.
    pub async fn run(&self) {
        info!("DevWorker starting, checking for recoverable tasks");

        // Recover tasks that were in progress when the server stopped.
        match self.task_store.list_recoverable().await {
            Ok(tasks) => {
                if !tasks.is_empty() {
                    info!(count = tasks.len(), "recovering in-progress tasks");
                }
                for task in tasks {
                    let task_id = task.id.clone();
                    info!(task_id = %task_id, status = %task.status, "recovering task");
                    if let Err(e) = self.process_task(&task_id).await {
                        error!(task_id = %task_id, error = %e, "failed to recover task");
                        let _ = self.task_store.set_error(&task_id, &e.to_string()).await;
                        let _ = self
                            .task_store
                            .update_status(&task_id, "failed", Some("Recovery failed"))
                            .await;
                    }
                }
            }
            Err(e) => {
                error!(error = %e, "failed to list recoverable tasks");
            }
        }

        // Poll for pending tasks.
        info!("DevWorker entering poll loop");
        loop {
            match self.task_store.list_by_status("pending", 1, 0).await {
                Ok(tasks) => {
                    if let Some(task) = tasks.into_iter().next() {
                        let task_id = task.id.clone();
                        info!(task_id = %task_id, intent = %task.intent, "processing pending task");
                        if let Err(e) = self.process_task(&task_id).await {
                            error!(task_id = %task_id, error = %e, "task processing failed");
                            let _ = self.task_store.set_error(&task_id, &e.to_string()).await;
                            let _ = self
                                .task_store
                                .update_status(&task_id, "failed", Some("Processing failed"))
                                .await;
                            if let Some(chat_id) = task.chat_id {
                                self.report_progress(chat_id, &format!("Task failed: {e}"))
                                    .await;
                            }
                        }
                    }
                }
                Err(e) => {
                    warn!(error = %e, "failed to poll for pending tasks");
                }
            }

            tokio::time::sleep(std::time::Duration::from_secs(5)).await;
        }
    }

    /// Process a single dev task, routing to simple or full pipeline.
    async fn process_task(&self, task_id: &str) -> Result<()> {
        let task = self
            .task_store
            .get(task_id)
            .await
            .context("failed to fetch task")?
            .ok_or_else(|| anyhow::anyhow!("task not found: {task_id}"))?;

        let intent = task.intent.clone();
        let chat_id = task.chat_id;

        match classify_intent(&self.llm, &self.model, &intent).await {
            TaskKind::Simple => {
                info!(task_id = %task_id, "classified as simple operation");
                return self.process_simple_task(task_id, &intent, chat_id).await;
            }
            TaskKind::Development => {
                info!(task_id = %task_id, "classified as development task");
            }
        }

        let max_retries = task.max_retries;

        // Step 1: Create branch (skip if already past branching).
        let branch = if task.branch.is_some() && task.status != "branching" {
            task.branch.clone().unwrap_or_default()
        } else {
            self.task_store
                .update_status(task_id, "branching", Some("Creating feature branch"))
                .await
                .context("failed to update status to branching")?;
            if let Some(cid) = chat_id {
                self.report_progress(cid, &format!("Creating feature branch for: {intent}"))
                    .await;
            }

            let branch = self
                .step_create_branch(task_id, &intent)
                .await
                .context("failed to create branch")?;

            self.task_store
                .set_branch(task_id, &branch)
                .await
                .context("failed to set branch")?;
            self.task_store
                .append_progress(task_id, &format!("Branch created: {branch}"))
                .await
                .context("failed to append progress")?;

            branch
        };

        // Step 2 + 3: Write code and test (with retries).
        // `last_failure` carries test/clippy error output into the next retry so
        // the agent knows exactly what went wrong and can apply a targeted fix.
        let mut last_failure: Option<String> = None;

        let agent_summary = loop {
            // Step 2: Write code.
            let status_msg = if last_failure.is_some() {
                "Agent fixing issues from failed tests"
            } else {
                "Agent analyzing and writing code"
            };
            self.task_store
                .update_status(task_id, "coding", Some(status_msg))
                .await
                .context("failed to update status to coding")?;
            if let Some(cid) = chat_id {
                self.report_progress(cid, &format!("{status_msg}..."))
                    .await;
            }

            // Check for mid-task user messages before running agent.
            let injected = self.check_user_messages(task_id).await;
            let mut extra_context = String::new();
            for msg in &injected {
                extra_context.push_str(&format!("\n\nAdditional instruction from user: {msg}"));
            }

            let full_intent = if extra_context.is_empty() {
                intent.clone()
            } else {
                format!("{intent}{extra_context}")
            };

            let summary = self
                .step_write_code(task_id, &full_intent, &branch, last_failure.as_deref())
                .await
                .context("failed to write code")?;

            self.task_store
                .append_progress(
                    task_id,
                    &format!("Code written: {}", truncate(&summary, 200)),
                )
                .await
                .context("failed to append progress")?;

            // Step 3: Test.
            self.task_store
                .update_status(task_id, "testing", Some("Running cargo test"))
                .await
                .context("failed to update status to testing")?;
            if let Some(cid) = chat_id {
                self.report_progress(cid, "Running tests...").await;
            }

            match self.step_run_tests(task_id).await {
                Ok(()) => {
                    self.task_store
                        .append_progress(task_id, "All tests passed")
                        .await
                        .context("failed to append progress")?;
                    break summary;
                }
                Err(e) => {
                    let err_msg = e.to_string();

                    let retry_count = self
                        .task_store
                        .increment_retry(task_id)
                        .await
                        .context("failed to increment retry")?;

                    self.task_store
                        .set_error(task_id, &err_msg)
                        .await
                        .context("failed to set error")?;

                    if retry_count >= max_retries {
                        self.task_store
                            .update_status(
                                task_id,
                                "failed",
                                Some("Tests failed after max retries"),
                            )
                            .await
                            .context("failed to update status to failed")?;
                        if let Some(cid) = chat_id {
                            self.report_progress(
                                cid,
                                &format!(
                                    "Failed after {max_retries} retries. Error: {}",
                                    truncate(&err_msg, 500)
                                ),
                            )
                            .await;
                        }
                        return Err(anyhow::anyhow!(
                            "tests failed after {max_retries} retries: {err_msg}"
                        ));
                    }

                    if let Some(cid) = chat_id {
                        self.report_progress(
                            cid,
                            &format!(
                                "Tests failed: {}. Retrying... ({retry_count}/{max_retries})",
                                truncate(&err_msg, 200)
                            ),
                        )
                        .await;
                    }

                    self.task_store
                        .append_progress(
                            task_id,
                            &format!(
                                "Retry {retry_count}/{max_retries}: {}",
                                truncate(&err_msg, 200)
                            ),
                        )
                        .await
                        .context("failed to append progress")?;

                    // Store failure context so next iteration gets it.
                    last_failure = Some(err_msg);

                    // Loop back to step 2 — agent will try to fix the issues.
                    continue;
                }
            }
        };

        // Step 4: Create PR.
        self.task_store
            .update_status(task_id, "pr_created", Some("Creating pull request"))
            .await
            .context("failed to update status to pr_created")?;
        if let Some(cid) = chat_id {
            self.report_progress(cid, "Creating pull request...").await;
        }

        let pr_url = self
            .step_create_pr(task_id, &intent, &branch, &agent_summary)
            .await
            .context("failed to create PR")?;

        self.task_store
            .set_pr_url(task_id, &pr_url)
            .await
            .context("failed to set PR URL")?;
        self.task_store
            .update_status(task_id, "awaiting_review", Some("PR ready for review"))
            .await
            .context("failed to update status to awaiting_review")?;
        self.task_store
            .append_progress(task_id, &format!("PR created: {pr_url}"))
            .await
            .context("failed to append progress")?;

        if let Some(cid) = chat_id {
            self.report_progress(
                cid,
                &format!(
                    "PR created: {pr_url}\nReply /merge {task_id} to merge, or /cancel {task_id} to cancel."
                ),
            )
            .await;
        }

        info!(task_id = %task_id, pr_url = %pr_url, "task completed successfully");
        Ok(())
    }

    /// Process a simple operation (git commit, push, format, etc.) without
    /// the full branch/test/PR pipeline.
    async fn process_simple_task(
        &self,
        task_id: &str,
        intent: &str,
        chat_id: Option<i64>,
    ) -> Result<()> {
        self.task_store
            .update_status(task_id, "coding", Some("Executing operation"))
            .await
            .context("failed to update status")?;
        if let Some(cid) = chat_id {
            self.report_progress(cid, &format!("Executing: {intent}"))
                .await;
        }

        let system_prompt = format!(
            "You are a helpful assistant for the OpenIntentOS project (a Rust codebase).\n\
             Repository root: {repo_path}\n\n\
             The user wants you to perform a simple operation. \
             Execute it directly using the available tools. \
             Do NOT create branches or pull requests. \
             Just perform the operation and report the result.\n\n\
             Available tools:\n\
             - `fs_read_file` — Read a file\n\
             - `fs_write_file` — Write a file\n\
             - `fs_list_dir` — List directory contents\n\
             - `shell_execute` — Run shell commands (git, cargo, etc.)\n\
             - `fs_delete_file` — Delete a file\n\
             - `fs_create_dir` — Create a directory",
            repo_path = self.repo_path.display()
        );

        let agent_config = AgentConfig {
            max_turns: 15,
            model: self.model.clone(),
            temperature: Some(0.0),
            max_tokens: Some(4096),
            ..AgentConfig::default()
        };

        let mut ctx = AgentContext::new(self.llm.clone(), self.adapters.clone(), agent_config)
            .with_system_prompt(&system_prompt)
            .with_user_message(intent);

        let response = react_loop(&mut ctx)
            .await
            .map_err(|e| anyhow::anyhow!("agent failed: {e}"))?;

        let summary = response.text.clone();

        self.task_store
            .append_message(task_id, "agent", &summary)
            .await
            .context("failed to append agent message")?;
        self.task_store
            .append_progress(task_id, &truncate(&summary, 200))
            .await
            .context("failed to append progress")?;
        self.task_store
            .update_status(task_id, "completed", Some("Operation completed"))
            .await
            .context("failed to update status to completed")?;

        if let Some(cid) = chat_id {
            self.report_progress(cid, &format!("Done: {}", truncate(&summary, 500)))
                .await;
        }

        info!(task_id = %task_id, "simple task completed");
        Ok(())
    }

    /// Step 1: Create a git feature branch.
    async fn step_create_branch(&self, task_id: &str, intent: &str) -> Result<String> {
        // Generate a short hash from the intent for the branch name.
        let hash = short_hash(intent);
        let branch_name = format!("feat/dev-{hash}");

        info!(task_id = %task_id, branch = %branch_name, "creating feature branch");

        // Checkout main, pull, and create the new branch.
        self.shell_exec("git checkout main")
            .await
            .context("failed to checkout main")?;
        self.shell_exec("git pull origin main")
            .await
            .context("failed to pull origin main")?;
        self.shell_exec(&format!("git checkout -b {branch_name}"))
            .await
            .context("failed to create branch")?;

        self.task_store
            .append_message(task_id, "system", &format!("Created branch: {branch_name}"))
            .await
            .context("failed to append message")?;

        Ok(branch_name)
    }

    /// Step 2: Run the agent to write code.
    ///
    /// If `test_failure` is provided, the agent gets the error context for a retry.
    async fn step_write_code(
        &self,
        task_id: &str,
        intent: &str,
        branch: &str,
        test_failure: Option<&str>,
    ) -> Result<String> {
        info!(task_id = %task_id, branch = %branch, "agent writing code");

        // Make sure we are on the correct branch.
        self.shell_exec(&format!("git checkout {branch}"))
            .await
            .context("failed to checkout branch for coding")?;

        let system_prompt = self.build_dev_system_prompt(intent, branch);

        let agent_config = AgentConfig {
            max_turns: 40,
            model: self.model.clone(),
            temperature: Some(0.0),
            max_tokens: Some(8192),
            ..AgentConfig::default()
        };

        // Build the user message with context.
        let user_message = if let Some(failure) = test_failure {
            format!(
                "{intent}\n\n\
                 IMPORTANT: The previous attempt failed with the following errors. \
                 You MUST fix these issues:\n\
                 ```\n{failure}\n```\n\n\
                 First, review the error output carefully. Then read the relevant files, \
                 understand the root cause, and apply a targeted fix."
            )
        } else {
            format!(
                "{intent}\n\n\
                 WORKFLOW:\n\
                 1. EXPLORE: Use `shell_execute` to run `find crates/ -name '*.rs' | head -50` \
                    and read the most relevant files with `fs_read_file` to understand \
                    existing patterns.\n\
                 2. PLAN: Decide which files to create/modify. State your plan briefly.\n\
                 3. CODE: Make changes file by file. After each file, run `cargo check` \
                    to verify compilation.\n\
                 4. VERIFY: Run `cargo check --workspace` to ensure everything compiles.\n\
                 5. SUMMARIZE: Describe what you changed and why."
            )
        };

        let mut ctx = AgentContext::new(self.llm.clone(), self.adapters.clone(), agent_config)
            .with_system_prompt(&system_prompt)
            .with_user_message(&user_message);

        // Load any previous conversation messages for this task.
        let prev_messages = self
            .task_store
            .get_messages(task_id, Some(10))
            .await
            .unwrap_or_default();
        for msg in &prev_messages {
            match msg.role.as_str() {
                "user" => {
                    ctx.messages
                        .push(openintent_agent::Message::user(&msg.content));
                }
                "agent" | "assistant" => {
                    ctx.messages
                        .push(openintent_agent::Message::assistant(&msg.content));
                }
                _ => {}
            }
        }

        let response = react_loop(&mut ctx)
            .await
            .map_err(|e| anyhow::anyhow!("agent react loop failed: {e}"))?;

        let summary = response.text.clone();

        self.task_store
            .append_message(task_id, "agent", &summary)
            .await
            .context("failed to append agent message")?;

        info!(
            task_id = %task_id,
            turns = response.turns_used,
            "agent completed code writing"
        );

        Ok(summary)
    }

    /// Build a rich system prompt that includes project rules, coding standards,
    /// and architecture context so the agent writes intelligent, project-aware code.
    fn build_dev_system_prompt(&self, intent: &str, branch: &str) -> String {
        // Try to load CLAUDE.md for project rules.
        let claude_md = std::fs::read_to_string(self.repo_path.join("CLAUDE.md"))
            .unwrap_or_default();
        let project_rules = if claude_md.is_empty() {
            String::new()
        } else {
            format!(
                "\n## Project Rules (from CLAUDE.md)\n\n\
                 {}\n",
                truncate(&claude_md, 3000)
            )
        };

        format!(
            "You are a senior Rust developer working on OpenIntentOS — an AI-native \
             micro operating system built entirely in Rust.\n\
             \n\
             ## Your Task\n\
             {intent}\n\
             Working on branch: {branch}\n\
             Repository root: {repo_path}\n\
             {project_rules}\
             \n\
             ## Critical Rules\n\
             - **ALWAYS read existing files before modifying them.** Understand the current \
               code before making changes.\n\
             - **No hardcoded multilingual content.** All user-facing strings must come from \
               config files, never hardcoded in source.\n\
             - **Maximum 1000 lines per file.** If a file exceeds this, split into modules.\n\
             - Use `thiserror` for library error types, `anyhow` only in CLI binary.\n\
             - Use `tracing` for all logging — never `println!` in library code.\n\
             - Everything must be `Send + Sync`. Use `Arc<T>` over `Rc<T>`.\n\
             - Never `unwrap()` or `expect()` in library code — only in tests.\n\
             - Use `tokio::task::spawn_blocking` for CPU-heavy or synchronous work.\n\
             - Run `cargo check` after modifying files to catch compilation errors early.\n\
             - Keep changes minimal and focused. Don't refactor unrelated code.\n\
             \n\
             ## Project Structure\n\
             ```\n\
             crates/\n\
               openintent-kernel/   -- Micro-kernel (IPC, scheduler, intent router)\n\
               openintent-agent/    -- Agent runtime (ReAct loop, LLM client, planner)\n\
               openintent-store/    -- Storage engine (SQLite WAL, sessions, 3-layer memory)\n\
               openintent-adapters/ -- Tool adapters (filesystem, shell, GitHub, web, email)\n\
               openintent-cli/      -- CLI binary (bot, REPL, web server, dev worker)\n\
               openintent-skills/   -- OpenClaw-compatible skill system\n\
               openintent-vault/    -- Encrypted credential store (AES-256-GCM)\n\
               openintent-intent/   -- Intent parsing & workflow engine\n\
               openintent-web/      -- Web server (axum)\n\
               openintent-tui/      -- Terminal UI (ratatui)\n\
             config/\n\
               default.toml         -- Runtime configuration\n\
               IDENTITY.md          -- System persona\n\
               SOUL.md              -- Behavioral guidelines\n\
             ```\n\
             \n\
             ## Available Tools\n\
             - `fs_read_file` — Read a file (use to understand existing code)\n\
             - `fs_write_file` — Write/create a file\n\
             - `fs_list_dir` — List directory contents\n\
             - `shell_execute` — Run shell commands (cargo check, cargo test, git, etc.)\n\
             - `fs_delete_file` — Delete a file\n\
             - `fs_create_dir` — Create a directory\n\
             \n\
             IMPORTANT: Start by exploring the relevant code. Read the files you plan to \
             modify. Understand the patterns. Then write code that fits naturally into the \
             existing codebase.",
            repo_path = self.repo_path.display()
        )
    }

    /// Step 3: Run cargo fmt + clippy + test.
    ///
    /// Returns `Ok(())` if all checks pass. Returns `Err` with a detailed error
    /// message (including compiler/test output) if any step fails. This error
    /// message is fed back to the agent on retry so it can fix the issues.
    async fn step_run_tests(&self, task_id: &str) -> Result<()> {
        info!(task_id = %task_id, "running tests");

        // Run cargo fmt.
        if let Err(e) = self.shell_exec("cargo fmt --all").await {
            let err_msg = format!("cargo fmt failed:\n{e}");
            warn!(task_id = %task_id, error = %e, "cargo fmt failed");
            self.task_store
                .append_message(task_id, "system", &err_msg)
                .await
                .context("failed to append message")?;
            anyhow::bail!("{err_msg}");
        }

        // Run cargo clippy.
        if let Err(e) = self
            .shell_exec("cargo clippy --workspace -- -D warnings")
            .await
        {
            let err_msg = format!("cargo clippy failed:\n{e}");
            warn!(task_id = %task_id, error = %e, "cargo clippy failed");
            self.task_store
                .append_message(task_id, "system", &err_msg)
                .await
                .context("failed to append message")?;
            anyhow::bail!("{err_msg}");
        }

        // Run cargo test.
        if let Err(e) = self.shell_exec("cargo test --workspace").await {
            let err_msg = format!("cargo test failed:\n{e}");
            warn!(task_id = %task_id, error = %e, "cargo test failed");
            self.task_store
                .append_message(task_id, "system", &err_msg)
                .await
                .context("failed to append message")?;
            anyhow::bail!("{err_msg}");
        }

        self.task_store
            .append_message(task_id, "system", "All checks passed: fmt, clippy, test")
            .await
            .context("failed to append message")?;

        Ok(())
    }

    /// Step 4: Commit, push, and create PR.
    async fn step_create_pr(
        &self,
        task_id: &str,
        intent: &str,
        branch: &str,
        agent_summary: &str,
    ) -> Result<String> {
        info!(task_id = %task_id, branch = %branch, "creating pull request");

        // Stage all changes and commit.
        let commit_msg = format!("feat: {}", truncate(intent, 60));
        self.shell_exec("git add -A")
            .await
            .context("failed to git add")?;

        // Check if there are staged changes to commit.
        let diff_result = self.shell_exec("git diff --cached --stat").await;
        let has_changes = match &diff_result {
            Ok(output) => !output.trim().is_empty(),
            Err(_) => false,
        };

        if has_changes {
            self.shell_exec(&format!("git commit -m '{commit_msg}'"))
                .await
                .context("failed to git commit")?;
        }

        // Push the branch.
        self.shell_exec(&format!("git push -u origin {branch}"))
            .await
            .context("failed to push branch")?;

        // Determine the GitHub owner and repo from the remote URL.
        let (owner, repo) = self.resolve_github_remote().await?;

        // Create the PR via the GitHub adapter tool.
        let pr_body = format!(
            "## Summary\n\n{}\n\n## Intent\n\n{}\n\n## Task ID\n\n`{}`",
            truncate(agent_summary, 1000),
            intent,
            task_id
        );

        let pr_url = self
            .create_pr_via_adapter(&owner, &repo, &commit_msg, &pr_body, branch)
            .await?;

        self.task_store
            .append_message(task_id, "system", &format!("PR created: {pr_url}"))
            .await
            .context("failed to append message")?;

        Ok(pr_url)
    }

    /// Create a PR using the GitHub adapter tool.
    async fn create_pr_via_adapter(
        &self,
        owner: &str,
        repo: &str,
        title: &str,
        body: &str,
        head: &str,
    ) -> Result<String> {
        // Find the GitHub adapter among our adapters.
        let github_adapter = self
            .adapters
            .iter()
            .find(|a| a.adapter_id() == "github")
            .ok_or_else(|| anyhow::anyhow!("GitHub adapter not found"))?;

        let params = serde_json::json!({
            "owner": owner,
            "repo": repo,
            "title": title,
            "body": body,
            "head": head,
            "base": "main",
        });

        let result = github_adapter
            .execute("github_create_pull_request", params)
            .await
            .map_err(|e| anyhow::anyhow!("failed to create PR via GitHub adapter: {e}"))?;

        // Parse the result to extract the PR URL.
        let pr_data: serde_json::Value = serde_json::from_str(&result)
            .unwrap_or_else(|_| serde_json::json!({"html_url": result}));

        let pr_url = pr_data
            .get("html_url")
            .and_then(|v| v.as_str())
            .unwrap_or_else(|| {
                pr_data
                    .get("url")
                    .and_then(|v| v.as_str())
                    .unwrap_or("unknown")
            })
            .to_string();

        Ok(pr_url)
    }

    /// Send progress update via callback.
    async fn report_progress(&self, chat_id: i64, message: &str) {
        info!(chat_id, message, "progress update");
        if let Some(ref cb) = self.progress_cb {
            cb(chat_id, message).await;
        }
    }

    /// Execute a shell command and return stdout.
    async fn shell_exec(&self, command: &str) -> Result<String> {
        let output = tokio::process::Command::new("sh")
            .arg("-c")
            .arg(command)
            .current_dir(&self.repo_path)
            .output()
            .await
            .context("failed to spawn shell command")?;

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            let stdout = String::from_utf8_lossy(&output.stdout);
            anyhow::bail!(
                "command `{command}` failed (exit {}): {stderr}{stdout}",
                output.status.code().unwrap_or(-1)
            );
        }

        Ok(String::from_utf8_lossy(&output.stdout).to_string())
    }

    /// Check for mid-task user messages and return them.
    async fn check_user_messages(&self, task_id: &str) -> Vec<String> {
        let messages = self
            .task_store
            .get_messages(task_id, Some(20))
            .await
            .unwrap_or_default();

        // Find user messages that haven't been processed yet
        // (messages after the last agent or system message).
        let last_non_user_idx = messages.iter().rposition(|m| m.role != "user");

        let start = match last_non_user_idx {
            Some(idx) => idx + 1,
            None => 0,
        };

        messages[start..]
            .iter()
            .filter(|m| m.role == "user")
            .map(|m| m.content.clone())
            .collect()
    }

    /// Resolve the GitHub owner and repo from the git remote URL.
    async fn resolve_github_remote(&self) -> Result<(String, String)> {
        let remote_url = self
            .shell_exec("git remote get-url origin")
            .await
            .context("failed to get git remote URL")?;

        let remote = remote_url.trim();

        // Handle SSH format: git@github.com:owner/repo.git
        if let Some(path) = remote.strip_prefix("git@github.com:") {
            let path = path.strip_suffix(".git").unwrap_or(path);
            let parts: Vec<&str> = path.splitn(2, '/').collect();
            if parts.len() == 2 {
                return Ok((parts[0].to_string(), parts[1].to_string()));
            }
        }

        // Handle HTTPS format: https://github.com/owner/repo.git
        if remote.contains("github.com") {
            let url = url::Url::parse(remote).context("failed to parse remote URL")?;
            let segments: Vec<&str> = url.path_segments().map(|s| s.collect()).unwrap_or_default();
            if segments.len() >= 2 {
                let repo = segments[1].strip_suffix(".git").unwrap_or(segments[1]);
                return Ok((segments[0].to_string(), repo.to_string()));
            }
        }

        anyhow::bail!("could not parse GitHub owner/repo from remote: {remote}")
    }
}

// ═══════════════════════════════════════════════════════════════════════
//  Utility functions
// ═══════════════════════════════════════════════════════════════════════

/// Generate a short 8-character hash from a string for branch naming.
fn short_hash(input: &str) -> String {
    use std::collections::hash_map::DefaultHasher;
    use std::hash::{Hash, Hasher};

    let mut hasher = DefaultHasher::new();
    input.hash(&mut hasher);
    let hash = hasher.finish();
    format!("{hash:016x}")[..8].to_string()
}

/// Truncate a string to approximately `max_len` bytes, respecting UTF-8
/// char boundaries. Appends "..." if truncated.
fn truncate(s: &str, max_len: usize) -> String {
    if s.len() <= max_len {
        return s.to_string();
    }

    // Find the last char boundary at or before max_len.
    let mut end = max_len;
    while end > 0 && !s.is_char_boundary(end) {
        end -= 1;
    }

    let mut result = s[..end].to_string();
    result.push_str("...");
    result
}

// ═══════════════════════════════════════════════════════════════════════
//  Tests
// ═══════════════════════════════════════════════════════════════════════

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn short_hash_is_deterministic() {
        let h1 = short_hash("add dark mode support");
        let h2 = short_hash("add dark mode support");
        assert_eq!(h1, h2);
        assert_eq!(h1.len(), 8);
    }

    #[test]
    fn short_hash_differs_for_different_inputs() {
        let h1 = short_hash("add dark mode");
        let h2 = short_hash("fix login bug");
        assert_ne!(h1, h2);
    }

    #[test]
    fn truncate_short_string() {
        assert_eq!(truncate("hello", 10), "hello");
    }

    #[test]
    fn truncate_long_string() {
        let long = "a".repeat(100);
        let result = truncate(&long, 10);
        assert_eq!(result.len(), 13); // 10 + "..."
        assert!(result.ends_with("..."));
    }

    #[test]
    fn truncate_exact_length() {
        let s = "exactly10!";
        assert_eq!(truncate(s, 10), "exactly10!");
    }

    #[test]
    fn truncate_multibyte_utf8() {
        // Chinese chars are 3 bytes each. "代码提交" = 12 bytes.
        let s = "代码提交到git仓库";
        // Truncate at 10 bytes -- falls inside '提' (bytes 6..9),
        // so it should back up to byte 6.
        let result = truncate(s, 10);
        assert!(result.ends_with("..."));
        // Should not panic and should contain valid UTF-8.
        assert!(result.starts_with("代码"));
    }
}
