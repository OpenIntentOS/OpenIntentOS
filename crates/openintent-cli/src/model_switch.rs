//! Runtime model switching for the Telegram bot.
//!
//! Detects model-switching intent in user messages (natural language or
//! `/model` command) and hot-swaps the LLM provider without restarting.

use std::sync::Arc;

use openintent_agent::{LlmClient, LlmProvider};

/// Result of a successful model switch.
pub struct ModelSwitch {
    pub provider_name: String,
    pub model: String,
}

/// Known model aliases — maps user-friendly names to (provider, model, base_url).
const MODEL_ALIASES: &[(&str, &str, &str, &str)] = &[
    // (alias, provider_name, model_id, base_url)
    // Anthropic
    ("claude", "Anthropic", "claude-sonnet-4-20250514", "https://api.anthropic.com"),
    ("claude-sonnet", "Anthropic", "claude-sonnet-4-20250514", "https://api.anthropic.com"),
    ("sonnet", "Anthropic", "claude-sonnet-4-20250514", "https://api.anthropic.com"),
    ("claude-haiku", "Anthropic", "claude-haiku-4-5-20251001", "https://api.anthropic.com"),
    ("haiku", "Anthropic", "claude-haiku-4-5-20251001", "https://api.anthropic.com"),
    ("claude-opus", "Anthropic", "claude-opus-4-20250514", "https://api.anthropic.com"),
    ("opus", "Anthropic", "claude-opus-4-20250514", "https://api.anthropic.com"),
    // DeepSeek
    ("deepseek", "DeepSeek", "deepseek-chat", "https://api.deepseek.com/v1"),
    ("deepseek-chat", "DeepSeek", "deepseek-chat", "https://api.deepseek.com/v1"),
    ("deepseek-reasoner", "DeepSeek", "deepseek-reasoner", "https://api.deepseek.com/v1"),
    // OpenAI
    ("gpt-4o", "OpenAI", "gpt-4o", "https://api.openai.com/v1"),
    ("gpt4o", "OpenAI", "gpt-4o", "https://api.openai.com/v1"),
    ("gpt-4", "OpenAI", "gpt-4o", "https://api.openai.com/v1"),
    ("gpt", "OpenAI", "gpt-4o", "https://api.openai.com/v1"),
    ("o1", "OpenAI", "o1", "https://api.openai.com/v1"),
    ("o3", "OpenAI", "o3-mini", "https://api.openai.com/v1"),
    // Ollama (local)
    ("ollama", "Ollama", "qwen2.5:latest", "http://localhost:11434/v1"),
    ("qwen", "Ollama", "qwen2.5:latest", "http://localhost:11434/v1"),
    ("llama", "Ollama", "llama3:latest", "http://localhost:11434/v1"),
    ("local", "Ollama", "qwen2.5:latest", "http://localhost:11434/v1"),
];

/// Try to detect a model-switching intent in the user's message.
///
/// Supports:
/// - `/model <name>` — explicit command
/// - Natural language: "切换到deepseek", "switch to claude", "use gpt-4o", "用opus"
///
/// Returns `Some(ModelSwitch)` if a switch was performed, `None` otherwise.
pub fn try_switch_model(text: &str, llm: &Arc<LlmClient>) -> Option<ModelSwitch> {
    let lower = text.to_lowercase();
    let trimmed = lower.trim();

    // Extract the target model name from the message.
    let target = if let Some(rest) = trimmed.strip_prefix("/model") {
        // /model <name>
        rest.trim().to_string()
    } else {
        // Natural language patterns (multilingual).
        extract_model_target(trimmed)?
    };

    if target.is_empty() {
        return None;
    }

    // Look up the target in known aliases.
    let (_, provider_name, model_id, base_url) = MODEL_ALIASES
        .iter()
        .find(|(alias, _, _, _)| *alias == target)?;

    // Resolve the API key for the target provider.
    let api_key = match *provider_name {
        "Anthropic" => {
            crate::helpers::env_non_empty("ANTHROPIC_API_KEY")
                .or_else(|| crate::helpers::read_claude_code_keychain_token())
        }
        "OpenAI" => crate::helpers::env_non_empty("OPENAI_API_KEY"),
        "DeepSeek" => crate::helpers::env_non_empty("DEEPSEEK_API_KEY"),
        "Ollama" => Some("ollama".to_string()),
        _ => None,
    }?;

    // Determine the LlmProvider enum variant.
    let provider = match *provider_name {
        "Anthropic" => LlmProvider::Anthropic,
        _ => LlmProvider::OpenAI, // OpenAI, DeepSeek, Ollama all use OpenAI-compatible API
    };

    llm.update_api_key(api_key);
    llm.switch_provider(provider, base_url.to_string(), model_id.to_string());

    Some(ModelSwitch {
        provider_name: provider_name.to_string(),
        model: model_id.to_string(),
    })
}

/// Extract a model target name from natural language text.
///
/// Supports patterns like:
/// - "switch to X", "change to X", "use X"
/// - "切换到X", "换成X", "用X", "切换X模型", "使用X"
fn extract_model_target(text: &str) -> Option<String> {
    // English patterns
    let en_prefixes = [
        "switch to ",
        "switch model to ",
        "change to ",
        "change model to ",
        "use ",
        "use model ",
    ];
    for prefix in &en_prefixes {
        if let Some(rest) = text.strip_prefix(prefix) {
            let target = rest
                .trim()
                .trim_end_matches(" model")
                .trim_end_matches(" please");
            if !target.is_empty() && MODEL_ALIASES.iter().any(|(a, _, _, _)| *a == target) {
                return Some(target.to_string());
            }
        }
    }

    // Chinese patterns
    let zh_prefixes = ["切换到", "切换成", "换成", "换到", "切换", "使用", "用"];
    for prefix in &zh_prefixes {
        if let Some(rest) = text.strip_prefix(prefix) {
            let target = rest
                .trim()
                .trim_end_matches("模型")
                .trim_end_matches("吧")
                .trim_end_matches("呗")
                .trim();
            if !target.is_empty() && MODEL_ALIASES.iter().any(|(a, _, _, _)| *a == target) {
                return Some(target.to_string());
            }
        }
    }

    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn extract_english_patterns() {
        assert_eq!(extract_model_target("switch to deepseek"), Some("deepseek".into()));
        assert_eq!(extract_model_target("use claude"), Some("claude".into()));
        assert_eq!(extract_model_target("change to gpt-4o"), Some("gpt-4o".into()));
        assert_eq!(extract_model_target("use opus"), Some("opus".into()));
    }

    #[test]
    fn extract_chinese_patterns() {
        assert_eq!(extract_model_target("切换到deepseek"), Some("deepseek".into()));
        assert_eq!(extract_model_target("换成claude"), Some("claude".into()));
        assert_eq!(extract_model_target("用opus"), Some("opus".into()));
        assert_eq!(extract_model_target("切换deepseek模型"), Some("deepseek".into()));
        assert_eq!(extract_model_target("使用haiku"), Some("haiku".into()));
    }

    #[test]
    fn no_match_for_normal_text() {
        assert_eq!(extract_model_target("hello world"), None);
        assert_eq!(extract_model_target("what is deepseek"), None);
        assert_eq!(extract_model_target("帮我写代码"), None);
    }

    #[test]
    fn no_match_for_unknown_model() {
        assert_eq!(extract_model_target("switch to banana"), None);
        assert_eq!(extract_model_target("切换到unknown"), None);
    }
}
