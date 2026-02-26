//! User-facing message templates loaded from `config/default.toml`.
//!
//! All user-facing strings are stored in English in the config file.
//! At runtime, messages are translated to the user's language via LLM
//! when the user's language is not English.

use std::collections::HashMap;
use std::sync::Arc;

use tokio::sync::Mutex;
use tracing::{info, warn};

use openintent_agent::{ChatRequest, LlmClient, LlmResponse, Message};

// ---------------------------------------------------------------------------
// Message keys (compile-time constants to avoid typos)
// ---------------------------------------------------------------------------

pub mod keys {
    // Self-repair
    pub const REPAIR_STARTED: &str = "self_repair.started";
    pub const REPAIR_ANALYZING: &str = "self_repair.analyzing";
    pub const REPAIR_FIXING: &str = "self_repair.fixing";
    pub const REPAIR_COMPILING: &str = "self_repair.compiling";
    pub const REPAIR_VERIFYING: &str = "self_repair.verifying";
    pub const REPAIR_CHECK_FAILED: &str = "self_repair.check_failed";
    pub const REPAIR_TESTING: &str = "self_repair.testing";
    pub const REPAIR_TEST_FAILED: &str = "self_repair.test_failed";
    pub const REPAIR_BUILDING: &str = "self_repair.building";
    pub const REPAIR_BUILD_FAILED: &str = "self_repair.build_failed";
    pub const REPAIR_COMMITTING: &str = "self_repair.committing";
    pub const REPAIR_PUSHING: &str = "self_repair.pushing";
    pub const REPAIR_PUSH_FAILED: &str = "self_repair.push_failed";
    pub const REPAIR_SUCCESS: &str = "self_repair.success";

    // Errors
    pub const ERROR_GENERAL: &str = "errors.general";
    pub const ERROR_REPAIR_FAILED: &str = "errors.repair_failed";

    // Startup
    pub const STARTUP_WITH_UPDATES: &str = "startup.with_updates";
    pub const STARTUP_SIMPLE: &str = "startup.simple";

    // Bot messages
    pub const BOT_TOKEN_USAGE: &str = "bot.token_usage";
    pub const BOT_TOKENS_ON: &str = "bot.tokens_on";
    pub const BOT_TOKENS_OFF: &str = "bot.tokens_off";

    // Tool status
    pub const STATUS_RESEARCHING: &str = "status.researching";
    pub const STATUS_SEARCHING: &str = "status.searching";
    pub const STATUS_READING_PAGE: &str = "status.reading_page";
    pub const STATUS_READING_FILES: &str = "status.reading_files";
    pub const STATUS_RUNNING_COMMAND: &str = "status.running_command";
    pub const STATUS_ACCESSING_MEMORY: &str = "status.accessing_memory";
    pub const STATUS_GITHUB: &str = "status.github";
}

// ---------------------------------------------------------------------------
// Messages store
// ---------------------------------------------------------------------------

/// Thread-safe store of user-facing message templates loaded from config.
#[derive(Clone)]
pub struct Messages {
    templates: HashMap<String, String>,
    /// Cache of translated messages: (key, lang) -> translated text.
    translation_cache: Arc<Mutex<HashMap<(String, String), String>>>,
}

impl Messages {
    /// Load message templates from `config/default.toml`.
    ///
    /// Reads the `[messages]` section and flattens nested tables into
    /// dot-separated keys (e.g., `self_repair.started`).
    pub fn load() -> Self {
        let templates = load_from_config();
        Self {
            templates,
            translation_cache: Arc::new(Mutex::new(HashMap::new())),
        }
    }

    /// Get the English template for a key, with optional placeholder substitution.
    pub fn get(&self, key: &str) -> String {
        self.templates
            .get(key)
            .cloned()
            .unwrap_or_else(|| key.to_string())
    }

    /// Get a message with placeholder substitution.
    ///
    /// Replaces `{name}` placeholders with values from the provided map.
    pub fn get_with(&self, key: &str, vars: &[(&str, &str)]) -> String {
        let mut msg = self.get(key);
        for (name, value) in vars {
            msg = msg.replace(&format!("{{{name}}}"), value);
        }
        msg
    }

    /// Get a message translated to the user's language.
    ///
    /// If the user's language is English (or undetected), returns the
    /// English template directly. Otherwise, uses the LLM to translate
    /// and caches the result.
    pub async fn get_translated(
        &self,
        key: &str,
        vars: &[(&str, &str)],
        user_lang: &str,
        llm: &Arc<LlmClient>,
        model: &str,
    ) -> String {
        let english = self.get_with(key, vars);

        if is_english(user_lang) {
            return english;
        }

        // Check cache first.
        let cache_key = (format!("{key}:{}", vars_to_string(vars)), user_lang.to_string());
        {
            let cache = self.translation_cache.lock().await;
            if let Some(cached) = cache.get(&cache_key) {
                return cached.clone();
            }
        }

        // Translate via LLM.
        let translated = translate_message(llm, model, &english, user_lang).await;
        let result = translated.unwrap_or(english);

        // Cache the translation.
        {
            let mut cache = self.translation_cache.lock().await;
            cache.insert(cache_key, result.clone());
        }

        result
    }

    /// Batch-translate a set of messages for a user's language.
    ///
    /// This is more efficient than translating one by one — it sends all
    /// messages in a single LLM call. Useful for self-repair progress
    /// messages that need to be sent quickly.
    pub async fn batch_translate(
        &self,
        keys: &[&str],
        user_lang: &str,
        llm: &Arc<LlmClient>,
        model: &str,
    ) -> HashMap<String, String> {
        let mut result = HashMap::new();

        if is_english(user_lang) {
            for key in keys {
                result.insert(key.to_string(), self.get(key));
            }
            return result;
        }

        // Collect English messages.
        let english_msgs: Vec<(String, String)> = keys
            .iter()
            .map(|k| (k.to_string(), self.get(k)))
            .collect();

        // Check cache and collect untranslated ones.
        let mut untranslated = Vec::new();
        {
            let cache = self.translation_cache.lock().await;
            for (key, eng) in &english_msgs {
                let cache_key = (format!("{key}:"), user_lang.to_string());
                if let Some(cached) = cache.get(&cache_key) {
                    result.insert(key.clone(), cached.clone());
                } else {
                    untranslated.push((key.clone(), eng.clone()));
                }
            }
        }

        if untranslated.is_empty() {
            return result;
        }

        // Batch translate via LLM.
        let translations =
            batch_translate_messages(llm, model, &untranslated, user_lang).await;

        // Cache and collect results.
        {
            let mut cache = self.translation_cache.lock().await;
            for (key, translated) in &translations {
                let cache_key = (format!("{key}:"), user_lang.to_string());
                cache.insert(cache_key, translated.clone());
                result.insert(key.clone(), translated.clone());
            }
        }

        // Fill in any missing (fallback to English).
        for (key, eng) in &english_msgs {
            result.entry(key.clone()).or_insert_with(|| eng.clone());
        }

        result
    }
}

// ---------------------------------------------------------------------------
// Language detection
// ---------------------------------------------------------------------------

/// Determine if a language code represents English.
pub fn is_english(lang: &str) -> bool {
    let l = lang.to_lowercase();
    l.is_empty() || l == "en" || l.starts_with("en-") || l.starts_with("en_")
}

/// Normalize a Telegram language_code (e.g., "zh-hans" -> "Chinese",
/// "en" -> "English", "ja" -> "Japanese").
pub fn lang_display_name(code: &str) -> &str {
    let c = code.to_lowercase();
    if c.starts_with("zh") {
        "Chinese"
    } else if c.starts_with("en") || c.is_empty() {
        "English"
    } else if c.starts_with("ja") {
        "Japanese"
    } else if c.starts_with("ko") {
        "Korean"
    } else if c.starts_with("es") {
        "Spanish"
    } else if c.starts_with("fr") {
        "French"
    } else if c.starts_with("de") {
        "German"
    } else if c.starts_with("pt") {
        "Portuguese"
    } else if c.starts_with("ru") {
        "Russian"
    } else if c.starts_with("ar") {
        "Arabic"
    } else {
        // Return the code itself as a fallback — the LLM will understand it.
        code
    }
}

// ---------------------------------------------------------------------------
// Config loading
// ---------------------------------------------------------------------------

/// Load templates from `config/default.toml` `[messages]` section.
fn load_from_config() -> HashMap<String, String> {
    let mut templates = HashMap::new();

    let config_path = std::path::Path::new("config/default.toml");
    let content = match std::fs::read_to_string(config_path) {
        Ok(c) => c,
        Err(e) => {
            warn!(error = %e, "failed to read config/default.toml, using built-in defaults");
            return builtin_defaults();
        }
    };

    let table: toml::Table = match content.parse() {
        Ok(t) => t,
        Err(e) => {
            warn!(error = %e, "failed to parse config/default.toml, using built-in defaults");
            return builtin_defaults();
        }
    };

    let messages = match table.get("messages") {
        Some(toml::Value::Table(m)) => m,
        _ => {
            warn!("no [messages] section in config/default.toml, using built-in defaults");
            return builtin_defaults();
        }
    };

    // Flatten nested tables: messages.self_repair.started -> "self_repair.started"
    flatten_toml(messages, "", &mut templates);

    info!(count = templates.len(), "loaded message templates from config");
    templates
}

/// Recursively flatten a TOML table into dot-separated key-value pairs.
fn flatten_toml(table: &toml::Table, prefix: &str, out: &mut HashMap<String, String>) {
    for (key, value) in table {
        let full_key = if prefix.is_empty() {
            key.clone()
        } else {
            format!("{prefix}.{key}")
        };

        match value {
            toml::Value::String(s) => {
                out.insert(full_key, s.clone());
            }
            toml::Value::Table(t) => {
                flatten_toml(t, &full_key, out);
            }
            _ => {
                // Skip non-string, non-table values.
            }
        }
    }
}

/// Built-in defaults in case config file is missing or malformed.
fn builtin_defaults() -> HashMap<String, String> {
    let mut m = HashMap::new();

    m.insert("self_repair.started".into(), "Found an issue, analyzing and fixing automatically...".into());
    m.insert("self_repair.analyzing".into(), "Analyzing code...".into());
    m.insert("self_repair.fixing".into(), "Fixing the issue...".into());
    m.insert("self_repair.compiling".into(), "Compiling and verifying...".into());
    m.insert("self_repair.verifying".into(), "Verifying the fix...".into());
    m.insert("self_repair.check_failed".into(), "Auto-fix failed: code verification didn't pass. I'll keep improving.".into());
    m.insert("self_repair.testing".into(), "Running tests...".into());
    m.insert("self_repair.test_failed".into(), "Auto-fix failed: tests didn't pass. I'll keep improving.".into());
    m.insert("self_repair.building".into(), "Building new version...".into());
    m.insert("self_repair.build_failed".into(), "Auto-fix failed: build error. I'll keep improving.".into());
    m.insert("self_repair.committing".into(), "Saving the fix...".into());
    m.insert("self_repair.pushing".into(), "Pushing fix to remote repository...".into());
    m.insert("self_repair.push_failed".into(), "Fix saved locally, but push to remote failed. Will retry later.".into());
    m.insert("self_repair.success".into(), "I found and fixed an issue automatically!\n\nFix: {summary}\n\nRestarting now — please resend your message in a few seconds...".into());
    m.insert("errors.general".into(), "Sorry, something went wrong while processing your request. Please try again later or rephrase your message.".into());
    m.insert("errors.repair_failed".into(), "Sorry, I encountered an issue and tried to fix it automatically, but it didn't work this time. The developer has been notified. Please try again later.".into());
    m.insert("startup.with_updates".into(), "I just restarted with updates!\n\n{summary}".into());
    m.insert("startup.simple".into(), "I just restarted and I'm ready to help!".into());
    m.insert("status.researching".into(), "Researching...".into());
    m.insert("status.searching".into(), "Searching...".into());
    m.insert("status.reading_page".into(), "Reading page...".into());
    m.insert("status.reading_files".into(), "Reading files...".into());
    m.insert("status.running_command".into(), "Running command...".into());
    m.insert("status.accessing_memory".into(), "Accessing memory...".into());
    m.insert("status.github".into(), "Working with GitHub...".into());
    m.insert("bot.token_usage".into(), "📊 tokens: ↑{input} ↓{output} (total: {total})".into());
    m.insert("bot.tokens_on".into(), "Token usage display enabled for this chat.".into());
    m.insert("bot.tokens_off".into(), "Token usage display disabled for this chat.".into());

    m
}

// ---------------------------------------------------------------------------
// LLM translation
// ---------------------------------------------------------------------------

/// Translate a single message to the user's language via LLM.
async fn translate_message(
    llm: &Arc<LlmClient>,
    model: &str,
    english: &str,
    user_lang: &str,
) -> Option<String> {
    let lang_name = lang_display_name(user_lang);

    let request = ChatRequest {
        model: model.to_owned(),
        messages: vec![
            Message::system(
                "You are a translator. Translate the following message to the target language. \
                 Keep it natural, friendly, and concise. Preserve any emoji. \
                 Output ONLY the translated text, nothing else.",
            ),
            Message::user(format!(
                "Translate to {lang_name}:\n\n{english}"
            )),
        ],
        tools: vec![],
        temperature: Some(0.1),
        max_tokens: Some(256),
        stream: false,
    };

    match llm.chat(&request).await {
        Ok(LlmResponse::Text(text)) => Some(text.trim().to_string()),
        _ => None,
    }
}

/// Batch-translate multiple messages in a single LLM call.
async fn batch_translate_messages(
    llm: &Arc<LlmClient>,
    model: &str,
    messages: &[(String, String)],
    user_lang: &str,
) -> Vec<(String, String)> {
    let lang_name = lang_display_name(user_lang);

    // Build a numbered list for the LLM.
    let mut numbered = String::new();
    for (i, (_key, eng)) in messages.iter().enumerate() {
        numbered.push_str(&format!("{}. {}\n", i + 1, eng));
    }

    let request = ChatRequest {
        model: model.to_owned(),
        messages: vec![
            Message::system(
                "You are a translator. Translate each numbered line to the target language. \
                 Keep the numbering. Keep it natural, friendly, and concise. Preserve any emoji. \
                 Output ONLY the numbered translations, one per line.",
            ),
            Message::user(format!(
                "Translate each line to {lang_name}:\n\n{numbered}"
            )),
        ],
        tools: vec![],
        temperature: Some(0.1),
        max_tokens: Some(1024),
        stream: false,
    };

    let response = match llm.chat(&request).await {
        Ok(LlmResponse::Text(text)) => text,
        _ => return Vec::new(),
    };

    // Parse numbered results: "1. 翻译的文字" -> ("key", "翻译的文字")
    let mut results = Vec::new();
    for line in response.lines() {
        let line = line.trim();
        if line.is_empty() {
            continue;
        }
        // Match "N. text" or "N) text" patterns.
        if let Some(dot_pos) = line.find(". ") {
            if let Ok(idx) = line[..dot_pos].trim().parse::<usize>() {
                if idx >= 1 && idx <= messages.len() {
                    let translated = line[dot_pos + 2..].trim().to_string();
                    results.push((messages[idx - 1].0.clone(), translated));
                }
            }
        }
    }

    results
}

/// Safely truncate a string to at most `max_bytes`, respecting UTF-8
/// char boundaries. Appends "..." if truncated.
///
/// This MUST be used instead of `&s[..n]` to avoid char-boundary panics.
pub fn safe_truncate(s: &str, max_bytes: usize) -> String {
    if s.len() <= max_bytes {
        return s.to_string();
    }
    let mut end = max_bytes;
    while end > 0 && !s.is_char_boundary(end) {
        end -= 1;
    }
    let mut result = s[..end].to_string();
    result.push_str("...");
    result
}

/// Safely take the first `max_bytes` of a string without "..." suffix.
/// Useful for IDs and short labels.
pub fn safe_prefix(s: &str, max_bytes: usize) -> &str {
    if s.len() <= max_bytes {
        return s;
    }
    let mut end = max_bytes;
    while end > 0 && !s.is_char_boundary(end) {
        end -= 1;
    }
    &s[..end]
}

/// Helper: serialize vars to a string for cache key.
fn vars_to_string(vars: &[(&str, &str)]) -> String {
    vars.iter()
        .map(|(k, v)| format!("{k}={v}"))
        .collect::<Vec<_>>()
        .join(",")
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn is_english_detects_correctly() {
        assert!(is_english("en"));
        assert!(is_english("en-US"));
        assert!(is_english("en_GB"));
        assert!(is_english("EN"));
        assert!(is_english(""));
        assert!(!is_english("zh-hans"));
        assert!(!is_english("ja"));
        assert!(!is_english("ko"));
    }

    #[test]
    fn lang_display_name_maps_common_codes() {
        assert_eq!(lang_display_name("zh-hans"), "Chinese");
        assert_eq!(lang_display_name("ja"), "Japanese");
        assert_eq!(lang_display_name("ko"), "Korean");
        assert_eq!(lang_display_name("en"), "English");
        assert_eq!(lang_display_name(""), "English");
    }

    #[test]
    fn builtin_defaults_has_all_keys() {
        let defaults = builtin_defaults();
        assert!(defaults.contains_key("self_repair.started"));
        assert!(defaults.contains_key("errors.general"));
        assert!(defaults.contains_key("startup.simple"));
        assert!(defaults.contains_key("status.researching"));
    }

    #[test]
    fn get_with_substitution() {
        let msgs = Messages {
            templates: {
                let mut m = HashMap::new();
                m.insert("test.hello".into(), "Hello {name}, welcome to {place}!".into());
                m
            },
            translation_cache: Arc::new(Mutex::new(HashMap::new())),
        };

        let result = msgs.get_with("test.hello", &[("name", "Alice"), ("place", "Wonderland")]);
        assert_eq!(result, "Hello Alice, welcome to Wonderland!");
    }

    #[test]
    fn flatten_toml_works() {
        let toml_str = r#"
            [self_repair]
            started = "Starting..."
            done = "Done!"
            [errors]
            general = "Oops"
        "#;
        let table: toml::Table = toml_str.parse().unwrap();
        let mut out = HashMap::new();
        flatten_toml(&table, "", &mut out);

        assert_eq!(out.get("self_repair.started").unwrap(), "Starting...");
        assert_eq!(out.get("self_repair.done").unwrap(), "Done!");
        assert_eq!(out.get("errors.general").unwrap(), "Oops");
    }

    #[test]
    fn safe_truncate_ascii() {
        assert_eq!(safe_truncate("hello", 10), "hello");
        assert_eq!(safe_truncate("hello world", 5), "hello...");
    }

    #[test]
    fn safe_truncate_multibyte() {
        // "代码" = 6 bytes. Truncating at 4 should back up to 3 (end of "代").
        let s = "代码提交";
        let result = safe_truncate(s, 4);
        assert!(result.is_char_boundary(result.len()));
        assert_eq!(result, "代...");
    }

    #[test]
    fn safe_prefix_multibyte() {
        let s = "代码提交";
        let result = safe_prefix(s, 4);
        assert_eq!(result, "代"); // 3 bytes, next char starts at 3
    }
}
