//! Self-repair pipeline for OpenIntentOS.
//!
//! When the bot encounters a code bug (panic, internal error), this module:
//! 1. Analyzes the error and reads recent logs
//! 2. Spawns a repair agent that reads source, fixes the bug
//! 3. Runs `cargo check` -> `cargo test` -> `cargo build --release`
//! 4. Commits the fix to git and pushes to remote
//! 5. Restarts the process with the new binary
//! 6. Notifies the user that the fix is deployed

use std::collections::HashMap;
use std::path::Path;
use std::sync::Arc;

use tracing::{error, info, warn};

use openintent_agent::runtime::ToolAdapter;
use openintent_agent::{AgentConfig, AgentContext, LlmClient, react_loop};

use crate::messages::{Messages, keys};

// ---------------------------------------------------------------------------
// Public API
// ---------------------------------------------------------------------------

/// Outcome of a self-repair attempt.
pub enum RepairOutcome {
    /// The bug was fixed, committed, and a new binary was built.
    /// The caller should restart the process.
    Fixed {
        commit_hash: String,
        summary: String,
    },
    /// The error is not a code bug (transient, external, etc.).
    NotACodeBug,
    /// Self-repair was attempted but failed.
    Failed { reason: String },
}

/// Telegram message sender with language-aware translation.
pub struct TelegramNotifier {
    http: reqwest::Client,
    api_base: String,
    chat_id: i64,
    user_lang: String,
    messages: Messages,
    llm: Arc<LlmClient>,
    model: String,
}

impl TelegramNotifier {
    pub fn new(
        http: reqwest::Client,
        api_base: String,
        chat_id: i64,
        user_lang: String,
        messages: Messages,
        llm: Arc<LlmClient>,
        model: String,
    ) -> Self {
        Self {
            http,
            api_base,
            chat_id,
            user_lang,
            messages,
            llm,
            model,
        }
    }

    /// Send a raw text message to the chat.
    pub async fn send_raw(&self, text: &str) {
        let _ = self
            .http
            .post(format!("{}/sendMessage", self.api_base))
            .json(&serde_json::json!({
                "chat_id": self.chat_id,
                "text": text,
            }))
            .send()
            .await;
    }

    /// Send a message by key, auto-translated to the user's language.
    pub async fn send_msg(&self, key: &str) {
        let text = self
            .messages
            .get_translated(key, &[], &self.user_lang, &self.llm, &self.model)
            .await;
        self.send_raw(&text).await;
    }

    /// Send a message by key with placeholder substitution, auto-translated.
    pub async fn send_msg_with(&self, key: &str, vars: &[(&str, &str)]) {
        let text = self
            .messages
            .get_translated(key, vars, &self.user_lang, &self.llm, &self.model)
            .await;
        self.send_raw(&text).await;
    }

    pub async fn send_typing(&self) {
        let _ = self
            .http
            .post(format!("{}/sendChatAction", self.api_base))
            .json(&serde_json::json!({
                "chat_id": self.chat_id,
                "action": "typing",
            }))
            .send()
            .await;
    }
}

/// Attempt to self-repair after an agent error.
///
/// Only tries to repair errors that look like code bugs (panics, internal
/// errors). Transient errors (network, LLM, rate limits) are skipped.
pub async fn attempt_repair(
    error: &openintent_agent::error::AgentError,
    user_message: &str,
    notifier: &TelegramNotifier,
    llm: &Arc<LlmClient>,
    adapters: &[Arc<dyn ToolAdapter>],
    model: &str,
    repo_path: &Path,
) -> RepairOutcome {
    // Step 0: Decide if this error is worth self-repairing.
    if !is_code_bug(error) {
        return RepairOutcome::NotACodeBug;
    }

    let error_text = error.to_string();
    info!(error = %error_text, "self-repair triggered");

    notifier.send_msg(keys::REPAIR_STARTED).await;

    // Pre-translate tool progress messages for the callback (which can't await).
    let progress_msgs = notifier
        .messages
        .batch_translate(
            &[
                keys::REPAIR_ANALYZING,
                keys::REPAIR_FIXING,
                keys::REPAIR_COMPILING,
            ],
            &notifier.user_lang,
            &notifier.llm,
            &notifier.model,
        )
        .await;

    // Step 1: Gather diagnostic context.
    let log_tail = read_log_tail("/tmp/openintent-bot.log", 80);
    let recent_commits = read_recent_commits(repo_path, 10);

    // Step 2: Run the repair agent.
    let repair_prompt = build_repair_system_prompt(repo_path);
    let user_prompt = build_repair_user_prompt(
        &error_text,
        user_message,
        &log_tail,
        &recent_commits,
    );

    notifier.send_typing().await;

    let agent_config = AgentConfig {
        max_turns: 30,
        model: model.to_owned(),
        temperature: Some(0.0),
        max_tokens: Some(8192),
        ..AgentConfig::default()
    };

    let mut ctx = AgentContext::new(llm.clone(), adapters.to_vec(), agent_config)
        .with_system_prompt(&repair_prompt)
        .with_user_message(&user_prompt);

    // Progress callback — notify user during repair using pre-translated messages.
    let notifier_http = notifier.http.clone();
    let notifier_api = notifier.api_base.clone();
    let notifier_chat = notifier.chat_id;
    let sent: Arc<std::sync::Mutex<std::collections::HashSet<String>>> =
        Arc::new(std::sync::Mutex::new(std::collections::HashSet::new()));
    let progress_map: Arc<HashMap<String, String>> = Arc::new(progress_msgs);

    ctx.on_tool_start = Some(Arc::new(move |tool_name: &str, _args: &serde_json::Value| {
        let key = match tool_name {
            "fs_read_file" | "fs_list_directory" => Some(keys::REPAIR_ANALYZING),
            "fs_str_replace" | "fs_write_file" => Some(keys::REPAIR_FIXING),
            "shell_execute" => Some(keys::REPAIR_COMPILING),
            _ => None,
        };
        if let Some(msg_key) = key {
            let msg = progress_map
                .get(msg_key)
                .cloned()
                .unwrap_or_else(|| msg_key.to_string());
            let already = {
                let mut set = sent.lock().unwrap_or_else(|e| e.into_inner());
                !set.insert(msg_key.to_string())
            };
            if already {
                return;
            }
            let client = notifier_http.clone();
            let api = notifier_api.clone();
            tokio::spawn(async move {
                let _ = client
                    .post(format!("{api}/sendMessage"))
                    .json(&serde_json::json!({
                        "chat_id": notifier_chat,
                        "text": msg,
                    }))
                    .send()
                    .await;
            });
        }
    }));

    let agent_result = react_loop(&mut ctx).await;

    let agent_summary = match agent_result {
        Ok(response) => {
            info!(turns = response.turns_used, "repair agent completed");
            response.text
        }
        Err(e) => {
            warn!(error = %e, "repair agent failed");
            return RepairOutcome::Failed {
                reason: format!("repair agent error: {e}"),
            };
        }
    };

    // Step 3: Verify the fix independently.
    notifier.send_msg(keys::REPAIR_VERIFYING).await;

    if let Err(e) = run_shell(repo_path, "cargo check", 180).await {
        warn!(error = %e, "cargo check failed after repair");
        notifier.send_msg(keys::REPAIR_CHECK_FAILED).await;
        return RepairOutcome::Failed {
            reason: format!("cargo check failed: {e}"),
        };
    }

    notifier.send_msg(keys::REPAIR_TESTING).await;

    if let Err(e) = run_shell(repo_path, "cargo test --workspace", 300).await {
        warn!(error = %e, "cargo test failed after repair");
        notifier.send_msg(keys::REPAIR_TEST_FAILED).await;
        return RepairOutcome::Failed {
            reason: format!("cargo test failed: {e}"),
        };
    }

    notifier.send_msg(keys::REPAIR_BUILDING).await;

    if let Err(e) = run_shell(repo_path, "cargo build --release", 600).await {
        warn!(error = %e, "cargo build --release failed after repair");
        notifier.send_msg(keys::REPAIR_BUILD_FAILED).await;
        return RepairOutcome::Failed {
            reason: format!("cargo build --release failed: {e}"),
        };
    }

    // Step 4: Commit the fix.
    notifier.send_msg(keys::REPAIR_COMMITTING).await;

    let commit_result = commit_fix(repo_path, &error_text).await;
    let commit_hash = match commit_result {
        Ok(hash) => hash,
        Err(e) => {
            warn!(error = %e, "failed to commit fix");
            "unknown".to_string()
        }
    };

    // Step 5: Push to remote.
    notifier.send_msg(keys::REPAIR_PUSHING).await;

    if let Err(e) = run_shell(repo_path, "git push", 60).await {
        warn!(error = %e, "git push failed after self-repair commit");
        notifier.send_msg(keys::REPAIR_PUSH_FAILED).await;
    } else {
        info!("self-repair fix pushed to remote");
    }

    info!(
        commit = %commit_hash,
        "self-repair completed successfully"
    );

    let short_summary = if agent_summary.len() > 200 {
        format!("{}...", &agent_summary[..200])
    } else {
        agent_summary.clone()
    };

    RepairOutcome::Fixed {
        commit_hash,
        summary: short_summary,
    }
}

/// Restart the current process by exec-ing into the new binary.
///
/// This replaces the running process with a fresh instance, preserving
/// all environment variables and command-line arguments.
///
/// # Safety
/// This function never returns on success (the process is replaced).
#[cfg(unix)]
pub fn restart_process() -> ! {
    use std::os::unix::process::CommandExt;

    let exe = std::env::current_exe().expect("failed to get current executable path");
    let args: Vec<String> = std::env::args().collect();

    info!(exe = %exe.display(), args = ?&args[1..], "restarting process via exec");

    // Give a small delay so any pending Telegram messages can be sent.
    std::thread::sleep(std::time::Duration::from_secs(2));

    // Use exec to replace this process with the new binary.
    let err = std::process::Command::new(&exe)
        .args(&args[1..])
        .exec();

    // If exec returns, it failed.
    error!(error = %err, "exec failed, exiting");
    std::process::exit(1);
}

/// Restart the current process (non-Unix fallback: spawn + exit).
#[cfg(not(unix))]
pub fn restart_process() -> ! {
    let exe = std::env::current_exe().expect("failed to get current executable path");
    let args: Vec<String> = std::env::args().collect();

    info!(exe = %exe.display(), args = ?&args[1..], "restarting process via spawn");

    std::thread::sleep(std::time::Duration::from_secs(2));

    let _ = std::process::Command::new(&exe)
        .args(&args[1..])
        .spawn();

    std::process::exit(0);
}

// ---------------------------------------------------------------------------
// Internal helpers
// ---------------------------------------------------------------------------

/// Determine whether an agent error looks like a code bug that we can fix.
fn is_code_bug(error: &openintent_agent::error::AgentError) -> bool {
    use openintent_agent::error::AgentError;

    match error {
        // Panics and internal errors are almost always code bugs.
        AgentError::Internal(msg) => {
            msg.contains("panicked")
                || msg.contains("char boundary")
                || msg.contains("index out of bounds")
                || msg.contains("overflow")
                || msg.contains("unwrap")
                || msg.contains("expect")
        }
        // Tool execution failure CAN be a code bug if it's a panic.
        AgentError::ToolExecutionFailed { reason, .. } => {
            reason.contains("panicked") || reason.contains("char boundary")
        }
        // Everything else is external (LLM, network, config, etc.).
        _ => false,
    }
}

/// Read the tail of a log file.
fn read_log_tail(path: &str, lines: usize) -> String {
    let path = Path::new(path);
    if !path.exists() {
        return String::new();
    }
    match std::fs::read_to_string(path) {
        Ok(content) => {
            let all_lines: Vec<&str> = content.lines().collect();
            let start = all_lines.len().saturating_sub(lines);
            all_lines[start..].join("\n")
        }
        Err(_) => String::new(),
    }
}

/// Read recent git commits.
fn read_recent_commits(repo_path: &Path, count: usize) -> String {
    let output = std::process::Command::new("git")
        .args(["log", "--oneline", &format!("-{count}")])
        .current_dir(repo_path)
        .output();

    match output {
        Ok(o) if o.status.success() => {
            String::from_utf8_lossy(&o.stdout).trim().to_string()
        }
        _ => String::new(),
    }
}

/// Build the system prompt for the repair agent.
fn build_repair_system_prompt(repo_path: &Path) -> String {
    // Load CLAUDE.md for project rules.
    let claude_md = std::fs::read_to_string(repo_path.join("CLAUDE.md")).unwrap_or_default();
    let rules_section = if claude_md.is_empty() {
        String::new()
    } else {
        let truncated = if claude_md.len() > 2000 {
            format!("{}...", &claude_md[..2000])
        } else {
            claude_md
        };
        format!("\n## Project Rules\n\n{truncated}\n")
    };

    format!(
        "You are the **self-repair module** of OpenIntentOS, an AI operating system written in Rust.\n\
         Repository root: {repo_path}\n\
         \n\
         ## Your Mission\n\
         A runtime error occurred while handling a user request. Your job is to:\n\
         1. **Analyze** the error message and log context to identify the root cause\n\
         2. **Locate** the relevant source file(s) using the error details\n\
         3. **Fix** the bug with a minimal, targeted edit\n\
         4. **Verify** the fix compiles with `cargo check` (use timeout_secs: 300)\n\
         \n\
         ## Critical Rules\n\
         - **ALWAYS `fs_read_file` before `fs_str_replace`.** You need exact current content.\n\
         - **Make MINIMAL changes.** Fix only the bug, don't refactor or improve.\n\
         - **Use `cargo check` after every edit** to verify compilation (timeout_secs: 300).\n\
         - **If `fs_str_replace` fails, re-read the file and try again.** Do NOT guess.\n\
         - **Use `shell_execute` with timeout_secs: 300** for cargo commands.\n\
         - After fixing, run `cargo check --workspace` to make sure everything compiles.\n\
         - Do NOT run `cargo build --release` — the caller handles that.\n\
         - Do NOT create git commits — the caller handles that.\n\
         - If you cannot identify the bug after 3 attempts, STOP and explain what you found.\n\
         {rules_section}\
         \n\
         ## Available Tools\n\
         - `fs_read_file` — Read a file\n\
         - `fs_write_file` — Write/overwrite a file\n\
         - `fs_str_replace` — Replace a unique string in a file\n\
         - `fs_list_dir` — List directory contents\n\
         - `shell_execute` — Run shell commands (cargo check, etc.)\n\
         - `fs_file_info` — Get file metadata\n",
        repo_path = repo_path.display()
    )
}

/// Build the user prompt with error details and diagnostic context.
fn build_repair_user_prompt(
    error: &str,
    user_message: &str,
    log_tail: &str,
    recent_commits: &str,
) -> String {
    let mut prompt = format!(
        "## Error\n\n```\n{error}\n```\n\n\
         ## User Message That Triggered the Error\n\n{user_message}\n\n"
    );

    if !log_tail.is_empty() {
        let log_section = if log_tail.len() > 3000 {
            &log_tail[log_tail.len() - 3000..]
        } else {
            log_tail
        };
        prompt.push_str(&format!(
            "## Recent Log (last 80 lines)\n\n```\n{log_section}\n```\n\n"
        ));
    }

    if !recent_commits.is_empty() {
        prompt.push_str(&format!(
            "## Recent Commits\n\n```\n{recent_commits}\n```\n\n"
        ));
    }

    prompt.push_str(
        "## Instructions\n\n\
         1. Read the error carefully. Identify which file and function caused it.\n\
         2. Read that source file with `fs_read_file`.\n\
         3. Find the buggy code and fix it with `fs_str_replace`.\n\
         4. Run `cargo check --workspace` to verify the fix compiles.\n\
         5. Summarize what you found and fixed.\n"
    );

    prompt
}

/// Run a shell command with a timeout. Returns stdout on success, error message on failure.
async fn run_shell(repo_path: &Path, command: &str, timeout_secs: u64) -> Result<String, String> {
    let result = tokio::time::timeout(
        std::time::Duration::from_secs(timeout_secs),
        tokio::process::Command::new("sh")
            .arg("-c")
            .arg(command)
            .current_dir(repo_path)
            .output(),
    )
    .await;

    match result {
        Ok(Ok(output)) => {
            if output.status.success() {
                Ok(String::from_utf8_lossy(&output.stdout).to_string())
            } else {
                let stderr = String::from_utf8_lossy(&output.stderr);
                let stdout = String::from_utf8_lossy(&output.stdout);
                Err(format!(
                    "`{command}` failed (exit {}):\n{stderr}{stdout}",
                    output.status.code().unwrap_or(-1)
                ))
            }
        }
        Ok(Err(e)) => Err(format!("failed to spawn `{command}`: {e}")),
        Err(_) => Err(format!("`{command}` timed out after {timeout_secs}s")),
    }
}

/// Stage all changes and commit with a descriptive message.
async fn commit_fix(repo_path: &Path, error_summary: &str) -> Result<String, String> {
    // Stage all changes.
    run_shell(repo_path, "git add -A", 30).await?;

    // Check if there are staged changes.
    let diff = run_shell(repo_path, "git diff --cached --stat", 30).await?;
    if diff.trim().is_empty() {
        return Err("no changes to commit".to_string());
    }

    // Build a commit message from the error.
    let short_error = if error_summary.len() > 60 {
        format!("{}...", &error_summary[..60])
    } else {
        error_summary.to_string()
    };
    // Sanitize for shell safety.
    let safe_msg = short_error.replace('\'', "").replace('\n', " ");
    let commit_msg = format!("fix: self-repair for {safe_msg}");

    run_shell(
        repo_path,
        &format!("git commit -m '{commit_msg}'"),
        30,
    )
    .await?;

    // Get the commit hash.
    let hash = run_shell(repo_path, "git log --oneline -1", 10)
        .await
        .unwrap_or_else(|_| "unknown".to_string());

    Ok(hash.trim().to_string())
}
