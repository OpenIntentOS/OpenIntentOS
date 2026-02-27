//! Multi-task splitting and per-task model routing.
//!
//! When a user sends multiple tasks in a single message, this module:
//! 1. Splits the message into individual sub-tasks.
//! 2. Classifies each task's complexity tier.
//! 3. Selects the appropriate model and turn budget per task.

use std::sync::Arc;

use openintent_agent::{
    AgentConfig, AgentContext, LlmClient, ToolAdapter, react_loop,
};
use tracing::info;

use crate::bot_helpers::split_telegram_message;
use crate::helpers::env_non_empty;

// ---------------------------------------------------------------------------
// Constants
// ---------------------------------------------------------------------------

const GOOGLE_BASE_URL: &str =
    "https://generativelanguage.googleapis.com/v1beta/openai";
const DEEPSEEK_BASE_URL: &str = "https://api.deepseek.com/v1";

// ---------------------------------------------------------------------------
// Types
// ---------------------------------------------------------------------------

/// Complexity tier for a sub-task.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TaskTier {
    /// Simple Q&A, greetings, lookups.
    Light,
    /// Research, analysis, web search, summaries.
    Medium,
    /// Coding, file creation, multi-step tool use.
    Heavy,
}

/// A single sub-task extracted from a multi-task message.
#[derive(Debug, Clone)]
pub struct SubTask {
    pub index: usize,
    pub text: String,
    pub tier: TaskTier,
}

/// Model + turn budget for a task tier.
pub struct TaskModelConfig {
    pub model: String,
    pub max_turns: u32,
}

// ---------------------------------------------------------------------------
// Task splitting
// ---------------------------------------------------------------------------

/// Split a user message into individual sub-tasks using heuristic parsing.
///
/// Detects numbered lists (`1.`, `1)`, `1、`, etc.) and splits accordingly.
/// If no numbered list is found, returns the whole message as a single task.
pub fn split_tasks(text: &str) -> Vec<SubTask> {
    let lines: Vec<&str> = text.lines().collect();
    let mut tasks: Vec<SubTask> = Vec::new();
    let mut current_task = String::new();
    let mut found_numbered = false;

    // Detect if the message uses emoji-prefixed task format.
    // If so, only emoji-prefixed lines start new tasks (not plain `1.` sub-items).
    let uses_emoji_tasks = lines.iter().any(|l| is_emoji_task_start(l.trim()));

    for line in &lines {
        let trimmed = line.trim();
        if trimmed.is_empty() {
            if found_numbered && !current_task.is_empty() {
                current_task.push('\n');
            }
            continue;
        }

        let is_new_task = if uses_emoji_tasks {
            is_emoji_task_start(trimmed)
        } else {
            is_numbered_start(trimmed)
        };

        if is_new_task {
            if found_numbered && !current_task.trim().is_empty() {
                tasks.push(SubTask {
                    index: tasks.len() + 1,
                    text: current_task.trim().to_string(),
                    tier: TaskTier::Medium,
                });
            }
            current_task = strip_number_prefix(trimmed).to_string();
            found_numbered = true;
        } else if found_numbered {
            current_task.push('\n');
            current_task.push_str(trimmed);
        } else {
            // Before any numbered item, accumulate as preamble.
            current_task.push_str(trimmed);
            current_task.push('\n');
        }
    }

    // Push the last accumulated task.
    if found_numbered && !current_task.trim().is_empty() {
        tasks.push(SubTask {
            index: tasks.len() + 1,
            text: current_task.trim().to_string(),
            tier: TaskTier::Medium,
        });
    }

    // If no numbered list found, treat as single task.
    if tasks.is_empty() {
        tasks.push(SubTask {
            index: 1,
            text: text.to_string(),
            tier: TaskTier::Medium,
        });
    }

    // Classify each task's complexity.
    for task in &mut tasks {
        task.tier = classify_complexity(&task.text);
    }

    info!(
        task_count = tasks.len(),
        tiers = ?tasks.iter().map(|t| format!("{}:{:?}", t.index, t.tier)).collect::<Vec<_>>(),
        "split message into sub-tasks"
    );

    tasks
}

// ---------------------------------------------------------------------------
// Complexity classification
// ---------------------------------------------------------------------------

/// Classify a task's complexity based on keyword heuristics.
fn classify_complexity(text: &str) -> TaskTier {
    let lower = text.to_lowercase();

    // Heavy: coding, file operations, complex tool chains.
    let heavy_keywords = [
        "code", "coding", "implement", "build", "create file",
        "write code", "script", "function", "class ", "module",
        "debug", "fix bug", "refactor", "deploy", "compile",
        "写代码", "编程", "实现", "开发", "部署", "修复",
        "脚本", "创建文件", "编写",
    ];
    for kw in heavy_keywords {
        if lower.contains(kw) {
            return TaskTier::Heavy;
        }
    }

    // Light: simple Q&A, greetings, status checks.
    let light_keywords = [
        "hello", "hi ", "hey ", "thanks", "thank you",
        "what is", "what's", "who is", "how are",
        "你好", "谢谢", "什么是", "是什么", "帮我翻译",
        "status", "time", "date", "weather",
    ];
    for kw in light_keywords {
        if lower.contains(kw) {
            return TaskTier::Light;
        }
    }

    // Medium: research, analysis, search, summaries.
    TaskTier::Medium
}

// ---------------------------------------------------------------------------
// Model routing
// ---------------------------------------------------------------------------

/// Select the model and turn budget for a task tier.
///
/// Strategy:
/// - Light → Gemini Flash (free, fast) or DeepSeek fallback, 5 turns
/// - Medium → DeepSeek (cheap, reliable), 10 turns
/// - Heavy → primary model, 20 turns
///
/// Falls back to DeepSeek if Gemini key is unavailable.
pub fn model_config_for_tier(tier: TaskTier, primary_model: &str) -> TaskModelConfig {
    let has_gemini = env_non_empty("GOOGLE_API_KEY").is_some();
    let has_deepseek = env_non_empty("DEEPSEEK_API_KEY").is_some();

    match tier {
        TaskTier::Light => {
            let model = if has_gemini {
                "gemini-2.5-flash"
            } else if has_deepseek {
                "deepseek-chat"
            } else {
                primary_model
            };
            TaskModelConfig { model: model.to_string(), max_turns: 5 }
        }
        TaskTier::Medium => {
            // Prefer DeepSeek for medium tasks (more reliable, no strict rate limit).
            let model = if has_deepseek {
                "deepseek-chat"
            } else if has_gemini {
                "gemini-2.5-flash"
            } else {
                primary_model
            };
            TaskModelConfig { model: model.to_string(), max_turns: 15 }
        }
        TaskTier::Heavy => {
            TaskModelConfig { model: primary_model.to_string(), max_turns: 25 }
        }
    }
}

/// Switch the LLM client to a specific model for a task.
///
/// Returns `true` if the switch was successful (or already on that model).
pub fn switch_to_model(model: &str, llm: &Arc<LlmClient>) -> bool {
    // Gemini Flash
    if model == "gemini-2.5-flash" {
        if let Some(key) = env_non_empty("GOOGLE_API_KEY") {
            llm.update_api_key(key);
            llm.switch_provider(
                openintent_agent::LlmProvider::OpenAI,
                GOOGLE_BASE_URL.to_string(),
                model.to_string(),
            );
            return true;
        }
        return false;
    }

    // DeepSeek
    if model == "deepseek-chat" {
        if let Some(key) = env_non_empty("DEEPSEEK_API_KEY") {
            llm.update_api_key(key);
            llm.switch_provider(
                openintent_agent::LlmProvider::OpenAI,
                DEEPSEEK_BASE_URL.to_string(),
                model.to_string(),
            );
            return true;
        }
        return false;
    }

    // For the primary model, restore defaults (the LLM client was
    // initialized with the primary model config).
    llm.restore_defaults();
    true
}

// ---------------------------------------------------------------------------
// Number detection helpers
// ---------------------------------------------------------------------------

/// Check if a line is an emoji-prefixed task start (e.g. "🎬 需求 1 — Clip").
fn is_emoji_task_start(line: &str) -> bool {
    let stripped = trim_leading_emoji(line);
    if stripped == line {
        return false; // No emoji prefix.
    }
    let lower = stripped.to_lowercase();
    lower.starts_with("需求")
        || lower.starts_with("task")
        || lower.starts_with("requirement")
        || lower.starts_with("req ")
}

/// Check if a line starts with a numbered list marker.
fn is_numbered_start(line: &str) -> bool {
    let trimmed = line.trim();

    // "1." "2." ... "99."
    if let Some(rest) = trimmed.strip_prefix(|c: char| c.is_ascii_digit()) {
        let rest = rest.trim_start_matches(|c: char| c.is_ascii_digit());
        if rest.starts_with(". ")
            || rest.starts_with(".")
            || rest.starts_with(") ")
            || rest.starts_with(")")
            || rest.starts_with("、")
        {
            return true;
        }
    }

    // Chinese circled numbers: ① ② ③ ...
    if trimmed.starts_with('①')
        || trimmed.starts_with('②')
        || trimmed.starts_with('③')
        || trimmed.starts_with('④')
        || trimmed.starts_with('⑤')
        || trimmed.starts_with('⑥')
        || trimmed.starts_with('⑦')
        || trimmed.starts_with('⑧')
        || trimmed.starts_with('⑨')
        || trimmed.starts_with('⑩')
    {
        return true;
    }

    // Emoji-prefixed requirement patterns: "🎬 需求 1", "🎯 Task 2", etc.
    // Strip leading emoji characters, then check for requirement label.
    let stripped = trim_leading_emoji(trimmed);
    if stripped != trimmed {
        let lower = stripped.to_lowercase();
        if lower.starts_with("需求")
            || lower.starts_with("task")
            || lower.starts_with("requirement")
            || lower.starts_with("req ")
        {
            return true;
        }
    }

    // Dash or bullet markers with content (not a separator like "---").
    if (trimmed.starts_with("- ") || trimmed.starts_with("* "))
        && trimmed.len() > 2
    {
        return true;
    }

    false
}

/// Strip leading emoji characters (and surrounding spaces) from a string.
fn trim_leading_emoji(s: &str) -> &str {
    let mut chars = s.chars();
    let mut byte_offset = 0;
    // Skip emoji characters at the start (Unicode categories: symbols, emoticons).
    while let Some(c) = chars.next() {
        if c.is_ascii_alphanumeric() || c.is_ascii_punctuation() {
            break;
        }
        // Skip non-ASCII, non-CJK characters (likely emoji) and spaces.
        if c == ' ' || (!c.is_ascii() && !is_cjk(c)) {
            byte_offset += c.len_utf8();
        } else {
            break;
        }
    }
    s[byte_offset..].trim_start()
}

/// Check if a character is CJK (Chinese/Japanese/Korean).
fn is_cjk(c: char) -> bool {
    matches!(c,
        '\u{4E00}'..='\u{9FFF}'   // CJK Unified Ideographs
        | '\u{3400}'..='\u{4DBF}' // CJK Extension A
        | '\u{3000}'..='\u{303F}' // CJK Symbols
        | '\u{3040}'..='\u{309F}' // Hiragana
        | '\u{30A0}'..='\u{30FF}' // Katakana
    )
}

/// Strip the number/bullet prefix from a line.
fn strip_number_prefix(line: &str) -> &str {
    let trimmed = line.trim();

    // "1. text" → "text"
    if let Some(pos) = trimmed.find(|c: char| c == '.' || c == ')') {
        let before = &trimmed[..pos];
        if before.chars().all(|c| c.is_ascii_digit()) {
            return trimmed[pos + 1..].trim_start();
        }
    }

    // "1、text"
    if let Some(rest) = trimmed.strip_prefix(|c: char| c.is_ascii_digit()) {
        let rest = rest.trim_start_matches(|c: char| c.is_ascii_digit());
        if let Some(rest) = rest.strip_prefix('、') {
            return rest.trim_start();
        }
    }

    // Circled numbers.
    for prefix in ['①', '②', '③', '④', '⑤', '⑥', '⑦', '⑧', '⑨', '⑩'] {
        if let Some(rest) = trimmed.strip_prefix(prefix) {
            return rest.trim_start();
        }
    }

    // Emoji-prefixed requirement: "🎬 需求 1 — Clip\ndetails" → full line
    // Keep the full content since the label IS the task description.
    let stripped = trim_leading_emoji(trimmed);
    if stripped != trimmed {
        let lower = stripped.to_lowercase();
        if lower.starts_with("需求")
            || lower.starts_with("task")
            || lower.starts_with("requirement")
        {
            return stripped;
        }
    }

    // Dash/bullet.
    if let Some(rest) = trimmed.strip_prefix("- ") {
        return rest;
    }
    if let Some(rest) = trimmed.strip_prefix("* ") {
        return rest;
    }

    trimmed
}

// ---------------------------------------------------------------------------
// Multi-task execution
// ---------------------------------------------------------------------------

/// Result of multi-task execution.
pub struct MultiTaskResult {
    pub summary: String,
    pub total_input_tokens: u32,
    pub total_output_tokens: u32,
}

/// Execute multiple sub-tasks sequentially, each with its own model and turn
/// budget.  Sends results to Telegram incrementally after each task completes.
pub async fn run_multi_task(
    sub_tasks: &[SubTask],
    primary_model: &str,
    system_prompt: &str,
    llm: &Arc<LlmClient>,
    adapters: &[Arc<dyn ToolAdapter>],
    chat_id: i64,
    http: &reqwest::Client,
    telegram_api: &str,
) -> MultiTaskResult {
    let mut total_in: u32 = 0;
    let mut total_out: u32 = 0;
    let task_count = sub_tasks.len();

    for task in sub_tasks {
        let tc = model_config_for_tier(task.tier, primary_model);
        info!(
            chat_id,
            task_index = task.index,
            tier = ?task.tier,
            task_model = %tc.model,
            max_turns = tc.max_turns,
            "running sub-task"
        );

        // Switch LLM to the task-appropriate model.
        switch_to_model(&tc.model, llm);

        let task_config = AgentConfig {
            max_turns: tc.max_turns,
            model: tc.model.clone(),
            temperature: Some(0.5),
            max_tokens: Some(4096),
            ..AgentConfig::default()
        };
        let task_prompt = format!("[Task {}/{}] {}", task.index, task_count, task.text);
        let mut task_ctx =
            AgentContext::new(llm.clone(), adapters.to_vec(), task_config)
                .with_system_prompt(system_prompt)
                .with_user_message(&task_prompt);

        let (reply, in_tok, out_tok) = match react_loop(&mut task_ctx).await {
            Ok(mut resp) => {
                // Self-repair: if the sub-task hit its turn limit, retry once
                // with a continuation prompt to finish incomplete work.
                if resp.hit_turn_limit {
                    tracing::info!(
                        chat_id,
                        task_index = task.index,
                        "sub-task hit turn limit, attempting continuation"
                    );
                    let cont_prompt = format!(
                        "Your previous attempt ran out of turns. Partial result:\n\n{}\n\n\
                         COMPLETE the remaining work concisely. Do NOT repeat finished steps.",
                        resp.text
                    );
                    let retry_config = AgentConfig {
                        max_turns: 10,
                        model: tc.model.clone(),
                        temperature: Some(0.5),
                        max_tokens: Some(4096),
                        ..AgentConfig::default()
                    };
                    let mut retry_ctx =
                        AgentContext::new(llm.clone(), adapters.to_vec(), retry_config)
                            .with_system_prompt(system_prompt)
                            .with_user_message(&cont_prompt);
                    if let Ok(retry_resp) = react_loop(&mut retry_ctx).await {
                        tracing::info!(
                            chat_id,
                            task_index = task.index,
                            turns = retry_resp.turns_used,
                            "sub-task continuation completed"
                        );
                        resp.text = retry_resp.text;
                        resp.turns_used += retry_resp.turns_used;
                        resp.input_tokens += retry_resp.input_tokens;
                        resp.output_tokens += retry_resp.output_tokens;
                    }
                }
                info!(
                    chat_id,
                    task_index = task.index,
                    turns = resp.turns_used,
                    model = %tc.model,
                    "sub-task completed"
                );
                let label = format!("Task {}/{}", task.index, task_count);
                (format!("{label}\n{}", resp.text), resp.input_tokens, resp.output_tokens)
            }
            Err(e) => {
                let err_str = e.to_string();
                // If the cheap model hit rate limit, retry with primary model.
                if crate::failover::is_rate_limit_error(&err_str)
                    && tc.model != primary_model
                {
                    tracing::warn!(
                        chat_id,
                        task_index = task.index,
                        failed_model = %tc.model,
                        "sub-task rate-limited, retrying with primary model"
                    );
                    switch_to_model(primary_model, llm);
                    let retry_config = AgentConfig {
                        max_turns: tc.max_turns,
                        model: primary_model.to_string(),
                        temperature: Some(0.5),
                        max_tokens: Some(4096),
                        ..AgentConfig::default()
                    };
                    let mut retry_ctx =
                        AgentContext::new(llm.clone(), adapters.to_vec(), retry_config)
                            .with_system_prompt(system_prompt)
                            .with_user_message(&task_prompt);
                    match react_loop(&mut retry_ctx).await {
                        Ok(resp) => {
                            info!(
                                chat_id, task_index = task.index,
                                turns = resp.turns_used, model = primary_model,
                                "sub-task completed on primary model"
                            );
                            let label = format!("Task {}/{}", task.index, task_count);
                            (format!("{label}\n{}", resp.text), resp.input_tokens, resp.output_tokens)
                        }
                        Err(e2) => {
                            tracing::warn!(chat_id, task_index = task.index, error = %e2, "sub-task failed on retry");
                            let msg = format!("Task {}/{}: error — {}", task.index, task_count, e2);
                            (msg, 0, 0)
                        }
                    }
                } else if crate::failover::is_provider_error(&err_str)
                    && !crate::failover::is_rate_limit_error(&err_str)
                {
                    // Stream / transient provider error — retry once.
                    // If on a cheap model, retry with primary; otherwise retry same model.
                    let retry_model = if tc.model != primary_model {
                        primary_model
                    } else {
                        &tc.model
                    };
                    tracing::warn!(
                        chat_id,
                        task_index = task.index,
                        failed_model = %tc.model,
                        retry_model,
                        "sub-task stream/provider error, retrying"
                    );
                    switch_to_model(retry_model, llm);
                    let retry_config = AgentConfig {
                        max_turns: tc.max_turns,
                        model: retry_model.to_string(),
                        temperature: Some(0.5),
                        max_tokens: Some(4096),
                        ..AgentConfig::default()
                    };
                    let mut retry_ctx =
                        AgentContext::new(llm.clone(), adapters.to_vec(), retry_config)
                            .with_system_prompt(system_prompt)
                            .with_user_message(&task_prompt);
                    match react_loop(&mut retry_ctx).await {
                        Ok(resp) => {
                            info!(
                                chat_id, task_index = task.index,
                                turns = resp.turns_used, model = retry_model,
                                "sub-task completed after retry"
                            );
                            let label = format!("Task {}/{}", task.index, task_count);
                            (format!("{label}\n{}", resp.text), resp.input_tokens, resp.output_tokens)
                        }
                        Err(e2) => {
                            tracing::warn!(chat_id, task_index = task.index, error = %e2, "sub-task failed on retry");
                            let msg = format!("Task {}/{}: error — {}", task.index, task_count, e2);
                            (msg, 0, 0)
                        }
                    }
                } else {
                    tracing::warn!(chat_id, task_index = task.index, error = %e, "sub-task failed");
                    let msg = format!("Task {}/{}: error — {}", task.index, task_count, e);
                    (msg, 0, 0)
                }
            }
        };

        total_in += in_tok;
        total_out += out_tok;

        // Send this task's result immediately.
        let chunks = split_telegram_message(&reply, 4000);
        for chunk in &chunks {
            let _ = http
                .post(format!("{telegram_api}/sendMessage"))
                .json(&serde_json::json!({
                    "chat_id": chat_id,
                    "text": chunk,
                }))
                .send()
                .await;
        }
    }

    // Restore LLM to user's chosen model after multi-task run.
    switch_to_model(primary_model, llm);

    MultiTaskResult {
        summary: format!("All {} tasks processed.", task_count),
        total_input_tokens: total_in,
        total_output_tokens: total_out,
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn split_numbered_list() {
        let text = "1. Research AI companies\n2. Write a Python script\n3. Hello world";
        let tasks = split_tasks(text);
        assert_eq!(tasks.len(), 3);
        assert_eq!(tasks[0].text, "Research AI companies");
        assert_eq!(tasks[1].text, "Write a Python script");
        assert_eq!(tasks[2].text, "Hello world");
    }

    #[test]
    fn split_chinese_numbered() {
        let text = "1、帮我找AI公司\n2、帮我写代码\n3、你好";
        let tasks = split_tasks(text);
        assert_eq!(tasks.len(), 3);
    }

    #[test]
    fn split_single_message() {
        let text = "Tell me about Rust programming";
        let tasks = split_tasks(text);
        assert_eq!(tasks.len(), 1);
        assert_eq!(tasks[0].text, text);
    }

    #[test]
    fn split_bullet_points() {
        let text = "- Research competitors\n- Build landing page\n- Send emails";
        let tasks = split_tasks(text);
        assert_eq!(tasks.len(), 3);
    }

    #[test]
    fn classify_coding_as_heavy() {
        assert_eq!(classify_complexity("Write code for a web server"), TaskTier::Heavy);
        assert_eq!(classify_complexity("帮我写代码"), TaskTier::Heavy);
        assert_eq!(classify_complexity("Implement a REST API"), TaskTier::Heavy);
    }

    #[test]
    fn classify_greeting_as_light() {
        assert_eq!(classify_complexity("Hello!"), TaskTier::Light);
        assert_eq!(classify_complexity("你好"), TaskTier::Light);
        assert_eq!(classify_complexity("What is Rust?"), TaskTier::Light);
    }

    #[test]
    fn classify_research_as_medium() {
        assert_eq!(classify_complexity("Find top 10 AI startups"), TaskTier::Medium);
        assert_eq!(classify_complexity("Analyze market trends"), TaskTier::Medium);
    }

    #[test]
    fn split_emoji_prefixed_tasks() {
        let text = "\
\u{1f3ac} 需求 1 — Clip\n\
帮我把视频做成竖屏\n\
要求：加字幕\n\
\n\
\u{1f3af} 需求 2 — Lead\n\
帮我找客户\n\
ICP: 5-50人\n\
\n\
\u{1f50d} 需求 3 — Collector\n\
监控 OpenAI 动态";
        let tasks = split_tasks(text);
        assert_eq!(tasks.len(), 3, "expected 3 tasks, got {:?}", tasks);
        assert!(tasks[0].text.contains("Clip"), "task 1: {:?}", tasks[0].text);
        assert!(tasks[0].text.contains("加字幕"), "task 1 details: {:?}", tasks[0].text);
        assert!(tasks[1].text.contains("Lead"), "task 2: {:?}", tasks[1].text);
        assert!(tasks[2].text.contains("Collector"), "task 3: {:?}", tasks[2].text);
    }

    #[test]
    fn split_emoji_tasks_with_sub_numbers() {
        // Sub-items with numbers should NOT be split into separate tasks.
        let text = "\
\u{1f3ac} 需求 1 — Clip\n\
帮我做视频\n\
\n\
\u{1f310} 需求 2 — Browser\n\
用浏览器自动化帮我：\n\
1. 登录 GitHub\n\
2. 找 top 10\n\
3. 整理成表格";
        let tasks = split_tasks(text);
        assert_eq!(tasks.len(), 2, "expected 2 tasks, got {:?}", tasks);
        // The sub-items should be part of task 2.
        assert!(tasks[1].text.contains("登录 GitHub"), "task 2 should contain sub-items");
        assert!(tasks[1].text.contains("整理成表格"), "task 2 should contain sub-item 3");
    }

    #[test]
    fn model_config_tiers() {
        let light = model_config_for_tier(TaskTier::Light, "deepseek-chat");
        assert_eq!(light.model, "gemini-2.5-flash");
        assert_eq!(light.max_turns, 5);

        let heavy = model_config_for_tier(TaskTier::Heavy, "deepseek-chat");
        assert_eq!(heavy.model, "deepseek-chat");
        assert_eq!(heavy.max_turns, 20);
    }
}
