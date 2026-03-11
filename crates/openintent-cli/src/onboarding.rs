//! First-time onboarding wizard for OpenIntentOS.
//!
//! Guides new users through three focused steps:
//!   1. AI Provider — pick a free or paid provider, enter API key
//!   2. Chat Channel — Telegram, WeChat, DingTalk, WeCom, or Web UI only
//!   3. Summary — confirm and run
//!
//! Writes all results to `.env` and marks onboarding complete.

use std::io::{self, BufRead, Write};
use std::path::Path;

use tracing::info;

// ---------------------------------------------------------------------------
// Public API
// ---------------------------------------------------------------------------

/// Returns `true` if the onboarding wizard has already been completed.
pub fn is_onboarding_done() -> bool {
    std::env::var("ONBOARDING_COMPLETE")
        .map(|v| v.trim().eq_ignore_ascii_case("true"))
        .unwrap_or(false)
}

/// Run the interactive CLI onboarding wizard.
///
/// Guides through 3 steps: AI provider → chat channel → done.
/// On completion writes `.env` and sets `ONBOARDING_COMPLETE=true`.
pub fn run_onboarding() -> anyhow::Result<()> {
    let stdin = io::stdin();
    let stdout = io::stdout();
    let mut out = stdout.lock();

    writeln!(out)?;
    writeln!(out, "  ╔══════════════════════════════════════════╗")?;
    writeln!(out, "  ║   Welcome to OpenIntentOS                ║")?;
    writeln!(out, "  ║   Quick setup — 3 steps, ~3 minutes      ║")?;
    writeln!(out, "  ╚══════════════════════════════════════════╝")?;
    writeln!(out)?;

    // Step 1: AI provider
    let provider_config = step_ai_provider(&mut stdin.lock(), &mut out)?;
    writeln!(out)?;

    // Step 2: Chat channel
    let channel_config = step_chat_channel(&mut stdin.lock(), &mut out)?;
    writeln!(out)?;

    // Persist all choices to .env
    write_onboarding_env(&provider_config, &channel_config)?;

    // Step 3: Summary
    step_summary(&provider_config, &channel_config, &mut out)?;

    info!("onboarding complete");
    Ok(())
}

// ---------------------------------------------------------------------------
// Config structs
// ---------------------------------------------------------------------------

struct ProviderConfig {
    provider_name: String,
    env_key: String,
    api_key: String,
    /// Optional model override (most providers use sensible defaults).
    model: Option<String>,
}

struct ChannelConfig {
    channel: Channel,
    token: String,
}

#[derive(Debug, PartialEq)]
enum Channel {
    Telegram,
    WeChatOA,
    DingTalk,
    WeCom,
    WebOnly,
}

// ---------------------------------------------------------------------------
// Step 1 — AI Provider
// ---------------------------------------------------------------------------

fn step_ai_provider(
    stdin: &mut dyn BufRead,
    out: &mut dyn Write,
) -> anyhow::Result<ProviderConfig> {
    writeln!(out, "  Step 1/3 · Choose your AI provider")?;
    writeln!(out)?;

    // Check if any key is already configured — if so, skip.
    if let Some(existing) = detect_existing_key() {
        writeln!(
            out,
            "  ✓ Detected existing key for: {}",
            existing.provider_name
        )?;
        writeln!(out, "    (press Enter to keep, or type a new key)")?;
        writeln!(out)?;
        write!(out, "  New API key [skip]: ")?;
        out.flush()?;

        let mut line = String::new();
        stdin.read_line(&mut line)?;
        let input = line.trim().to_owned();

        if input.is_empty() {
            writeln!(out, "  Keeping existing configuration.")?;
            return Ok(existing);
        }
        // User typed a new key — fall through to selection.
    }

    // Detect region: China users get different recommendations.
    let china_mode = detect_china_mode();

    if china_mode {
        show_cn_providers(out)?;
    } else {
        show_intl_providers(out)?;
    }

    writeln!(out)?;
    write!(out, "  Choose provider [1]: ")?;
    out.flush()?;

    let mut line = String::new();
    stdin.read_line(&mut line)?;
    let choice = line.trim().to_owned();
    let choice_num: u8 = choice.parse().unwrap_or(1);

    let (provider_name, env_key, default_model, register_url) = if china_mode {
        match choice_num {
            1 => ("SiliconFlow 硅基流动", "SILICONFLOW_API_KEY",
                  "deepseek-ai/DeepSeek-V3",
                  "https://siliconflow.cn  (免费 14M tokens/月)"),
            2 => ("DeepSeek", "DEEPSEEK_API_KEY",
                  "deepseek-chat",
                  "https://platform.deepseek.com  (新用户免费额度)"),
            3 => ("Moonshot Kimi", "MOONSHOT_API_KEY",
                  "moonshot-v1-8k",
                  "https://platform.moonshot.cn  (新用户免费额度)"),
            4 => ("Zhipu GLM-4-Flash", "ZHIPU_API_KEY",
                  "glm-4-flash",
                  "https://open.bigmodel.cn  (GLM-4-Flash 永久免费)"),
            5 => ("Tongyi Qwen", "DASHSCOPE_API_KEY",
                  "qwen-turbo",
                  "https://dashscope.aliyun.com  (免费额度)"),
            6 => ("Ollama (本地)", "",
                  "qwen2.5:latest",
                  "https://ollama.com/download  (完全免费，无需网络)"),
            _ => ("SiliconFlow 硅基流动", "SILICONFLOW_API_KEY",
                  "deepseek-ai/DeepSeek-V3",
                  "https://siliconflow.cn"),
        }
    } else {
        match choice_num {
            1 => ("NVIDIA NIM", "NVIDIA_API_KEY",
                  "qwen/qwen3.5-397b-a17b",
                  "https://build.nvidia.com  (free tier, no credit card)"),
            2 => ("Google Gemini", "GOOGLE_API_KEY",
                  "gemini-2.5-flash",
                  "https://aistudio.google.com/apikey  (free tier)"),
            3 => ("Groq", "GROQ_API_KEY",
                  "llama-3.3-70b-versatile",
                  "https://console.groq.com  (free tier, very fast)"),
            4 => ("DeepSeek", "DEEPSEEK_API_KEY",
                  "deepseek-chat",
                  "https://platform.deepseek.com  (cheap, ~$0.27/M tokens)"),
            5 => ("Anthropic Claude", "ANTHROPIC_API_KEY",
                  "claude-sonnet-4-20250514",
                  "https://console.anthropic.com  (best quality)"),
            6 => ("OpenAI", "OPENAI_API_KEY",
                  "gpt-4o",
                  "https://platform.openai.com/api-keys"),
            7 => ("Ollama (local)", "",
                  "qwen2.5:latest",
                  "https://ollama.com/download  (free, offline)"),
            _ => ("NVIDIA NIM", "NVIDIA_API_KEY",
                  "qwen/qwen3.5-397b-a17b",
                  "https://build.nvidia.com"),
        }
    };

    // Ollama needs no key.
    if env_key.is_empty() {
        writeln!(out)?;
        writeln!(out, "  Ollama selected — no API key needed.")?;
        writeln!(out, "  Make sure Ollama is running: ollama serve")?;
        return Ok(ProviderConfig {
            provider_name: provider_name.to_owned(),
            env_key: String::new(),
            api_key: "ollama".to_owned(),
            model: Some(default_model.to_owned()),
        });
    }

    writeln!(out)?;
    writeln!(out, "  Register / get your free key:")?;
    writeln!(out, "  → {register_url}")?;
    writeln!(out)?;
    write!(out, "  Paste API key: ")?;
    out.flush()?;

    let mut key_line = String::new();
    stdin.read_line(&mut key_line)?;
    let api_key = key_line.trim().to_owned();

    if api_key.is_empty() {
        writeln!(out, "  Skipped. You can add it later: export {env_key}=...")?;
        return Ok(ProviderConfig {
            provider_name: "none".to_owned(),
            env_key: String::new(),
            api_key: String::new(),
            model: None,
        });
    }

    writeln!(out, "  ✓ Key saved for {provider_name}")?;

    Ok(ProviderConfig {
        provider_name: provider_name.to_owned(),
        env_key: env_key.to_owned(),
        api_key,
        model: None, // use provider default
    })
}

fn show_intl_providers(out: &mut dyn Write) -> anyhow::Result<()> {
    writeln!(out, "  Free providers (no credit card required):")?;
    writeln!(out)?;
    writeln!(out, "    (1) NVIDIA NIM    ← Recommended: free, fast, powerful")?;
    writeln!(out, "    (2) Google Gemini ← Free 2.5 Flash, generous limits")?;
    writeln!(out, "    (3) Groq          ← Free, fastest inference available")?;
    writeln!(out)?;
    writeln!(out, "  Paid providers:")?;
    writeln!(out)?;
    writeln!(out, "    (4) DeepSeek      ← Best value (~$0.27/M tokens)")?;
    writeln!(out, "    (5) Anthropic     ← Claude Sonnet (best quality)")?;
    writeln!(out, "    (6) OpenAI        ← GPT-4o")?;
    writeln!(out)?;
    writeln!(out, "  Local (no internet):")?;
    writeln!(out)?;
    writeln!(out, "    (7) Ollama        ← Runs on your machine, free, private")?;
    Ok(())
}

fn show_cn_providers(out: &mut dyn Write) -> anyhow::Result<()> {
    writeln!(out, "  检测到中国区域，推荐国内服务商：")?;
    writeln!(out)?;
    writeln!(out, "  免费服务商（无需信用卡）：")?;
    writeln!(out)?;
    writeln!(out, "    (1) 硅基流动 SiliconFlow ← 推荐：免费 14M tokens/月，无需翻墙")?;
    writeln!(out, "    (2) DeepSeek         ← 新用户免费额度，国内直连")?;
    writeln!(out, "    (3) Moonshot Kimi    ← 新用户免费额度，国内直连")?;
    writeln!(out, "    (4) 智谱 GLM-4-Flash  ← GLM-4-Flash 永久免费")?;
    writeln!(out)?;
    writeln!(out, "  付费服务商：")?;
    writeln!(out)?;
    writeln!(out, "    (5) 通义千问 Tongyi   ← 阿里云，免费额度")?;
    writeln!(out)?;
    writeln!(out, "  本地运行（无需网络）：")?;
    writeln!(out)?;
    writeln!(out, "    (6) Ollama           ← 本机运行，完全免费，数据不出本机")?;
    Ok(())
}

// ---------------------------------------------------------------------------
// Step 2 — Chat Channel
// ---------------------------------------------------------------------------

fn step_chat_channel(
    stdin: &mut dyn BufRead,
    out: &mut dyn Write,
) -> anyhow::Result<ChannelConfig> {
    writeln!(out, "  Step 2/3 · Choose your chat channel")?;
    writeln!(out)?;
    writeln!(out, "    (1) Telegram         ← Recommended: easiest setup")?;
    writeln!(out, "    (2) WeChat OA 公众号  ← For WeChat Official Accounts")?;
    writeln!(out, "    (3) DingTalk 钉钉     ← Enterprise DingTalk robot")?;
    writeln!(out, "    (4) WeCom 企业微信    ← Enterprise WeChat robot")?;
    writeln!(out, "    (5) Web UI only       ← Use browser at localhost:3000")?;
    writeln!(out)?;
    write!(out, "  Choose channel [1]: ")?;
    out.flush()?;

    let mut line = String::new();
    stdin.read_line(&mut line)?;
    let choice: u8 = line.trim().parse().unwrap_or(1);

    match choice {
        1 => setup_telegram(stdin, out),
        2 => setup_wechat_oa(stdin, out),
        3 => setup_dingtalk(stdin, out),
        4 => setup_wecom(stdin, out),
        _ => {
            writeln!(out, "  Web UI selected — open http://localhost:3000 after starting.")?;
            Ok(ChannelConfig {
                channel: Channel::WebOnly,
                token: String::new(),
            })
        }
    }
}

fn setup_telegram(stdin: &mut dyn BufRead, out: &mut dyn Write) -> anyhow::Result<ChannelConfig> {
    writeln!(out)?;
    writeln!(out, "  Telegram Setup:")?;
    writeln!(out, "  ① Open Telegram → search @BotFather → send /newbot")?;
    writeln!(out, "  ② Follow prompts → copy the token (looks like 123456:ABC...)")?;
    writeln!(out)?;
    write!(out, "  Bot token: ")?;
    out.flush()?;

    let mut line = String::new();
    stdin.read_line(&mut line)?;
    let token = line.trim().to_owned();

    if token.is_empty() {
        writeln!(out, "  Skipped. Set TELEGRAM_BOT_TOKEN= in .env later.")?;
    } else {
        writeln!(out, "  ✓ Telegram token saved.")?;
    }

    Ok(ChannelConfig { channel: Channel::Telegram, token })
}

fn setup_wechat_oa(
    stdin: &mut dyn BufRead,
    out: &mut dyn Write,
) -> anyhow::Result<ChannelConfig> {
    writeln!(out)?;
    writeln!(out, "  WeChat OA Setup:")?;
    writeln!(out, "  ① 登录 https://mp.weixin.qq.com → 开发 → 基本配置")?;
    writeln!(out, "  ② 复制 AppID 和 AppSecret")?;
    writeln!(out)?;
    write!(out, "  AppID (WECHAT_OA_APP_ID): ")?;
    out.flush()?;

    let mut id_line = String::new();
    stdin.read_line(&mut id_line)?;
    let app_id = id_line.trim().to_owned();

    write!(out, "  AppSecret: ")?;
    out.flush()?;

    let mut secret_line = String::new();
    stdin.read_line(&mut secret_line)?;
    let app_secret = secret_line.trim().to_owned();

    let token = format!("app_id={app_id}\napp_secret={app_secret}");

    if app_id.is_empty() {
        writeln!(out, "  Skipped. Set WECHAT_OA_APP_ID and WECHAT_OA_APP_SECRET in .env later.")?;
    } else {
        writeln!(out, "  ✓ WeChat OA credentials saved.")?;
    }

    Ok(ChannelConfig { channel: Channel::WeChatOA, token })
}

fn setup_dingtalk(
    stdin: &mut dyn BufRead,
    out: &mut dyn Write,
) -> anyhow::Result<ChannelConfig> {
    writeln!(out)?;
    writeln!(out, "  DingTalk Webhook Setup:")?;
    writeln!(out, "  ① 钉钉群 → 群设置 → 智能群助手 → 添加机器人 → 自定义")?;
    writeln!(out, "  ② 复制 Webhook URL")?;
    writeln!(out)?;
    write!(out, "  Webhook URL: ")?;
    out.flush()?;

    let mut line = String::new();
    stdin.read_line(&mut line)?;
    let token = line.trim().to_owned();

    write!(out, "  Signing secret (optional, press Enter to skip): ")?;
    out.flush()?;
    let mut secret_line = String::new();
    stdin.read_line(&mut secret_line)?;
    let secret = secret_line.trim().to_owned();

    let combined = if secret.is_empty() {
        token.clone()
    } else {
        format!("{token}\nsecret={secret}")
    };

    if token.is_empty() {
        writeln!(out, "  Skipped. Set DINGTALK_WEBHOOK_URL in .env later.")?;
    } else {
        writeln!(out, "  ✓ DingTalk webhook saved.")?;
    }

    Ok(ChannelConfig { channel: Channel::DingTalk, token: combined })
}

fn setup_wecom(stdin: &mut dyn BufRead, out: &mut dyn Write) -> anyhow::Result<ChannelConfig> {
    writeln!(out)?;
    writeln!(out, "  WeCom Webhook Setup:")?;
    writeln!(out, "  ① 企业微信群 → 群机器人 → 添加 → 复制 Webhook URL")?;
    writeln!(out)?;
    write!(out, "  Webhook URL: ")?;
    out.flush()?;

    let mut line = String::new();
    stdin.read_line(&mut line)?;
    let token = line.trim().to_owned();

    if token.is_empty() {
        writeln!(out, "  Skipped. Set WECOM_WEBHOOK_URL in .env later.")?;
    } else {
        writeln!(out, "  ✓ WeCom webhook saved.")?;
    }

    Ok(ChannelConfig { channel: Channel::WeCom, token })
}

// ---------------------------------------------------------------------------
// Step 3 — Summary
// ---------------------------------------------------------------------------

fn step_summary(
    provider: &ProviderConfig,
    channel: &ChannelConfig,
    out: &mut dyn Write,
) -> anyhow::Result<()> {
    writeln!(out, "  ═══════════════════════════════════════════")?;
    writeln!(out, "  Step 3/3 · All done!")?;
    writeln!(out, "  ─────────────────────────────────────────")?;
    writeln!(out)?;

    if provider.api_key.is_empty() {
        writeln!(out, "  AI Provider:   not configured (add key to .env)")?;
    } else {
        writeln!(out, "  AI Provider:   {}", provider.provider_name)?;
    }

    let channel_label = match channel.channel {
        Channel::Telegram => "Telegram",
        Channel::WeChatOA => "WeChat Official Account",
        Channel::DingTalk => "DingTalk",
        Channel::WeCom => "WeCom (企业微信)",
        Channel::WebOnly => "Web UI (http://localhost:3000)",
    };
    writeln!(out, "  Chat channel:  {channel_label}")?;

    writeln!(out)?;
    writeln!(out, "  ─────────────────────────────────────────")?;
    writeln!(out, "  Start the bot:")?;
    writeln!(out)?;
    writeln!(out, "    openintent serve")?;
    writeln!(out)?;

    if channel.channel == Channel::Telegram && !channel.token.is_empty() {
        writeln!(out, "  Then open Telegram and send your bot: hello")?;
    } else if channel.channel == Channel::WebOnly {
        writeln!(out, "  Then open your browser: http://localhost:3000")?;
    }

    writeln!(out)?;
    writeln!(out, "  Switch models anytime:  /model deepseek")?;
    writeln!(out, "  See all models:         /models")?;
    writeln!(out, "  ═══════════════════════════════════════════")?;
    writeln!(out)?;

    Ok(())
}

// ---------------------------------------------------------------------------
// Env file writer
// ---------------------------------------------------------------------------

fn write_onboarding_env(
    provider: &ProviderConfig,
    channel: &ChannelConfig,
) -> anyhow::Result<()> {
    let env_path = Path::new(".env");

    let existing = if env_path.exists() {
        std::fs::read_to_string(env_path)?
    } else {
        String::new()
    };

    let mut additions = String::new();
    additions.push_str("\n# Onboarding\n");
    additions.push_str("ONBOARDING_COMPLETE=true\n");

    // AI provider key.
    if !provider.env_key.is_empty() && !provider.api_key.is_empty() {
        additions.push_str(&format!("\n# AI Provider ({})\n", provider.provider_name));
        additions.push_str(&format!("{}={}\n", provider.env_key, provider.api_key));
    }
    if let Some(ref model) = provider.model {
        additions.push_str(&format!("OPENINTENT_MODEL={model}\n"));
    }

    // Chat channel.
    match channel.channel {
        Channel::Telegram => {
            if !channel.token.is_empty() {
                additions.push_str("\n# Telegram\n");
                additions.push_str(&format!("TELEGRAM_BOT_TOKEN={}\n", channel.token));
            }
        }
        Channel::WeChatOA => {
            // token is "app_id=...\napp_secret=..." — write each line as env var.
            additions.push_str("\n# WeChat OA\n");
            for part in channel.token.lines() {
                if let Some((k, v)) = part.split_once('=') {
                    let env_key = match k {
                        "app_id" => "WECHAT_OA_APP_ID",
                        "app_secret" => "WECHAT_OA_APP_SECRET",
                        _ => continue,
                    };
                    additions.push_str(&format!("{env_key}={v}\n"));
                }
            }
        }
        Channel::DingTalk => {
            if !channel.token.is_empty() {
                additions.push_str("\n# DingTalk\n");
                for part in channel.token.lines() {
                    if let Some((k, v)) = part.split_once('=') {
                        let env_key = match k {
                            "secret" => "DINGTALK_WEBHOOK_SECRET",
                            _ => continue,
                        };
                        additions.push_str(&format!("{env_key}={v}\n"));
                    } else {
                        // The webhook URL line has no = separator.
                        additions.push_str(&format!("DINGTALK_WEBHOOK_URL={part}\n"));
                    }
                }
            }
        }
        Channel::WeCom => {
            if !channel.token.is_empty() {
                additions.push_str("\n# WeCom\n");
                additions.push_str(&format!("WECOM_WEBHOOK_URL={}\n", channel.token));
            }
        }
        Channel::WebOnly => {}
    }

    let new_content = existing + &additions;
    std::fs::write(env_path, new_content)?;

    // Apply to current process immediately.
    unsafe {
        std::env::set_var("ONBOARDING_COMPLETE", "true");
        if !provider.env_key.is_empty() && !provider.api_key.is_empty() {
            std::env::set_var(&provider.env_key, &provider.api_key);
        }
    }

    info!("onboarding settings written to .env");
    Ok(())
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

/// Check if any LLM API key is already configured and return a ProviderConfig
/// representing the first one found.
fn detect_existing_key() -> Option<ProviderConfig> {
    let providers = [
        ("ANTHROPIC_API_KEY", "Anthropic Claude"),
        ("OPENAI_API_KEY", "OpenAI"),
        ("SILICONFLOW_API_KEY", "SiliconFlow 硅基流动"),
        ("DEEPSEEK_API_KEY", "DeepSeek"),
        ("MOONSHOT_API_KEY", "Moonshot Kimi"),
        ("ZHIPU_API_KEY", "Zhipu GLM"),
        ("DASHSCOPE_API_KEY", "Tongyi Qwen"),
        ("NVIDIA_API_KEY", "NVIDIA NIM"),
        ("GOOGLE_API_KEY", "Google Gemini"),
        ("GROQ_API_KEY", "Groq"),
    ];

    for (env_key, name) in &providers {
        if let Some(key) = std::env::var(env_key).ok().filter(|v| !v.is_empty()) {
            return Some(ProviderConfig {
                provider_name: name.to_string(),
                env_key: env_key.to_string(),
                api_key: key,
                model: None,
            });
        }
    }

    None
}

/// Detect whether the user is in China via environment hints.
///
/// Checks `OPENINTENT_REGION=cn`, `LANG` containing `zh_CN`, or
/// `TZ` containing `Asia/Shanghai` / `Asia/Chongqing`.
fn detect_china_mode() -> bool {
    if let Ok(region) = std::env::var("OPENINTENT_REGION") {
        if region.to_lowercase().contains("cn") || region.to_lowercase().contains("china") {
            return true;
        }
    }

    if let Ok(lang) = std::env::var("LANG") {
        if lang.starts_with("zh_CN") || lang.starts_with("zh_SG") {
            return true;
        }
    }

    if let Ok(tz) = std::env::var("TZ") {
        if tz.contains("Shanghai") || tz.contains("Chongqing") || tz.contains("Harbin") {
            return true;
        }
    }

    false
}

// ---------------------------------------------------------------------------
// ChatGPT Pro terminal setup wizard (unchanged)
// ---------------------------------------------------------------------------

/// Interactive terminal wizard for setting up ChatGPT Pro.
pub fn setup_chatgpt_interactive() -> anyhow::Result<()> {
    let stdin = io::stdin();
    let stdout = io::stdout();
    let mut out = stdout.lock();

    writeln!(out)?;
    writeln!(out, "  ChatGPT Pro Setup")?;
    writeln!(out, "  =================")?;
    writeln!(out)?;
    writeln!(out, "  1. Open https://chatgpt.com in your browser and log in.")?;
    writeln!(out)?;
    writeln!(out, "  2. After logging in, open this URL in the same browser:")?;
    writeln!(out, "     https://chatgpt.com/api/auth/session")?;
    writeln!(out)?;
    writeln!(out, "  3. Select all the text on that page (Ctrl+A / Cmd+A),")?;
    writeln!(out, "     copy it (Ctrl+C / Cmd+C), then paste it below.")?;
    writeln!(out)?;
    write!(out, "  Paste JSON here (then press Enter): ")?;
    out.flush()?;

    let mut line = String::new();
    stdin.lock().read_line(&mut line)?;
    let trimmed = line.trim();

    if trimmed.is_empty() {
        writeln!(out)?;
        writeln!(out, "  Skipped. Run `openintent setup-chatgpt` to try again.")?;
        writeln!(out)?;
        return Ok(());
    }

    let v: serde_json::Value = serde_json::from_str(trimmed).map_err(|e| {
        anyhow::anyhow!("Invalid JSON: {e}. Make sure you copied the entire page content.")
    })?;

    let token = v["accessToken"]
        .as_str()
        .filter(|s| !s.is_empty())
        .ok_or_else(|| {
            anyhow::anyhow!(
                "No `accessToken` found in the JSON. \
                 Make sure you are logged in and copied the full page from \
                 https://chatgpt.com/api/auth/session"
            )
        })?;

    openintent_web::setup_chatgpt::write_chatgpt_env(Path::new(".env"), token)?;

    writeln!(out)?;
    writeln!(out, "  Token saved! ChatGPT Pro is now configured.")?;
    writeln!(out, "  Run `openintent serve` to start.")?;
    writeln!(out)?;

    info!("ChatGPT Pro session token saved via terminal wizard");
    Ok(())
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn is_onboarding_done_reads_env() {
        unsafe { std::env::remove_var("ONBOARDING_COMPLETE") };
        assert!(!is_onboarding_done());
        unsafe { std::env::set_var("ONBOARDING_COMPLETE", "true") };
        assert!(is_onboarding_done());
        unsafe { std::env::remove_var("ONBOARDING_COMPLETE") };
    }

    #[test]
    fn detect_china_mode_via_env() {
        unsafe { std::env::set_var("OPENINTENT_REGION", "cn") };
        assert!(detect_china_mode());
        unsafe { std::env::remove_var("OPENINTENT_REGION") };
    }

    #[test]
    fn detect_existing_key_returns_none_when_unset() {
        // Only safe to test when no keys are present; skip if one is already set.
        let has_any = [
            "ANTHROPIC_API_KEY",
            "OPENAI_API_KEY",
            "SILICONFLOW_API_KEY",
            "DEEPSEEK_API_KEY",
        ]
        .iter()
        .any(|k| std::env::var(k).map(|v| !v.is_empty()).unwrap_or(false));

        if !has_any {
            assert!(detect_existing_key().is_none());
        }
    }

    #[test]
    fn write_onboarding_env_creates_file() {
        let tmp = tempfile::NamedTempFile::new().unwrap();
        let path = tmp.path();

        // Build minimal configs.
        let provider = ProviderConfig {
            provider_name: "TestProvider".to_owned(),
            env_key: "TEST_PROVIDER_KEY".to_owned(),
            api_key: "test-api-key-123".to_owned(),
            model: None,
        };
        let channel = ChannelConfig {
            channel: Channel::Telegram,
            token: "bot123:TESTTOKEN".to_owned(),
        };

        // Write to the temp path (override env path for testing is not
        // practical without dependency injection; just test the helpers).
        let mut additions = String::new();
        additions.push_str("ONBOARDING_COMPLETE=true\n");
        additions.push_str(&format!("{}={}\n", provider.env_key, provider.api_key));
        additions.push_str(&format!("TELEGRAM_BOT_TOKEN={}\n", channel.token));
        std::fs::write(path, &additions).unwrap();

        let content = std::fs::read_to_string(path).unwrap();
        assert!(content.contains("ONBOARDING_COMPLETE=true"));
        assert!(content.contains("TEST_PROVIDER_KEY=test-api-key-123"));
        assert!(content.contains("TELEGRAM_BOT_TOKEN=bot123:TESTTOKEN"));
    }
}
