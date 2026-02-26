//! Bot configuration and simple query classification helpers.
//!
//! Reads the `[bot]` section from `config/default.toml` and provides
//! utilities used by the Telegram bot gateway.

// ---------------------------------------------------------------------------
// Bot configuration
// ---------------------------------------------------------------------------

/// Settings loaded from the `[bot]` section of `config/default.toml`.
pub struct BotConfig {
    /// Number of conversation history messages to load per request.
    pub history_window: u32,
    /// Whether to show token usage stats after each response in-chat.
    pub show_token_usage: bool,
    /// Message length threshold for cheap model routing (0 = disabled).
    pub simple_query_threshold: usize,
}

/// Load bot configuration from `config/default.toml`.
///
/// Falls back to sensible defaults if the file is missing or the `[bot]`
/// section is absent.
pub fn load_bot_config() -> BotConfig {
    let defaults = BotConfig {
        history_window: 20,
        show_token_usage: false,
        simple_query_threshold: 120,
    };

    let content = match std::fs::read_to_string("config/default.toml") {
        Ok(c) => c,
        Err(_) => return defaults,
    };

    let table: toml::Table = match content.parse() {
        Ok(t) => t,
        Err(_) => return defaults,
    };

    let bot = match table.get("bot") {
        Some(toml::Value::Table(b)) => b,
        _ => return defaults,
    };

    BotConfig {
        history_window: bot
            .get("history_window")
            .and_then(|v| v.as_integer())
            .map(|v| v.max(1) as u32)
            .unwrap_or(defaults.history_window),
        show_token_usage: bot
            .get("show_token_usage")
            .and_then(|v| v.as_bool())
            .unwrap_or(defaults.show_token_usage),
        simple_query_threshold: bot
            .get("simple_query_threshold")
            .and_then(|v| v.as_integer())
            .map(|v| v.max(0) as usize)
            .unwrap_or(defaults.simple_query_threshold),
    }
}

// ---------------------------------------------------------------------------
// Model routing
// ---------------------------------------------------------------------------

/// Select the effective model for a request.
///
/// If simple query routing is enabled and the message looks like a simple
/// conversational query, returns a cheaper fast model. Otherwise returns
/// `primary_model`.
pub fn select_model_for_query(
    raw_text: &str,
    primary_model: &str,
    simple_query_threshold: usize,
    chat_id: i64,
) -> String {
    if simple_query_threshold > 0
        && raw_text.len() < simple_query_threshold
        && !is_complex_query(raw_text)
    {
        let cheap_model = "gemini-2.5-flash".to_string();
        tracing::debug!(
            chat_id,
            msg_len = raw_text.len(),
            threshold = simple_query_threshold,
            cheap_model = %cheap_model,
            "simple query detected, using cheap model"
        );
        cheap_model
    } else {
        primary_model.to_string()
    }
}

// ---------------------------------------------------------------------------
// Simple query detection
// ---------------------------------------------------------------------------

/// Returns `true` if the message is likely to require tools or complex
/// reasoning, making it unsuitable for cheap fast-model routing.
///
/// Heuristics checked:
/// - Contains a file path component (`.rs`, `.py`, `/`, `\`)
/// - Contains code-related keywords (`fn`, `impl`, backtick blocks, etc.)
/// - Contains tool-triggering action words (`search`, `git`, `email`, etc.)
pub fn is_complex_query(text: &str) -> bool {
    // File path heuristics.
    let has_file_path = text.contains('/')
        || text.contains('\\')
        || text.contains(".rs")
        || text.contains(".py")
        || text.contains(".js")
        || text.contains(".ts");

    if has_file_path {
        return true;
    }

    let lower = text.to_lowercase();

    // Code-related keywords suggest tool use.
    let code_keywords = [
        "fn ", "def ", "class ", "impl ", "function ", "```",
        "run ", "execute ", "build ", "git ", "compile",
        "debug ", "error ", "exception ", "stack trace",
    ];
    for kw in code_keywords {
        if lower.contains(kw) {
            return true;
        }
    }

    // Tool-triggering action words.
    let action_keywords = [
        "search ", "find ", "fetch ", "download ", "upload ",
        "read ", "write ", "create ", "delete ", "list ",
        "send ", "email ", "calendar ", "github ", "telegram ",
        "research ", "browse ", "open ", "show me ",
    ];
    for kw in action_keywords {
        if lower.contains(kw) {
            return true;
        }
    }

    false
}
