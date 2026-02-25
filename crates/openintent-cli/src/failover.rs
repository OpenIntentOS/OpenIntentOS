//! Automatic rate-limit failover for the Telegram bot.
//!
//! When an LLM provider returns a 429 (rate limit) or similar throttling
//! error, this module selects the next available free provider, switches the
//! `LlmClient` to it, and tracks a cooldown so the rate-limited provider is
//! temporarily bypassed.

use std::collections::HashMap;
use std::sync::Arc;
use std::time::{Duration, Instant};

use openintent_agent::{LlmClient, LlmProvider};
use tracing::{info, warn};

use crate::helpers::env_non_empty;

// ---------------------------------------------------------------------------
// Constants
// ---------------------------------------------------------------------------

/// How long a provider stays on cooldown after a rate-limit hit.
const RATE_LIMIT_COOLDOWN: Duration = Duration::from_secs(120);

const DEEPSEEK_BASE_URL: &str = "https://api.deepseek.com/v1";
const NVIDIA_BASE_URL: &str = "https://integrate.api.nvidia.com/v1";
const GOOGLE_BASE_URL: &str = "https://generativelanguage.googleapis.com/v1beta/openai";
const GROQ_BASE_URL: &str = "https://api.groq.com/openai/v1";
const OLLAMA_BASE_URL: &str = "http://localhost:11434/v1";

// ---------------------------------------------------------------------------
// Types
// ---------------------------------------------------------------------------

/// A candidate fallback provider.
struct FallbackCandidate {
    name: &'static str,
    model: &'static str,
    base_url: &'static str,
    key_env: &'static str,
    provider: LlmProvider,
}

/// Tracks rate-limited providers and orchestrates failover.
pub struct FailoverManager {
    /// Map of provider name → when the cooldown expires.
    cooldowns: HashMap<String, Instant>,
}

/// Result of a successful failover.
pub struct FailoverResult {
    pub provider_name: String,
    pub model: String,
}

// ---------------------------------------------------------------------------
// Fallback chain
// ---------------------------------------------------------------------------

/// Ordered list of fallback candidates. Free-tier providers first, then
/// local Ollama as ultimate fallback.
const FALLBACK_CHAIN: &[FallbackCandidate] = &[
    // NVIDIA NIM free-tier models (verified available for our account)
    FallbackCandidate {
        name: "NVIDIA Qwen 3.5 397B",
        model: "qwen/qwen3.5-397b-a17b",
        base_url: NVIDIA_BASE_URL,
        key_env: "NVIDIA_API_KEY",
        provider: LlmProvider::OpenAI,
    },
    FallbackCandidate {
        name: "NVIDIA Kimi K2.5",
        model: "moonshotai/kimi-k2.5",
        base_url: NVIDIA_BASE_URL,
        key_env: "NVIDIA_API_KEY",
        provider: LlmProvider::OpenAI,
    },
    FallbackCandidate {
        name: "NVIDIA Nemotron 3 Nano",
        model: "nvidia/nemotron-3-nano-30b-a3b",
        base_url: NVIDIA_BASE_URL,
        key_env: "NVIDIA_API_KEY",
        provider: LlmProvider::OpenAI,
    },
    // Google Gemini (free tier)
    FallbackCandidate {
        name: "Google Gemini Flash",
        model: "gemini-2.5-flash",
        base_url: GOOGLE_BASE_URL,
        key_env: "GOOGLE_API_KEY",
        provider: LlmProvider::OpenAI,
    },
    // DeepSeek direct
    FallbackCandidate {
        name: "DeepSeek",
        model: "deepseek-chat",
        base_url: DEEPSEEK_BASE_URL,
        key_env: "DEEPSEEK_API_KEY",
        provider: LlmProvider::OpenAI,
    },
    // Groq free-tier
    FallbackCandidate {
        name: "Groq",
        model: "llama-3.3-70b-versatile",
        base_url: GROQ_BASE_URL,
        key_env: "GROQ_API_KEY",
        provider: LlmProvider::OpenAI,
    },
    // Ollama local (always available, no key needed)
    FallbackCandidate {
        name: "Ollama",
        model: "qwen2.5:latest",
        base_url: OLLAMA_BASE_URL,
        key_env: "",
        provider: LlmProvider::OpenAI,
    },
];

// ---------------------------------------------------------------------------
// Implementation
// ---------------------------------------------------------------------------

impl FailoverManager {
    pub fn new() -> Self {
        Self {
            cooldowns: HashMap::new(),
        }
    }

    /// Record that a provider just hit a rate limit.
    pub fn mark_rate_limited(&mut self, provider_name: &str) {
        let expires = Instant::now() + RATE_LIMIT_COOLDOWN;
        warn!(
            provider = provider_name,
            cooldown_secs = RATE_LIMIT_COOLDOWN.as_secs(),
            "provider rate-limited, adding cooldown"
        );
        self.cooldowns.insert(provider_name.to_string(), expires);
    }

    /// Check if a provider is currently on cooldown.
    fn is_on_cooldown(&self, provider_name: &str) -> bool {
        self.cooldowns
            .get(provider_name)
            .is_some_and(|expires| Instant::now() < *expires)
    }

    /// Clean up expired cooldowns.
    pub fn cleanup_expired(&mut self) {
        let now = Instant::now();
        self.cooldowns.retain(|_, expires| now < *expires);
    }

    /// Try to failover to the next available provider that is not on cooldown
    /// and not the current provider.
    ///
    /// Returns `Some(FailoverResult)` if a switch was made, `None` if no
    /// fallback is available.
    pub fn try_failover(
        &mut self,
        current_model: &str,
        llm: &Arc<LlmClient>,
    ) -> Option<FailoverResult> {
        self.cleanup_expired();

        for candidate in FALLBACK_CHAIN {
            // Skip if this is the model that just failed.
            if candidate.model == current_model {
                continue;
            }

            // Skip if this provider is on cooldown.
            if self.is_on_cooldown(candidate.name) {
                continue;
            }

            // Check if the API key is available.
            let api_key = if candidate.key_env.is_empty() {
                // Ollama — no key needed.
                "ollama".to_string()
            } else {
                match env_non_empty(candidate.key_env) {
                    Some(k) => k,
                    None => continue,
                }
            };

            // Perform the switch.
            llm.update_api_key(api_key);
            llm.switch_provider(
                candidate.provider.clone(),
                candidate.base_url.to_string(),
                candidate.model.to_string(),
            );

            info!(
                from_model = current_model,
                to_provider = candidate.name,
                to_model = candidate.model,
                "rate-limit failover: switched provider"
            );

            return Some(FailoverResult {
                provider_name: candidate.name.to_string(),
                model: candidate.model.to_string(),
            });
        }

        warn!("rate-limit failover: no fallback providers available");
        None
    }
}

/// Check whether an error string indicates a rate-limit / throttling response.
pub fn is_rate_limit_error(error: &str) -> bool {
    error.contains("429")
        || error.contains("rate_limit")
        || error.contains("rate limit")
        || error.contains("Too Many Requests")
        || error.contains("too many requests")
        || error.contains("quota")
        || error.contains("throttl")
        || error.contains("overloaded")
        || error.contains("capacity")
}

/// Check whether an error is a provider-level failure that should trigger
/// failover (rate limit, model not found, auth failure, server errors, etc.).
pub fn is_provider_error(error: &str) -> bool {
    is_rate_limit_error(error)
        || error.contains("401 Unauthorized")
        || error.contains("authentication_error")
        || error.contains("token has expired")
        || error.contains("403 Forbidden")
        || error.contains("404 Not Found")
        || error.contains("Not found for account")
        || error.contains("model_not_found")
        || error.contains("does not exist")
        || error.contains("503 Service Unavailable")
        || error.contains("502 Bad Gateway")
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rate_limit_detection() {
        assert!(is_rate_limit_error("API returned 429 Too Many Requests: {}"));
        assert!(is_rate_limit_error("rate_limit_exceeded"));
        assert!(is_rate_limit_error("you have exceeded your rate limit"));
        assert!(is_rate_limit_error("quota exceeded"));
        assert!(is_rate_limit_error("server overloaded"));
        assert!(is_rate_limit_error("insufficient capacity"));
        assert!(!is_rate_limit_error("API returned 401 Unauthorized"));
        assert!(!is_rate_limit_error("invalid JSON"));
    }

    #[test]
    fn cooldown_tracking() {
        let mut mgr = FailoverManager::new();
        assert!(!mgr.is_on_cooldown("DeepSeek"));

        mgr.mark_rate_limited("DeepSeek");
        assert!(mgr.is_on_cooldown("DeepSeek"));
        assert!(!mgr.is_on_cooldown("NVIDIA Nemotron 70B"));
    }

    #[test]
    fn cleanup_expired_cooldowns() {
        let mut mgr = FailoverManager::new();
        // Insert an already-expired cooldown.
        mgr.cooldowns.insert(
            "expired-provider".to_string(),
            Instant::now() - Duration::from_secs(1),
        );
        mgr.cooldowns.insert(
            "active-provider".to_string(),
            Instant::now() + Duration::from_secs(60),
        );

        mgr.cleanup_expired();
        assert!(!mgr.cooldowns.contains_key("expired-provider"));
        assert!(mgr.cooldowns.contains_key("active-provider"));
    }
}
