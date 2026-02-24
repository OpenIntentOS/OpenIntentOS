//! Telegram bot command handlers for dev tasks.
//!
//! Provides handlers for the `/dev`, `/tasks`, `/taskstatus`, `/merge`, and
//! `/cancel` commands, as well as mid-task message injection for active tasks.

use tracing::{info, warn};

use openintent_store::DevTaskStore;

// ═══════════════════════════════════════════════════════════════════════
//  Command handlers
// ═══════════════════════════════════════════════════════════════════════

/// Handle `/dev <instruction>` -- create a new dev task.
///
/// Checks for duplicate active tasks with the same intent (prevents
/// re-creation when Telegram redelivers messages after a bot restart).
pub async fn handle_dev_command(
    task_store: &DevTaskStore,
    chat_id: i64,
    instruction: &str,
) -> String {
    if instruction.trim().is_empty() {
        return "Usage: /dev <instruction>\n\nExample: /dev add a health check endpoint to the web server"
            .to_string();
    }

    info!(chat_id, instruction, "creating dev task");

    // Check for duplicate: same chat + same intent + still active.
    match task_store.find_active_by_intent(chat_id, instruction).await {
        Ok(Some(existing)) => {
            let short_id = &existing.id[..8.min(existing.id.len())];
            info!(
                chat_id,
                task_id = %existing.id,
                "duplicate dev task detected, returning existing"
            );
            return format!(
                "A task with the same intent already exists.\n\n\
                 Task ID: {}\n\
                 Status: {} {}\n\n\
                 Use /taskstatus {short_id} to check progress, \
                 or /cancel {short_id} to cancel it first.",
                existing.id,
                status_indicator(&existing.status),
                existing.status,
            );
        }
        Ok(None) => {} // No duplicate, proceed to create.
        Err(e) => {
            warn!(error = %e, "failed to check for duplicate dev task");
            // Non-fatal: proceed to create anyway.
        }
    }

    match task_store
        .create("telegram", Some(chat_id), instruction)
        .await
    {
        Ok(task) => {
            format!(
                "Dev task created!\n\n\
                 Task ID: {}\n\
                 Intent: {}\n\
                 Status: pending\n\n\
                 The agent will start working on this shortly. \
                 You'll receive progress updates here.\n\n\
                 Commands:\n\
                 /tasks - List your tasks\n\
                 /taskstatus {} - Check this task\n\
                 /cancel {} - Cancel this task",
                task.id,
                instruction,
                &task.id[..8],
                &task.id[..8],
            )
        }
        Err(e) => {
            warn!(error = %e, "failed to create dev task");
            format!("Failed to create dev task: {e}")
        }
    }
}

/// Handle `/tasks` -- list dev tasks for this chat.
pub async fn handle_tasks_command(task_store: &DevTaskStore, chat_id: i64) -> String {
    info!(chat_id, "listing dev tasks");

    match task_store.list_by_chat(chat_id, 10, 0).await {
        Ok(tasks) => {
            if tasks.is_empty() {
                return "No dev tasks found for this chat.\n\nUse /dev <instruction> to create one."
                    .to_string();
            }

            let mut output = format!("Dev tasks ({}):\n\n", tasks.len());
            for task in &tasks {
                let status_emoji = status_indicator(&task.status);
                let short_id = &task.id[..8.min(task.id.len())];
                let intent_preview = if task.intent.len() > 50 {
                    format!("{}...", &task.intent[..50])
                } else {
                    task.intent.clone()
                };

                output.push_str(&format!(
                    "{status_emoji} [{short_id}] {intent_preview}\n   Status: {}\n",
                    task.status,
                ));

                if let Some(ref pr_url) = task.pr_url {
                    output.push_str(&format!("   PR: {pr_url}\n"));
                }
                if let Some(ref error) = task.error {
                    let err_preview = if error.len() > 100 {
                        format!("{}...", &error[..100])
                    } else {
                        error.clone()
                    };
                    output.push_str(&format!("   Error: {err_preview}\n"));
                }
                output.push('\n');
            }

            output
        }
        Err(e) => {
            warn!(error = %e, "failed to list dev tasks");
            format!("Failed to list tasks: {e}")
        }
    }
}

/// Handle `/taskstatus <task_id>` -- show task details.
pub async fn handle_task_status_command(task_store: &DevTaskStore, task_id: &str) -> String {
    info!(task_id, "checking task status");

    // Allow short IDs by searching for a matching task.
    let task = match find_task_by_prefix(task_store, task_id).await {
        Ok(Some(t)) => t,
        Ok(None) => return format!("Task not found: {task_id}"),
        Err(e) => return format!("Error looking up task: {e}"),
    };

    let mut output = format!(
        "Task: {}\n\
         Status: {} {}\n\
         Intent: {}\n\
         Source: {}\n\
         Retries: {}/{}\n\
         Created: {}\n",
        task.id,
        status_indicator(&task.status),
        task.status,
        task.intent,
        task.source,
        task.retry_count,
        task.max_retries,
        format_timestamp(task.created_at),
    );

    if let Some(ref branch) = task.branch {
        output.push_str(&format!("Branch: {branch}\n"));
    }
    if let Some(ref pr_url) = task.pr_url {
        output.push_str(&format!("PR: {pr_url}\n"));
    }
    if let Some(ref step) = task.current_step {
        output.push_str(&format!("Current step: {step}\n"));
    }
    if let Some(ref error) = task.error {
        output.push_str(&format!("Error: {error}\n"));
    }

    // Show progress log.
    if let Some(log) = task.progress_log.as_array()
        && !log.is_empty()
    {
        output.push_str("\nProgress:\n");
        for entry in log {
            if let Some(s) = entry.as_str() {
                output.push_str(&format!("  - {s}\n"));
            }
        }
    }

    output
}

/// Handle `/merge <task_id>` -- merge a PR (mark as merging).
pub async fn handle_merge_command(
    task_store: &DevTaskStore,
    task_id: &str,
    chat_id: i64,
) -> String {
    info!(task_id, chat_id, "merge requested");

    let task = match find_task_by_prefix(task_store, task_id).await {
        Ok(Some(t)) => t,
        Ok(None) => return format!("Task not found: {task_id}"),
        Err(e) => return format!("Error looking up task: {e}"),
    };

    // Only allow merge from awaiting_review or pr_created status.
    if task.status != "awaiting_review" && task.status != "pr_created" {
        return format!(
            "Cannot merge task in '{}' status. Task must be in 'awaiting_review' status.",
            task.status
        );
    }

    // Verify this chat owns the task.
    if task.chat_id != Some(chat_id) {
        return "You can only merge tasks created from this chat.".to_string();
    }

    match task_store
        .update_status(&task.id, "merging", Some("Merge requested by user"))
        .await
    {
        Ok(()) => {
            let pr_info = task.pr_url.as_deref().unwrap_or("unknown");
            format!(
                "Merge initiated for task {}.\n\
                 PR: {}\n\n\
                 Note: Please merge the PR on GitHub directly. \
                 The task status has been updated to 'merging'.",
                &task.id[..8.min(task.id.len())],
                pr_info
            )
        }
        Err(e) => {
            warn!(error = %e, "failed to update task status for merge");
            format!("Failed to initiate merge: {e}")
        }
    }
}

/// Handle `/cancel <task_id>` -- cancel a task.
pub async fn handle_cancel_command(
    task_store: &DevTaskStore,
    task_id: &str,
    chat_id: i64,
) -> String {
    info!(task_id, chat_id, "cancel requested");

    let task = match find_task_by_prefix(task_store, task_id).await {
        Ok(Some(t)) => t,
        Ok(None) => return format!("Task not found: {task_id}"),
        Err(e) => return format!("Error looking up task: {e}"),
    };

    // Check terminal states.
    if task.status == "completed" || task.status == "cancelled" {
        return format!("Task is already in '{}' status.", task.status);
    }

    // Verify this chat owns the task.
    if task.chat_id != Some(chat_id) {
        return "You can only cancel tasks created from this chat.".to_string();
    }

    match task_store.cancel(&task.id).await {
        Ok(()) => {
            format!(
                "Task {} has been cancelled.\nIntent: {}",
                &task.id[..8.min(task.id.len())],
                task.intent,
            )
        }
        Err(e) => {
            warn!(error = %e, "failed to cancel task");
            format!("Failed to cancel task: {e}")
        }
    }
}

/// Check if a message is a mid-task instruction for an active dev task.
///
/// If the chat has an active task (status: coding, testing, or branching),
/// the message is appended as a user message to that task.
/// Returns `true` if the message was injected.
pub async fn try_inject_mid_task_message(
    task_store: &DevTaskStore,
    chat_id: i64,
    text: &str,
) -> bool {
    // Look for an active task in this chat.
    let tasks = match task_store.list_by_chat(chat_id, 5, 0).await {
        Ok(t) => t,
        Err(_) => return false,
    };

    let active_task = tasks
        .iter()
        .find(|t| t.status == "coding" || t.status == "testing" || t.status == "branching");

    let task = match active_task {
        Some(t) => t,
        None => return false,
    };

    info!(
        chat_id,
        task_id = %task.id,
        "injecting mid-task message"
    );

    match task_store.append_message(&task.id, "user", text).await {
        Ok(_) => true,
        Err(e) => {
            warn!(error = %e, "failed to inject mid-task message");
            false
        }
    }
}

// ═══════════════════════════════════════════════════════════════════════
//  Helper functions
// ═══════════════════════════════════════════════════════════════════════

/// Find a task by full ID or prefix match.
async fn find_task_by_prefix(
    task_store: &DevTaskStore,
    id_or_prefix: &str,
) -> Result<Option<openintent_store::DevTask>, String> {
    // Try exact match first.
    match task_store.get(id_or_prefix).await {
        Ok(Some(task)) => return Ok(Some(task)),
        Ok(None) => {}
        Err(e) => return Err(e.to_string()),
    }

    // Try prefix match by searching recent tasks.
    // We search across all statuses by checking multiple status groups.
    let statuses = [
        "pending",
        "branching",
        "coding",
        "testing",
        "pr_created",
        "awaiting_review",
        "merging",
        "completed",
        "failed",
        "cancelled",
    ];

    for status in &statuses {
        match task_store.list_by_status(status, 100, 0).await {
            Ok(tasks) => {
                for task in tasks {
                    if task.id.starts_with(id_or_prefix) {
                        return Ok(Some(task));
                    }
                }
            }
            Err(_) => continue,
        }
    }

    Ok(None)
}

/// Return a text indicator for the task status.
fn status_indicator(status: &str) -> &'static str {
    match status {
        "pending" => "[PENDING]",
        "branching" => "[BRANCH]",
        "coding" => "[CODING]",
        "testing" => "[TEST]",
        "pr_created" => "[PR]",
        "awaiting_review" => "[REVIEW]",
        "merging" => "[MERGE]",
        "completed" => "[DONE]",
        "failed" => "[FAIL]",
        "cancelled" => "[CANCEL]",
        _ => "[?]",
    }
}

/// Format a Unix timestamp into a human-readable string.
fn format_timestamp(ts: i64) -> String {
    chrono::DateTime::from_timestamp(ts, 0)
        .map(|dt| dt.format("%Y-%m-%d %H:%M:%S UTC").to_string())
        .unwrap_or_else(|| "unknown".to_string())
}
