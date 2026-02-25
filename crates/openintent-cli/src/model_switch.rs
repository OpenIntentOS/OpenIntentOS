//! Runtime model switching for the Telegram bot.
//!
//! Detects model-switching intent in user messages (natural language or
//! `/model` command) and hot-swaps the LLM provider without restarting.
//!
//! Supports:
//! - Direct providers: Anthropic, OpenAI, DeepSeek, NVIDIA, Google Gemini, Groq, xAI, Mistral, Ollama
//! - OpenRouter: universal access to 700+ models via `provider/model` format
//! - Short aliases: "claude", "gpt", "gemini", "grok", "nvidia", "llama", etc.

use std::sync::Arc;

use openintent_agent::{LlmClient, LlmProvider};

// ---------------------------------------------------------------------------
// Constants
// ---------------------------------------------------------------------------

const OPENROUTER_BASE_URL: &str = "https://openrouter.ai/api/v1";
const DEEPSEEK_BASE_URL: &str = "https://api.deepseek.com/v1";
const NVIDIA_BASE_URL: &str = "https://integrate.api.nvidia.com/v1";
const GOOGLE_BASE_URL: &str = "https://generativelanguage.googleapis.com/v1beta/openai";
const GROQ_BASE_URL: &str = "https://api.groq.com/openai/v1";
const XAI_BASE_URL: &str = "https://api.x.ai/v1";
const MISTRAL_BASE_URL: &str = "https://api.mistral.ai/v1";
const OLLAMA_BASE_URL: &str = "http://localhost:11434/v1";

// ---------------------------------------------------------------------------
// Types
// ---------------------------------------------------------------------------

/// Result of a successful model switch.
pub struct ModelSwitch {
    pub provider_name: String,
    pub model: String,
}

/// Entry in the alias table.
/// (alias, provider_name, model_id, base_url, env_var_for_key)
struct AliasEntry {
    alias: &'static str,
    provider: &'static str,
    model: &'static str,
    base_url: &'static str,
    key_env: &'static str,
}

// ---------------------------------------------------------------------------
// Alias table
// ---------------------------------------------------------------------------

/// Known model aliases — maps user-friendly names to provider config.
///
/// Ordered by priority within each provider group. The first match wins.
const ALIASES: &[AliasEntry] = &[
    // -- Anthropic (direct) ---------------------------------------------------
    AliasEntry { alias: "claude", provider: "Anthropic", model: "claude-sonnet-4-20250514", base_url: "https://api.anthropic.com", key_env: "ANTHROPIC_API_KEY" },
    AliasEntry { alias: "claude-sonnet", provider: "Anthropic", model: "claude-sonnet-4-20250514", base_url: "https://api.anthropic.com", key_env: "ANTHROPIC_API_KEY" },
    AliasEntry { alias: "sonnet", provider: "Anthropic", model: "claude-sonnet-4-20250514", base_url: "https://api.anthropic.com", key_env: "ANTHROPIC_API_KEY" },
    AliasEntry { alias: "claude-haiku", provider: "Anthropic", model: "claude-haiku-4-5-20251001", base_url: "https://api.anthropic.com", key_env: "ANTHROPIC_API_KEY" },
    AliasEntry { alias: "haiku", provider: "Anthropic", model: "claude-haiku-4-5-20251001", base_url: "https://api.anthropic.com", key_env: "ANTHROPIC_API_KEY" },
    AliasEntry { alias: "claude-opus", provider: "Anthropic", model: "claude-opus-4-20250514", base_url: "https://api.anthropic.com", key_env: "ANTHROPIC_API_KEY" },
    AliasEntry { alias: "opus", provider: "Anthropic", model: "claude-opus-4-20250514", base_url: "https://api.anthropic.com", key_env: "ANTHROPIC_API_KEY" },

    // -- OpenAI (direct) ------------------------------------------------------
    AliasEntry { alias: "gpt-4o", provider: "OpenAI", model: "gpt-4o", base_url: "https://api.openai.com/v1", key_env: "OPENAI_API_KEY" },
    AliasEntry { alias: "gpt4o", provider: "OpenAI", model: "gpt-4o", base_url: "https://api.openai.com/v1", key_env: "OPENAI_API_KEY" },
    AliasEntry { alias: "gpt-4", provider: "OpenAI", model: "gpt-4o", base_url: "https://api.openai.com/v1", key_env: "OPENAI_API_KEY" },
    AliasEntry { alias: "gpt", provider: "OpenAI", model: "gpt-4o", base_url: "https://api.openai.com/v1", key_env: "OPENAI_API_KEY" },
    AliasEntry { alias: "o1", provider: "OpenAI", model: "o1", base_url: "https://api.openai.com/v1", key_env: "OPENAI_API_KEY" },
    AliasEntry { alias: "o3", provider: "OpenAI", model: "o3-mini", base_url: "https://api.openai.com/v1", key_env: "OPENAI_API_KEY" },
    AliasEntry { alias: "o4-mini", provider: "OpenAI", model: "o4-mini", base_url: "https://api.openai.com/v1", key_env: "OPENAI_API_KEY" },
    AliasEntry { alias: "codex", provider: "OpenAI", model: "codex-mini-latest", base_url: "https://api.openai.com/v1", key_env: "OPENAI_API_KEY" },

    // -- DeepSeek (direct) ----------------------------------------------------
    AliasEntry { alias: "deepseek", provider: "DeepSeek", model: "deepseek-chat", base_url: DEEPSEEK_BASE_URL, key_env: "DEEPSEEK_API_KEY" },
    AliasEntry { alias: "deepseek-chat", provider: "DeepSeek", model: "deepseek-chat", base_url: DEEPSEEK_BASE_URL, key_env: "DEEPSEEK_API_KEY" },
    AliasEntry { alias: "deepseek-reasoner", provider: "DeepSeek", model: "deepseek-reasoner", base_url: DEEPSEEK_BASE_URL, key_env: "DEEPSEEK_API_KEY" },

    // -- NVIDIA NIM (direct, free tier) ------------------------------------------
    AliasEntry { alias: "nvidia", provider: "NVIDIA", model: "qwen/qwen3.5-397b-a17b", base_url: NVIDIA_BASE_URL, key_env: "NVIDIA_API_KEY" },
    AliasEntry { alias: "nvidia-qwen", provider: "NVIDIA", model: "qwen/qwen3.5-397b-a17b", base_url: NVIDIA_BASE_URL, key_env: "NVIDIA_API_KEY" },
    AliasEntry { alias: "nvidia-kimi", provider: "NVIDIA", model: "moonshotai/kimi-k2.5", base_url: NVIDIA_BASE_URL, key_env: "NVIDIA_API_KEY" },
    AliasEntry { alias: "kimi", provider: "NVIDIA", model: "moonshotai/kimi-k2.5", base_url: NVIDIA_BASE_URL, key_env: "NVIDIA_API_KEY" },
    AliasEntry { alias: "nemotron", provider: "NVIDIA", model: "nvidia/nemotron-3-nano-30b-a3b", base_url: NVIDIA_BASE_URL, key_env: "NVIDIA_API_KEY" },
    AliasEntry { alias: "nemotron-nano", provider: "NVIDIA", model: "nvidia/nemotron-3-nano-30b-a3b", base_url: NVIDIA_BASE_URL, key_env: "NVIDIA_API_KEY" },
    AliasEntry { alias: "nvidia-cosmos", provider: "NVIDIA", model: "nvidia/cosmos-reason2-8b", base_url: NVIDIA_BASE_URL, key_env: "NVIDIA_API_KEY" },

    // -- Groq (direct) --------------------------------------------------------
    AliasEntry { alias: "groq", provider: "Groq", model: "llama-3.3-70b-versatile", base_url: GROQ_BASE_URL, key_env: "GROQ_API_KEY" },
    AliasEntry { alias: "groq-llama", provider: "Groq", model: "llama-3.3-70b-versatile", base_url: GROQ_BASE_URL, key_env: "GROQ_API_KEY" },
    AliasEntry { alias: "groq-mixtral", provider: "Groq", model: "mixtral-8x7b-32768", base_url: GROQ_BASE_URL, key_env: "GROQ_API_KEY" },

    // -- xAI / Grok (direct) --------------------------------------------------
    AliasEntry { alias: "grok", provider: "xAI", model: "grok-3", base_url: XAI_BASE_URL, key_env: "XAI_API_KEY" },
    AliasEntry { alias: "grok-3", provider: "xAI", model: "grok-3", base_url: XAI_BASE_URL, key_env: "XAI_API_KEY" },
    AliasEntry { alias: "grok-mini", provider: "xAI", model: "grok-3-mini", base_url: XAI_BASE_URL, key_env: "XAI_API_KEY" },
    AliasEntry { alias: "xai", provider: "xAI", model: "grok-3", base_url: XAI_BASE_URL, key_env: "XAI_API_KEY" },

    // -- Mistral (direct) -----------------------------------------------------
    AliasEntry { alias: "mistral", provider: "Mistral", model: "mistral-large-latest", base_url: MISTRAL_BASE_URL, key_env: "MISTRAL_API_KEY" },
    AliasEntry { alias: "mistral-large", provider: "Mistral", model: "mistral-large-latest", base_url: MISTRAL_BASE_URL, key_env: "MISTRAL_API_KEY" },
    AliasEntry { alias: "mistral-small", provider: "Mistral", model: "mistral-small-latest", base_url: MISTRAL_BASE_URL, key_env: "MISTRAL_API_KEY" },
    AliasEntry { alias: "codestral", provider: "Mistral", model: "codestral-latest", base_url: MISTRAL_BASE_URL, key_env: "MISTRAL_API_KEY" },

    // -- Google Gemini (direct) ------------------------------------------------
    AliasEntry { alias: "gemini", provider: "Google", model: "gemini-2.5-pro", base_url: GOOGLE_BASE_URL, key_env: "GOOGLE_API_KEY" },
    AliasEntry { alias: "gemini-pro", provider: "Google", model: "gemini-2.5-pro", base_url: GOOGLE_BASE_URL, key_env: "GOOGLE_API_KEY" },
    AliasEntry { alias: "gemini-flash", provider: "Google", model: "gemini-2.5-flash", base_url: GOOGLE_BASE_URL, key_env: "GOOGLE_API_KEY" },
    AliasEntry { alias: "gemini-2.0-flash", provider: "Google", model: "gemini-2.0-flash", base_url: GOOGLE_BASE_URL, key_env: "GOOGLE_API_KEY" },
    AliasEntry { alias: "gemini-lite", provider: "Google", model: "gemini-2.0-flash-lite", base_url: GOOGLE_BASE_URL, key_env: "GOOGLE_API_KEY" },

    // -- Popular models via OpenRouter ----------------------------------------
    AliasEntry { alias: "llama", provider: "OpenRouter", model: "meta-llama/llama-4-maverick", base_url: OPENROUTER_BASE_URL, key_env: "OPENROUTER_API_KEY" },
    AliasEntry { alias: "llama-70b", provider: "OpenRouter", model: "meta-llama/llama-3.3-70b-instruct", base_url: OPENROUTER_BASE_URL, key_env: "OPENROUTER_API_KEY" },
    AliasEntry { alias: "qwen", provider: "OpenRouter", model: "qwen/qwen-2.5-coder-32b-instruct", base_url: OPENROUTER_BASE_URL, key_env: "OPENROUTER_API_KEY" },
    AliasEntry { alias: "command-r", provider: "OpenRouter", model: "cohere/command-r-plus", base_url: OPENROUTER_BASE_URL, key_env: "OPENROUTER_API_KEY" },

    // -- Ollama (local, no key needed) ----------------------------------------
    AliasEntry { alias: "ollama", provider: "Ollama", model: "qwen2.5:latest", base_url: OLLAMA_BASE_URL, key_env: "" },
    AliasEntry { alias: "local", provider: "Ollama", model: "qwen2.5:latest", base_url: OLLAMA_BASE_URL, key_env: "" },
];

// ---------------------------------------------------------------------------
// Provider list for /models
// ---------------------------------------------------------------------------

/// Available provider categories for the /models inline keyboard.
pub const PROVIDER_GROUPS: &[(&str, &[(&str, &str)])] = &[
    ("Anthropic", &[
        ("Claude Sonnet 4", "claude-sonnet"),
        ("Claude Haiku 4.5", "haiku"),
        ("Claude Opus 4", "opus"),
    ]),
    ("OpenAI", &[
        ("GPT-4o", "gpt-4o"),
        ("o1", "o1"),
        ("o3-mini", "o3"),
        ("o4-mini", "o4-mini"),
        ("Codex", "codex"),
    ]),
    ("DeepSeek", &[
        ("DeepSeek Chat", "deepseek"),
        ("DeepSeek Reasoner", "deepseek-reasoner"),
    ]),
    ("Google Gemini", &[
        ("Gemini 2.5 Pro", "gemini-pro"),
        ("Gemini 2.5 Flash", "gemini-flash"),
        ("Gemini 2.0 Flash", "gemini-2.0-flash"),
        ("Gemini 2.0 Lite", "gemini-lite"),
    ]),
    ("NVIDIA NIM (free)", &[
        ("Qwen 3.5 397B", "nvidia-qwen"),
        ("Kimi K2.5", "nvidia-kimi"),
        ("Nemotron 3 Nano", "nemotron-nano"),
        ("Cosmos Reason2", "nvidia-cosmos"),
    ]),
    ("Groq", &[
        ("Llama 3.3 70B", "groq-llama"),
        ("Mixtral 8x7B", "groq-mixtral"),
    ]),
    ("xAI", &[
        ("Grok 3", "grok-3"),
        ("Grok 3 Mini", "grok-mini"),
    ]),
    ("Mistral", &[
        ("Mistral Large", "mistral-large"),
        ("Mistral Small", "mistral-small"),
        ("Codestral", "codestral"),
    ]),
    ("Meta (OpenRouter)", &[
        ("Llama 4 Maverick", "llama"),
        ("Llama 3.3 70B", "llama-70b"),
    ]),
    ("Ollama (local)", &[
        ("Qwen 2.5", "ollama"),
    ]),
];

// ---------------------------------------------------------------------------
// Core switch logic
// ---------------------------------------------------------------------------

/// Try to detect a model-switching intent in the user's message.
///
/// Supports:
/// - `/model <name>` — explicit command with alias or `provider/model` ID
/// - Natural language: "switch to deepseek", "切换到gemini", "use grok", "用ollama"
///
/// Returns `Some(ModelSwitch)` if a switch was performed, `None` otherwise.
pub fn try_switch_model(text: &str, llm: &Arc<LlmClient>) -> Option<ModelSwitch> {
    let lower = text.to_lowercase();
    let trimmed = lower.trim();

    // Extract the target model name from the message.
    let target = if let Some(rest) = trimmed.strip_prefix("/model") {
        rest.trim().to_string()
    } else {
        extract_model_target(trimmed)?
    };

    if target.is_empty() {
        return None;
    }

    // 1. Try known aliases first.
    if let Some(result) = try_alias_switch(&target, llm) {
        return Some(result);
    }

    // 2. If target contains '/', treat as `provider/model` format.
    //    First check direct providers (ollama/qwen, deepseek/chat, etc.),
    //    then fall back to OpenRouter.
    if target.contains('/') {
        if let Some(result) = try_slash_format(&target, llm) {
            return Some(result);
        }
    }

    None
}

/// Switch using a known alias from the ALIASES table.
fn try_alias_switch(target: &str, llm: &Arc<LlmClient>) -> Option<ModelSwitch> {
    let entry = ALIASES.iter().find(|e| e.alias == target)?;

    let api_key = resolve_api_key(entry.provider, entry.key_env)?;
    let provider = to_llm_provider(entry.provider);

    llm.update_api_key(api_key);
    llm.switch_provider(provider, entry.base_url.to_string(), entry.model.to_string());

    Some(ModelSwitch {
        provider_name: entry.provider.to_string(),
        model: entry.model.to_string(),
    })
}

/// Handle `provider/model` format — route to direct providers first, then OpenRouter.
///
/// Examples:
/// - `ollama/qwen2.5` → Ollama at localhost:11434
/// - `deepseek/deepseek-reasoner` → DeepSeek API
/// - `groq/llama-3.3-70b` → Groq API
/// - `google/gemini-2.5-pro` → Google Gemini direct API
fn try_slash_format(target: &str, llm: &Arc<LlmClient>) -> Option<ModelSwitch> {
    let (provider_prefix, model_name) = target.split_once('/')?;
    if model_name.is_empty() {
        return None;
    }

    // Direct provider routing.
    match provider_prefix {
        "ollama" | "local" => {
            llm.update_api_key("ollama".to_string());
            llm.switch_provider(
                LlmProvider::OpenAI,
                OLLAMA_BASE_URL.to_string(),
                model_name.to_string(),
            );
            return Some(ModelSwitch {
                provider_name: "Ollama".to_string(),
                model: model_name.to_string(),
            });
        }
        "deepseek" => {
            let key = crate::helpers::env_non_empty("DEEPSEEK_API_KEY")?;
            llm.update_api_key(key);
            llm.switch_provider(
                LlmProvider::OpenAI,
                DEEPSEEK_BASE_URL.to_string(),
                model_name.to_string(),
            );
            return Some(ModelSwitch {
                provider_name: "DeepSeek".to_string(),
                model: model_name.to_string(),
            });
        }
        "nvidia" => {
            let key = crate::helpers::env_non_empty("NVIDIA_API_KEY")?;
            llm.update_api_key(key);
            // NVIDIA uses provider/model format in model IDs.
            llm.switch_provider(
                LlmProvider::OpenAI,
                NVIDIA_BASE_URL.to_string(),
                model_name.to_string(),
            );
            return Some(ModelSwitch {
                provider_name: "NVIDIA".to_string(),
                model: model_name.to_string(),
            });
        }
        "google" | "gemini" => {
            let key = crate::helpers::env_non_empty("GOOGLE_API_KEY")?;
            llm.update_api_key(key);
            llm.switch_provider(
                LlmProvider::OpenAI,
                GOOGLE_BASE_URL.to_string(),
                model_name.to_string(),
            );
            return Some(ModelSwitch {
                provider_name: "Google".to_string(),
                model: model_name.to_string(),
            });
        }
        "groq" => {
            let key = crate::helpers::env_non_empty("GROQ_API_KEY")?;
            llm.update_api_key(key);
            llm.switch_provider(
                LlmProvider::OpenAI,
                GROQ_BASE_URL.to_string(),
                model_name.to_string(),
            );
            return Some(ModelSwitch {
                provider_name: "Groq".to_string(),
                model: model_name.to_string(),
            });
        }
        "xai" => {
            let key = crate::helpers::env_non_empty("XAI_API_KEY")?;
            llm.update_api_key(key);
            llm.switch_provider(
                LlmProvider::OpenAI,
                XAI_BASE_URL.to_string(),
                model_name.to_string(),
            );
            return Some(ModelSwitch {
                provider_name: "xAI".to_string(),
                model: model_name.to_string(),
            });
        }
        "mistral" => {
            let key = crate::helpers::env_non_empty("MISTRAL_API_KEY")?;
            llm.update_api_key(key);
            llm.switch_provider(
                LlmProvider::OpenAI,
                MISTRAL_BASE_URL.to_string(),
                model_name.to_string(),
            );
            return Some(ModelSwitch {
                provider_name: "Mistral".to_string(),
                model: model_name.to_string(),
            });
        }
        "openai" => {
            let key = crate::helpers::env_non_empty("OPENAI_API_KEY")?;
            llm.update_api_key(key);
            llm.switch_provider(
                LlmProvider::OpenAI,
                "https://api.openai.com/v1".to_string(),
                model_name.to_string(),
            );
            return Some(ModelSwitch {
                provider_name: "OpenAI".to_string(),
                model: model_name.to_string(),
            });
        }
        "anthropic" => {
            let key = crate::helpers::env_non_empty("ANTHROPIC_API_KEY")
                .or_else(|| crate::helpers::read_claude_code_keychain_token())?;
            llm.update_api_key(key);
            llm.switch_provider(
                LlmProvider::Anthropic,
                "https://api.anthropic.com".to_string(),
                model_name.to_string(),
            );
            return Some(ModelSwitch {
                provider_name: "Anthropic".to_string(),
                model: model_name.to_string(),
            });
        }
        _ => {}
    }

    // Fall back to OpenRouter for unknown providers (google, meta-llama, etc.)
    let api_key = crate::helpers::env_non_empty("OPENROUTER_API_KEY")?;
    llm.update_api_key(api_key);
    llm.switch_provider(
        LlmProvider::OpenAI,
        OPENROUTER_BASE_URL.to_string(),
        target.to_string(),
    );

    Some(ModelSwitch {
        provider_name: provider_prefix.to_string(),
        model: target.to_string(),
    })
}

/// Resolve the API key for a given provider.
fn resolve_api_key(provider: &str, key_env: &str) -> Option<String> {
    match provider {
        "Anthropic" => {
            crate::helpers::env_non_empty("ANTHROPIC_API_KEY")
                .or_else(|| crate::helpers::read_claude_code_keychain_token())
        }
        "Ollama" => Some("ollama".to_string()),
        "NVIDIA" => crate::helpers::env_non_empty("NVIDIA_API_KEY"),
        "Google" => crate::helpers::env_non_empty("GOOGLE_API_KEY"),
        "OpenRouter" => crate::helpers::env_non_empty("OPENROUTER_API_KEY"),
        _ => crate::helpers::env_non_empty(key_env),
    }
}

/// Map provider name to the LlmProvider enum variant.
fn to_llm_provider(provider: &str) -> LlmProvider {
    match provider {
        "Anthropic" => LlmProvider::Anthropic,
        _ => LlmProvider::OpenAI, // All others use OpenAI-compatible API
    }
}

// ---------------------------------------------------------------------------
// Natural language intent extraction
// ---------------------------------------------------------------------------

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
            if !target.is_empty() && is_valid_target(target) {
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
            if !target.is_empty() && is_valid_target(target) {
                return Some(target.to_string());
            }
        }
    }

    None
}

/// Check if a target is a known alias or a valid `provider/model` format.
fn is_valid_target(target: &str) -> bool {
    ALIASES.iter().any(|e| e.alias == target) || target.contains('/')
}

/// Build a Telegram inline keyboard for `/models` showing available providers.
///
/// Returns a `serde_json::Value` representing the `reply_markup` field.
pub fn build_models_keyboard() -> serde_json::Value {
    let mut rows: Vec<serde_json::Value> = Vec::new();

    for (group_name, models) in PROVIDER_GROUPS {
        let mut row: Vec<serde_json::Value> = Vec::new();
        for (display_name, alias) in *models {
            // Check if the API key is available for this alias.
            let available = ALIASES
                .iter()
                .find(|e| e.alias == *alias)
                .map(|e| resolve_api_key(e.provider, e.key_env).is_some())
                .unwrap_or(false);

            let label = if available {
                format!("{display_name}")
            } else {
                format!("{display_name} [no key]")
            };

            row.push(serde_json::json!({
                "text": label,
                "callback_data": format!("model:{alias}"),
            }));

            // Max 3 buttons per row for readability.
            if row.len() >= 3 {
                rows.push(serde_json::json!(row));
                row = Vec::new();
            }
        }
        if !row.is_empty() {
            rows.push(serde_json::json!(row));
        }

        // Add a separator label row for the group.
        // Insert BEFORE the model buttons for this group.
        let group_label_row = serde_json::json!([{
            "text": format!("--- {group_name} ---"),
            "callback_data": "noop",
        }]);
        let insert_pos = rows.len().saturating_sub(
            (*models).len().div_ceil(3),
        );
        rows.insert(insert_pos, group_label_row);
    }

    serde_json::json!({
        "inline_keyboard": rows,
    })
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

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
    fn extract_openrouter_slash_format() {
        assert_eq!(
            extract_model_target("switch to google/gemini-2.5-pro"),
            Some("google/gemini-2.5-pro".into())
        );
        assert_eq!(
            extract_model_target("用meta-llama/llama-3.3-70b-instruct"),
            Some("meta-llama/llama-3.3-70b-instruct".into())
        );
    }

    #[test]
    fn extract_new_providers() {
        assert_eq!(extract_model_target("switch to grok"), Some("grok".into()));
        assert_eq!(extract_model_target("use groq"), Some("groq".into()));
        assert_eq!(extract_model_target("切换到mistral"), Some("mistral".into()));
        assert_eq!(extract_model_target("switch to gemini"), Some("gemini".into()));
        assert_eq!(extract_model_target("switch to nvidia"), Some("nvidia".into()));
        assert_eq!(extract_model_target("use nemotron"), Some("nemotron".into()));
        assert_eq!(extract_model_target("切换到nvidia-kimi"), Some("nvidia-kimi".into()));
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

    #[test]
    fn is_valid_target_works() {
        assert!(is_valid_target("claude"));
        assert!(is_valid_target("grok"));
        assert!(is_valid_target("groq"));
        assert!(is_valid_target("google/gemini-2.5-pro"));
        assert!(!is_valid_target("banana"));
        assert!(!is_valid_target(""));
    }
}
