//! Shared helper functions used across CLI subcommands.
//!
//! Includes tracing initialization, system prompt loading, LLM provider
//! resolution, and environment variable utilities.

use std::path::Path;

use openintent_agent::LlmClientConfig;
use tracing::info;
use tracing_subscriber::EnvFilter;

// ---------------------------------------------------------------------------
// Tracing
// ---------------------------------------------------------------------------

/// Initialize the tracing subscriber with the given default log level.
///
/// Uses `try_init` so it is safe to call from multiple entry points
/// (e.g., both `serve` and the bot task spawned inside it) — subsequent
/// calls are silently ignored if a subscriber is already installed.
pub fn init_tracing(default_level: &str) {
    let filter =
        EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new(default_level));

    let _ = tracing_subscriber::fmt()
        .with_env_filter(filter)
        .with_target(false)
        .compact()
        .try_init();
}

// ---------------------------------------------------------------------------
// System prompt
// ---------------------------------------------------------------------------

/// Load the system prompt by combining `config/IDENTITY.md` and
/// `config/SOUL.md`, then appending current date/time context.
///
/// Cascade order (matching OpenClaw's identity architecture):
///   1. IDENTITY.md — Who you are, capabilities, core identity
///   2. SOUL.md — How you behave, think, and communicate
///   3. Dynamic context — Current date/time
pub fn load_system_prompt() -> String {
    let mut prompt = String::with_capacity(4096);

    // 1. Identity layer.
    let identity_path = Path::new("config/IDENTITY.md");
    if identity_path.exists() {
        if let Ok(content) = std::fs::read_to_string(identity_path) {
            prompt.push_str(&content);
        }
    }

    if prompt.is_empty() {
        prompt.push_str(&default_system_prompt());
    }

    // 2. Soul layer (behavioral guidelines).
    let soul_path = Path::new("config/SOUL.md");
    if soul_path.exists() {
        if let Ok(content) = std::fs::read_to_string(soul_path) {
            prompt.push_str("\n\n");
            prompt.push_str(&content);
        }
    }

    // 3. Dynamic context: current date/time and recent activity.
    let now = chrono::Local::now();
    prompt.push_str(&format!(
        "\n\n## Current Date & Time\n\n{}\n",
        now.format("%Y-%m-%d %H:%M:%S %Z (%A)")
    ));

    // 4. Self-awareness: inject recent git activity so the bot knows
    //    what has been done to/by it recently.
    if let Some(git_context) = load_recent_git_activity() {
        prompt.push_str("\n\n## Recent Activity (from git log)\n\n");
        prompt.push_str(&git_context);
        prompt.push('\n');
    }

    prompt
}

/// Load recent git commits to give the bot self-awareness of its changes.
fn load_recent_git_activity() -> Option<String> {
    let output = std::process::Command::new("git")
        .args(["log", "--oneline", "--since=3 days ago", "-20"])
        .output()
        .ok()?;

    if !output.status.success() {
        return None;
    }

    let log = String::from_utf8(output.stdout).ok()?;
    let trimmed = log.trim();
    if trimmed.is_empty() {
        return None;
    }

    Some(format!(
        "These are YOUR recent changes (commits to your own codebase):\n```\n{trimmed}\n```\n\
         When a user asks what you've been doing, reference these commits."
    ))
}

/// The fallback system prompt used when no IDENTITY.md is found.
fn default_system_prompt() -> String {
    "You are OpenIntentOS, an AI-powered operating system assistant. \
     You have access to filesystem, shell, web search, web fetch, browser, \
     email, GitHub, Telegram, calendar, memory, and many other tools. \
     Use your tools actively to help users. Match the user's language. \
     Be thorough, specific, and actionable in your responses."
        .to_owned()
}

// ---------------------------------------------------------------------------
// LLM provider resolution
// ---------------------------------------------------------------------------

const DEFAULT_MODEL_ANTHROPIC: &str = "claude-sonnet-4-20250514";
const DEFAULT_MODEL_OPENAI: &str = "gpt-4o";
const DEFAULT_MODEL_DEEPSEEK: &str = "deepseek-chat";
const DEFAULT_MODEL_NVIDIA: &str = "qwen/qwen3.5-397b-a17b";
const DEFAULT_MODEL_GOOGLE: &str = "gemini-2.5-flash";
const DEFAULT_MODEL_OPENROUTER: &str = "anthropic/claude-sonnet-4";
const DEFAULT_MODEL_GROQ: &str = "llama-3.3-70b-versatile";
const DEFAULT_MODEL_XAI: &str = "grok-3";
const DEFAULT_MODEL_MISTRAL: &str = "mistral-large-latest";
const DEFAULT_MODEL_OLLAMA: &str = "qwen2.5:latest";
const DEFAULT_MODEL_SILICONFLOW: &str = "deepseek-ai/DeepSeek-V3";
const DEFAULT_MODEL_MOONSHOT: &str = "moonshot-v1-8k";
const DEFAULT_MODEL_ZHIPU: &str = "glm-4-flash";
const DEFAULT_MODEL_TONGYI: &str = "qwen-turbo";

const DEEPSEEK_BASE_URL: &str = "https://api.deepseek.com/v1";
const NVIDIA_BASE_URL: &str = "https://integrate.api.nvidia.com/v1";
const GOOGLE_BASE_URL: &str = "https://generativelanguage.googleapis.com/v1beta/openai";
const OPENROUTER_BASE_URL: &str = "https://openrouter.ai/api/v1";
const GROQ_BASE_URL: &str = "https://api.groq.com/openai/v1";
const XAI_BASE_URL: &str = "https://api.x.ai/v1";
const MISTRAL_BASE_URL: &str = "https://api.mistral.ai/v1";
const OLLAMA_BASE_URL: &str = "http://localhost:11434/v1";
const SILICONFLOW_BASE_URL: &str = "https://api.siliconflow.cn/v1";
const MOONSHOT_BASE_URL: &str = "https://api.moonshot.cn/v1";
const ZHIPU_BASE_URL: &str = "https://open.bigmodel.cn/api/paas/v4";
const TONGYI_BASE_URL: &str = "https://dashscope.aliyuncs.com/compatible-mode/v1";

/// Resolve which LLM provider, API key, and model to use.
///
/// Resolution order:
///
/// 1. If `OPENINTENT_PROVIDER` is set, use that provider explicitly.
/// 2. Otherwise, auto-detect based on available credentials:
///    `ANTHROPIC_API_KEY` -> `OPENAI_API_KEY` -> `DEEPSEEK_API_KEY` ->
///    `NVIDIA_API_KEY` -> `GOOGLE_API_KEY` -> `OPENROUTER_API_KEY` ->
///    `GROQ_API_KEY` -> `XAI_API_KEY` -> `MISTRAL_API_KEY` ->
///    Claude Code Keychain -> Ollama (no key).
///
/// The model can always be overridden with `OPENINTENT_MODEL`.
/// A custom base URL can be set with `OPENINTENT_API_BASE_URL`.
pub fn resolve_llm_config() -> LlmClientConfig {
    let explicit_provider = env_non_empty("OPENINTENT_PROVIDER");
    let model_override = env_non_empty("OPENINTENT_MODEL");
    let base_url_override = env_non_empty("OPENINTENT_API_BASE_URL");

    let try_anthropic = || -> Option<LlmClientConfig> {
        let key = env_non_empty("ANTHROPIC_API_KEY").or_else(|| {
            let token = read_claude_code_keychain_token()?;
            info!("using Claude Code OAuth token from macOS Keychain");
            Some(token)
        })?;
        let model = model_override
            .clone()
            .unwrap_or_else(|| DEFAULT_MODEL_ANTHROPIC.to_owned());
        let mut cfg = LlmClientConfig::anthropic(key, model);
        if let Some(ref url) = base_url_override {
            cfg.base_url = url.clone();
        }
        Some(cfg)
    };

    let try_openai = || -> Option<LlmClientConfig> {
        let key = env_non_empty("OPENAI_API_KEY")?;
        let model = model_override
            .clone()
            .unwrap_or_else(|| DEFAULT_MODEL_OPENAI.to_owned());
        let mut cfg = LlmClientConfig::openai(key, model);
        if let Some(ref url) = base_url_override {
            cfg.base_url = url.clone();
        }
        Some(cfg)
    };

    let try_deepseek = || -> Option<LlmClientConfig> {
        let key = env_non_empty("DEEPSEEK_API_KEY")?;
        let model = model_override
            .clone()
            .unwrap_or_else(|| DEFAULT_MODEL_DEEPSEEK.to_owned());
        let base = base_url_override
            .clone()
            .unwrap_or_else(|| DEEPSEEK_BASE_URL.to_owned());
        Some(LlmClientConfig::openai_compatible(key, model, base))
    };

    let try_nvidia = || -> Option<LlmClientConfig> {
        let key = env_non_empty("NVIDIA_API_KEY")?;
        let model = model_override
            .clone()
            .unwrap_or_else(|| DEFAULT_MODEL_NVIDIA.to_owned());
        let base = base_url_override
            .clone()
            .unwrap_or_else(|| NVIDIA_BASE_URL.to_owned());
        Some(LlmClientConfig::openai_compatible(key, model, base))
    };

    let try_google = || -> Option<LlmClientConfig> {
        let key = env_non_empty("GOOGLE_API_KEY")?;
        let model = model_override
            .clone()
            .unwrap_or_else(|| DEFAULT_MODEL_GOOGLE.to_owned());
        let base = base_url_override
            .clone()
            .unwrap_or_else(|| GOOGLE_BASE_URL.to_owned());
        Some(LlmClientConfig::openai_compatible(key, model, base))
    };

    let try_openrouter = || -> Option<LlmClientConfig> {
        let key = env_non_empty("OPENROUTER_API_KEY")?;
        let model = model_override
            .clone()
            .unwrap_or_else(|| DEFAULT_MODEL_OPENROUTER.to_owned());
        let base = base_url_override
            .clone()
            .unwrap_or_else(|| OPENROUTER_BASE_URL.to_owned());
        Some(LlmClientConfig::openai_compatible(key, model, base))
    };

    let try_groq = || -> Option<LlmClientConfig> {
        let key = env_non_empty("GROQ_API_KEY")?;
        let model = model_override
            .clone()
            .unwrap_or_else(|| DEFAULT_MODEL_GROQ.to_owned());
        let base = base_url_override
            .clone()
            .unwrap_or_else(|| GROQ_BASE_URL.to_owned());
        Some(LlmClientConfig::openai_compatible(key, model, base))
    };

    let try_xai = || -> Option<LlmClientConfig> {
        let key = env_non_empty("XAI_API_KEY")?;
        let model = model_override
            .clone()
            .unwrap_or_else(|| DEFAULT_MODEL_XAI.to_owned());
        let base = base_url_override
            .clone()
            .unwrap_or_else(|| XAI_BASE_URL.to_owned());
        Some(LlmClientConfig::openai_compatible(key, model, base))
    };

    let try_mistral = || -> Option<LlmClientConfig> {
        let key = env_non_empty("MISTRAL_API_KEY")?;
        let model = model_override
            .clone()
            .unwrap_or_else(|| DEFAULT_MODEL_MISTRAL.to_owned());
        let base = base_url_override
            .clone()
            .unwrap_or_else(|| MISTRAL_BASE_URL.to_owned());
        Some(LlmClientConfig::openai_compatible(key, model, base))
    };

    let try_siliconflow = || -> Option<LlmClientConfig> {
        let key = env_non_empty("SILICONFLOW_API_KEY")?;
        let model = model_override
            .clone()
            .unwrap_or_else(|| DEFAULT_MODEL_SILICONFLOW.to_owned());
        let base = base_url_override
            .clone()
            .unwrap_or_else(|| SILICONFLOW_BASE_URL.to_owned());
        Some(LlmClientConfig::openai_compatible(key, model, base))
    };

    let try_moonshot = || -> Option<LlmClientConfig> {
        let key = env_non_empty("MOONSHOT_API_KEY")?;
        let model = model_override
            .clone()
            .unwrap_or_else(|| DEFAULT_MODEL_MOONSHOT.to_owned());
        let base = base_url_override
            .clone()
            .unwrap_or_else(|| MOONSHOT_BASE_URL.to_owned());
        Some(LlmClientConfig::openai_compatible(key, model, base))
    };

    let try_zhipu = || -> Option<LlmClientConfig> {
        let key = env_non_empty("ZHIPU_API_KEY")?;
        let model = model_override
            .clone()
            .unwrap_or_else(|| DEFAULT_MODEL_ZHIPU.to_owned());
        let base = base_url_override
            .clone()
            .unwrap_or_else(|| ZHIPU_BASE_URL.to_owned());
        Some(LlmClientConfig::openai_compatible(key, model, base))
    };

    let try_tongyi = || -> Option<LlmClientConfig> {
        let key = env_non_empty("DASHSCOPE_API_KEY")?;
        let model = model_override
            .clone()
            .unwrap_or_else(|| DEFAULT_MODEL_TONGYI.to_owned());
        let base = base_url_override
            .clone()
            .unwrap_or_else(|| TONGYI_BASE_URL.to_owned());
        Some(LlmClientConfig::openai_compatible(key, model, base))
    };

    let try_ollama = || -> LlmClientConfig {
        let model = model_override
            .clone()
            .unwrap_or_else(|| DEFAULT_MODEL_OLLAMA.to_owned());
        let base = base_url_override
            .clone()
            .unwrap_or_else(|| OLLAMA_BASE_URL.to_owned());
        LlmClientConfig::openai_compatible("ollama", model, base)
    };

    // 1. Explicit provider selection.
    if let Some(ref provider) = explicit_provider {
        let p = provider.to_lowercase();
        return match p.as_str() {
            "anthropic" | "claude" => try_anthropic().unwrap_or_else(|| {
                exit_no_key("anthropic", "ANTHROPIC_API_KEY");
            }),
            "openai" | "gpt" => try_openai().unwrap_or_else(|| {
                exit_no_key("openai", "OPENAI_API_KEY");
            }),
            "deepseek" => try_deepseek().unwrap_or_else(|| {
                exit_no_key("deepseek", "DEEPSEEK_API_KEY");
            }),
            "nvidia" | "nim" => try_nvidia().unwrap_or_else(|| {
                exit_no_key("nvidia", "NVIDIA_API_KEY");
            }),
            "google" | "gemini" => try_google().unwrap_or_else(|| {
                exit_no_key("google", "GOOGLE_API_KEY");
            }),
            "openrouter" => try_openrouter().unwrap_or_else(|| {
                exit_no_key("openrouter", "OPENROUTER_API_KEY");
            }),
            "groq" => try_groq().unwrap_or_else(|| {
                exit_no_key("groq", "GROQ_API_KEY");
            }),
            "xai" | "grok" => try_xai().unwrap_or_else(|| {
                exit_no_key("xai", "XAI_API_KEY");
            }),
            "mistral" => try_mistral().unwrap_or_else(|| {
                exit_no_key("mistral", "MISTRAL_API_KEY");
            }),
            "siliconflow" | "sf" => try_siliconflow().unwrap_or_else(|| {
                exit_no_key("siliconflow", "SILICONFLOW_API_KEY");
            }),
            "moonshot" | "kimi-cn" => try_moonshot().unwrap_or_else(|| {
                exit_no_key("moonshot", "MOONSHOT_API_KEY");
            }),
            "zhipu" | "glm" => try_zhipu().unwrap_or_else(|| {
                exit_no_key("zhipu", "ZHIPU_API_KEY");
            }),
            "tongyi" | "qwen-cn" | "dashscope" => try_tongyi().unwrap_or_else(|| {
                exit_no_key("tongyi", "DASHSCOPE_API_KEY");
            }),
            "ollama" | "local" => try_ollama(),
            "chatgpt-web" | "chatgpt-pro" | "chatgpt" => {
                let token = env_non_empty("CHATGPT_SESSION_TOKEN").unwrap_or_else(|| {
                    exit_no_key("chatgpt-web", "CHATGPT_SESSION_TOKEN");
                });
                let model = model_override.unwrap_or_else(|| "gpt-4".to_owned());
                LlmClientConfig::chatgpt_web(token, model)
            }
            _ => {
                let key = env_non_empty("OPENINTENT_API_KEY")
                    .or_else(|| env_non_empty("OPENAI_API_KEY"))
                    .unwrap_or_else(|| "no-key".to_owned());
                let model = model_override.unwrap_or_else(|| p.clone());
                let base = base_url_override.unwrap_or_else(|| {
                    eprintln!("  Error: OPENINTENT_API_BASE_URL is required for provider '{p}'");
                    std::process::exit(1);
                });
                LlmClientConfig::openai_compatible(key, model, base)
            }
        };
    }

    // 2. Auto-detect from available credentials.
    if let Some(cfg) = try_anthropic() {
        return cfg;
    }
    if let Some(cfg) = try_openai() {
        return cfg;
    }
    if let Some(cfg) = try_deepseek() {
        return cfg;
    }
    if let Some(cfg) = try_nvidia() {
        return cfg;
    }
    if let Some(cfg) = try_google() {
        return cfg;
    }
    if let Some(cfg) = try_openrouter() {
        return cfg;
    }
    if let Some(cfg) = try_groq() {
        return cfg;
    }
    if let Some(cfg) = try_xai() {
        return cfg;
    }
    if let Some(cfg) = try_mistral() {
        return cfg;
    }
    if let Some(cfg) = try_siliconflow() {
        return cfg;
    }
    if let Some(cfg) = try_moonshot() {
        return cfg;
    }
    if let Some(cfg) = try_zhipu() {
        return cfg;
    }
    if let Some(cfg) = try_tongyi() {
        return cfg;
    }

    // 3. Last resort: try Ollama (local, no key needed).
    info!("no API key found, falling back to Ollama local model");
    try_ollama()
}

/// Read a non-empty environment variable, returning `None` if unset or empty.
pub fn env_non_empty(name: &str) -> Option<String> {
    std::env::var(name).ok().filter(|v| !v.is_empty())
}

/// Print an error about a missing API key and exit.
fn exit_no_key(provider: &str, env_var: &str) -> ! {
    eprintln!();
    eprintln!("  Error: {provider} provider selected but no API key found.");
    eprintln!("  Set it in your environment:");
    eprintln!("    export {env_var}=...");
    eprintln!();
    std::process::exit(1);
}

/// Attempt to read the Claude Code OAuth access token from the macOS Keychain.
pub fn read_claude_code_keychain_token() -> Option<String> {
    if !cfg!(target_os = "macos") {
        return None;
    }

    let output = std::process::Command::new("security")
        .args([
            "find-generic-password",
            "-s",
            "Claude Code-credentials",
            "-w",
        ])
        .output()
        .ok()?;

    if !output.status.success() {
        return None;
    }

    let json_str = String::from_utf8(output.stdout).ok()?;
    let json: serde_json::Value = serde_json::from_str(json_str.trim()).ok()?;

    json.get("claudeAiOauth")
        .and_then(|oauth| oauth.get("accessToken"))
        .and_then(|v| v.as_str())
        .filter(|s| !s.is_empty())
        .map(|s| s.to_owned())
}

// ---------------------------------------------------------------------------
// JWT utilities
// ---------------------------------------------------------------------------

/// Decode the `exp` claim from a JWT (without signature verification) and
/// return whether the token has already expired.
///
/// Returns `true` if the token is definitely expired.
/// Returns `false` if it is valid or if the expiry cannot be determined.
pub fn is_jwt_expired(token: &str) -> bool {
    let payload = match token.split('.').nth(1) {
        Some(p) => p,
        None => return false,
    };

    // Base64url decode (no padding required for this use).
    let decoded = {
        let engine = base64::engine::general_purpose::URL_SAFE_NO_PAD;
        match base64::Engine::decode(&engine, payload) {
            Ok(b) => b,
            Err(_) => return false,
        }
    };

    let v: serde_json::Value = match serde_json::from_slice(&decoded) {
        Ok(v) => v,
        Err(_) => return false,
    };

    let exp = match v["exp"].as_i64() {
        Some(e) => e,
        None => return false,
    };

    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0);

    now > exp
}

/// Return the remaining lifetime of a JWT in seconds, or `None` if
/// the token cannot be decoded or has no `exp` claim.
pub fn jwt_seconds_remaining(token: &str) -> Option<i64> {
    let payload = token.split('.').nth(1)?;
    let engine = base64::engine::general_purpose::URL_SAFE_NO_PAD;
    let decoded = base64::Engine::decode(&engine, payload).ok()?;
    let v: serde_json::Value = serde_json::from_slice(&decoded).ok()?;
    let exp = v["exp"].as_i64()?;
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0);
    Some(exp - now)
}
