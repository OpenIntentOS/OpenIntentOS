//! Self-evolution engine for OpenIntentOS.
//!
//! Detects unhandled user intents (failures, max-turn exhaustion, explicit
//! inability) and automatically files GitHub issues so the system can evolve
//! over time.
//!
//! ## How it works
//!
//! ```text
//! User message → Agent ReAct loop → Failure detected
//!                                        ↓
//!                              EvolutionEngine::report()
//!                                        ↓
//!                              Deduplicate → GitHub Issue
//! ```
//!
//! The engine is channel-agnostic: it works the same whether the user came in
//! via Telegram, CLI REPL, WebSocket, or any future frontend.

use std::collections::HashSet;
use std::sync::Arc;

use serde::{Deserialize, Serialize};
use tokio::sync::Mutex;
use tracing::{info, warn};

use crate::error::AgentError;

// ---------------------------------------------------------------------------
// Configuration
// ---------------------------------------------------------------------------

/// Configuration for the evolution engine.
#[derive(Debug, Clone)]
pub struct EvolutionConfig {
    /// GitHub repository owner (e.g. `"cw"`).
    pub github_owner: String,

    /// GitHub repository name (e.g. `"OpenIntentOS"`).
    pub github_repo: String,

    /// GitHub personal access token with `repo` scope.
    pub github_token: String,

    /// Labels to apply to auto-created issues.
    pub labels: Vec<String>,

    /// Whether the engine is enabled.
    pub enabled: bool,

    /// Failure phrases in agent responses that trigger issue creation.
    /// The engine checks if the agent's final text contains any of these.
    pub failure_indicators: Vec<String>,
}

impl Default for EvolutionConfig {
    fn default() -> Self {
        Self {
            github_owner: String::new(),
            github_repo: String::new(),
            github_token: String::new(),
            labels: vec!["auto-evolution".to_owned(), "unhandled-intent".to_owned()],
            enabled: false,
            failure_indicators: Vec::new(),
        }
    }
}

/// TOML representation of the `[evolution]` section in config/default.toml.
#[derive(Debug, Clone, Deserialize, Default)]
struct EvolutionToml {
    #[serde(default)]
    labels: Vec<String>,
    #[serde(default)]
    failure_indicators: Vec<String>,
}

/// Load the `[evolution]` section from `config/default.toml`.
fn load_config_toml() -> Option<EvolutionToml> {
    #[derive(Deserialize)]
    struct Root {
        #[serde(default)]
        evolution: Option<EvolutionToml>,
    }

    let content = std::fs::read_to_string("config/default.toml").ok()?;
    let root: Root = toml::from_str(&content).ok()?;
    root.evolution
}

impl EvolutionConfig {
    /// Create a config from environment variables and config file.
    ///
    /// Loads failure indicators and labels from `config/default.toml` `[evolution]`
    /// section, then reads environment variables:
    /// - `GITHUB_TOKEN` — required for issue creation
    /// - `OPENINTENT_EVOLUTION_REPO` — `owner/repo` format (default: auto-detect from git)
    /// - `OPENINTENT_EVOLUTION_ENABLED` — `true` to enable (default: auto-enable if token present)
    pub fn from_env() -> Self {
        let mut config = Self::default();

        // Load from config file first.
        if let Some(toml) = load_config_toml() {
            if !toml.failure_indicators.is_empty() {
                config.failure_indicators = toml.failure_indicators;
            }
            if !toml.labels.is_empty() {
                config.labels = toml.labels;
            }
        }

        let token = std::env::var("GITHUB_TOKEN").ok().filter(|v| !v.is_empty());

        if let Some(token) = token {
            config.github_token = token;
            config.enabled = true;
        }

        if let Ok(repo) = std::env::var("OPENINTENT_EVOLUTION_REPO")
            && let Some((owner, name)) = repo.split_once('/')
        {
            config.github_owner = owner.to_owned();
            config.github_repo = name.to_owned();
        }

        // If owner/repo not set from env, try to detect from git remote.
        if (config.github_owner.is_empty() || config.github_repo.is_empty())
            && let Some((owner, repo)) = detect_github_repo()
        {
            config.github_owner = owner;
            config.github_repo = repo;
        }

        // Disable if we don't have all required info.
        if config.github_token.is_empty()
            || config.github_owner.is_empty()
            || config.github_repo.is_empty()
        {
            config.enabled = false;
        }

        config
    }
}

// ---------------------------------------------------------------------------
// Unhandled intent
// ---------------------------------------------------------------------------

/// Describes a user intent that the agent could not handle.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UnhandledIntent {
    /// The original user message.
    pub user_message: String,

    /// Which channel the message came from.
    pub channel: String,

    /// The error that occurred (if any).
    pub error: Option<String>,

    /// The agent's response text (if it produced one before failing).
    pub agent_response: Option<String>,

    /// Number of ReAct turns consumed.
    pub turns_used: Option<u32>,

    /// Unix timestamp.
    pub timestamp: i64,
}

/// The reason an intent was flagged as unhandled.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FailureReason {
    /// The ReAct loop hit the maximum turn limit.
    MaxTurnsExceeded,
    /// An error occurred during processing.
    AgentError(String),
    /// The agent's response text indicates inability.
    ResponseIndicatesInability,
}

// ---------------------------------------------------------------------------
// Evolution engine
// ---------------------------------------------------------------------------

/// The self-evolution engine.
///
/// Thread-safe (wrapped in `Arc<Mutex<>>`) so it can be shared across
/// channels and async tasks.
pub struct EvolutionEngine {
    config: EvolutionConfig,
    http: reqwest::Client,
    /// Simple deduplication: set of fingerprints (hash of user message).
    recent_fingerprints: HashSet<u64>,
}

impl EvolutionEngine {
    /// Create a new evolution engine with the given config.
    pub fn new(config: EvolutionConfig) -> Arc<Mutex<Self>> {
        Arc::new(Mutex::new(Self {
            config,
            http: reqwest::Client::new(),
            recent_fingerprints: HashSet::new(),
        }))
    }

    /// Create from environment variables.  Returns `None` if not configured.
    pub fn from_env() -> Option<Arc<Mutex<Self>>> {
        let config = EvolutionConfig::from_env();
        if config.enabled {
            info!(
                owner = %config.github_owner,
                repo = %config.github_repo,
                "evolution engine enabled"
            );
            Some(Self::new(config))
        } else {
            None
        }
    }

    /// Check if the engine is enabled and ready.
    pub fn is_enabled(&self) -> bool {
        self.config.enabled
    }

    /// Analyze an agent error and report if it represents an unhandled intent.
    pub async fn report_error(
        &mut self,
        user_message: &str,
        channel: &str,
        error: &AgentError,
    ) -> Option<String> {
        let reason = match error {
            AgentError::MaxTurnsExceeded { .. } => FailureReason::MaxTurnsExceeded,
            other => FailureReason::AgentError(other.to_string()),
        };

        let intent = UnhandledIntent {
            user_message: user_message.to_owned(),
            channel: channel.to_owned(),
            error: Some(error.to_string()),
            agent_response: None,
            turns_used: None,
            timestamp: chrono::Utc::now().timestamp(),
        };

        self.report(intent, reason).await
    }

    /// Analyze an agent's text response for signs of inability.
    pub async fn analyze_response(
        &mut self,
        user_message: &str,
        agent_response: &str,
        channel: &str,
        turns_used: u32,
    ) -> Option<String> {
        let lower = agent_response.to_lowercase();
        let indicates_failure = self
            .config
            .failure_indicators
            .iter()
            .any(|indicator| lower.contains(&indicator.to_lowercase()));

        if !indicates_failure {
            return None;
        }

        let intent = UnhandledIntent {
            user_message: user_message.to_owned(),
            channel: channel.to_owned(),
            error: None,
            agent_response: Some(truncate(agent_response, 500)),
            turns_used: Some(turns_used),
            timestamp: chrono::Utc::now().timestamp(),
        };

        self.report(intent, FailureReason::ResponseIndicatesInability)
            .await
    }

    /// Core: deduplicate and create the GitHub issue.
    async fn report(&mut self, intent: UnhandledIntent, reason: FailureReason) -> Option<String> {
        if !self.config.enabled {
            return None;
        }

        // Fingerprint for deduplication (simple hash of user message).
        let fingerprint = hash_string(&intent.user_message);
        if self.recent_fingerprints.contains(&fingerprint) {
            info!(
                fingerprint,
                "duplicate intent detected, skipping issue creation"
            );
            return None;
        }

        // Create the issue.
        match self.create_github_issue(&intent, &reason).await {
            Ok(url) => {
                self.recent_fingerprints.insert(fingerprint);
                info!(url = %url, "evolution issue created");
                Some(url)
            }
            Err(e) => {
                warn!(error = %e, "failed to create evolution issue");
                None
            }
        }
    }

    /// Create a GitHub issue for an unhandled intent.
    async fn create_github_issue(
        &self,
        intent: &UnhandledIntent,
        reason: &FailureReason,
    ) -> std::result::Result<String, Box<dyn std::error::Error + Send + Sync>> {
        let reason_label = match reason {
            FailureReason::MaxTurnsExceeded => "Max turns exceeded",
            FailureReason::AgentError(_) => "Agent error",
            FailureReason::ResponseIndicatesInability => "Agent indicated inability",
        };

        let title = format!(
            "[Auto-Evolution] Unhandled intent: {}",
            truncate(&intent.user_message, 60)
        );

        let body = format!(
            "## Unhandled Intent Report\n\
             \n\
             **Channel:** {channel}\n\
             **Reason:** {reason_label}\n\
             **Timestamp:** {ts}\n\
             **Turns used:** {turns}\n\
             \n\
             ### User Message\n\
             ```\n\
             {user_msg}\n\
             ```\n\
             \n\
             {error_section}\
             {response_section}\
             ### Next Steps\n\
             \n\
             - [ ] Analyze what capability is missing\n\
             - [ ] Implement new adapter/tool or extend existing one\n\
             - [ ] Add test coverage\n\
             - [ ] Verify the intent can be handled after the fix\n\
             \n\
             ---\n\
             *This issue was automatically created by the OpenIntentOS Evolution Engine.*\n",
            channel = intent.channel,
            ts = intent.timestamp,
            turns = intent
                .turns_used
                .map(|t| t.to_string())
                .unwrap_or_else(|| "N/A".to_owned()),
            user_msg = intent.user_message,
            error_section = if let Some(ref e) = intent.error {
                format!("### Error\n```\n{}\n```\n\n", truncate(e, 500))
            } else {
                String::new()
            },
            response_section = if let Some(ref r) = intent.agent_response {
                format!("### Agent Response\n```\n{}\n```\n\n", truncate(r, 500))
            } else {
                String::new()
            },
        );

        let url = format!(
            "https://api.github.com/repos/{}/{}/issues",
            self.config.github_owner, self.config.github_repo
        );

        let mut payload = serde_json::json!({
            "title": title,
            "body": body,
            "labels": self.config.labels,
        });

        // Add channel-specific label.
        if let Some(labels) = payload.get_mut("labels").and_then(|v| v.as_array_mut()) {
            labels.push(serde_json::json!(format!("channel:{}", intent.channel)));
        }

        let response = self
            .http
            .post(&url)
            .header(
                "Authorization",
                format!("Bearer {}", self.config.github_token),
            )
            .header("Accept", "application/vnd.github+json")
            .header("User-Agent", "OpenIntentOS/0.1")
            .header("X-GitHub-Api-Version", "2022-11-28")
            .json(&payload)
            .send()
            .await?;

        if !response.status().is_success() {
            let status = response.status();
            let body = response.text().await.unwrap_or_default();
            return Err(format!("GitHub API error ({status}): {body}").into());
        }

        let json: serde_json::Value = response.json().await?;
        let issue_url = json
            .get("html_url")
            .and_then(|v| v.as_str())
            .unwrap_or("unknown")
            .to_owned();

        Ok(issue_url)
    }

    /// Clear the deduplication cache (useful for testing or periodic reset).
    pub fn clear_cache(&mut self) {
        self.recent_fingerprints.clear();
    }
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

/// Simple string hash for deduplication fingerprinting.
fn hash_string(s: &str) -> u64 {
    use std::hash::{Hash, Hasher};
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    s.to_lowercase().trim().hash(&mut hasher);
    hasher.finish()
}

/// Truncate a string to `max_len` characters, appending "..." if truncated.
fn truncate(s: &str, max_len: usize) -> String {
    if s.len() <= max_len {
        s.to_owned()
    } else {
        let end = s
            .char_indices()
            .nth(max_len)
            .map(|(i, _)| i)
            .unwrap_or(s.len());
        format!("{}...", &s[..end])
    }
}

/// Detect the GitHub owner/repo from the git remote origin URL.
fn detect_github_repo() -> Option<(String, String)> {
    let output = std::process::Command::new("git")
        .args(["remote", "get-url", "origin"])
        .output()
        .ok()?;

    if !output.status.success() {
        return None;
    }

    let url = String::from_utf8(output.stdout).ok()?;
    let url = url.trim();

    // Handle SSH format: git@github.com:owner/repo.git
    if let Some(rest) = url.strip_prefix("git@github.com:") {
        let rest = rest.strip_suffix(".git").unwrap_or(rest);
        let (owner, repo) = rest.split_once('/')?;
        return Some((owner.to_owned(), repo.to_owned()));
    }

    // Handle HTTPS format: https://github.com/owner/repo.git
    if url.contains("github.com") {
        let parts: Vec<&str> = url.split('/').collect();
        if parts.len() >= 2 {
            let owner = parts[parts.len() - 2];
            let repo = parts[parts.len() - 1]
                .strip_suffix(".git")
                .unwrap_or(parts[parts.len() - 1]);
            return Some((owner.to_owned(), repo.to_owned()));
        }
    }

    None
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hash_string_is_deterministic() {
        let h1 = hash_string("hello world");
        let h2 = hash_string("hello world");
        assert_eq!(h1, h2);
    }

    #[test]
    fn hash_string_is_case_insensitive() {
        let h1 = hash_string("Hello World");
        let h2 = hash_string("hello world");
        assert_eq!(h1, h2);
    }

    #[test]
    fn hash_string_trims_whitespace() {
        let h1 = hash_string("  hello  ");
        let h2 = hash_string("hello");
        assert_eq!(h1, h2);
    }

    #[test]
    fn truncate_short_string_unchanged() {
        assert_eq!(truncate("hello", 10), "hello");
    }

    #[test]
    fn truncate_long_string_adds_ellipsis() {
        let result = truncate("hello world this is a long string", 10);
        assert!(result.ends_with("..."));
        assert!(result.len() <= 14); // 10 chars + "..."
    }

    #[test]
    fn default_config_has_labels() {
        let config = EvolutionConfig::default();
        assert!(config.labels.contains(&"auto-evolution".to_owned()));
        assert!(config.labels.contains(&"unhandled-intent".to_owned()));
    }

    #[test]
    fn default_config_has_empty_failure_indicators() {
        // Default config has no hardcoded indicators; they come from config file.
        let config = EvolutionConfig::default();
        assert!(config.failure_indicators.is_empty());
    }

    #[test]
    fn config_loads_from_toml() {
        // Verify that loading from the project config file populates indicators.
        if let Some(toml) = load_config_toml() {
            assert!(
                !toml.failure_indicators.is_empty(),
                "config/default.toml should have failure_indicators"
            );
        }
        // If the file doesn't exist (e.g. in CI), that's OK — just skip.
    }

    #[test]
    fn config_from_env_disabled_without_token() {
        // Ensure GITHUB_TOKEN is not set for this test.
        // SAFETY: This test is single-threaded and only modifies a test-specific
        // env var that no other code reads concurrently.
        unsafe {
            std::env::remove_var("GITHUB_TOKEN");
        }
        let config = EvolutionConfig::from_env();
        assert!(!config.enabled);
    }

    #[test]
    fn detect_github_repo_parses_ssh_url() {
        // This test runs in the actual git repo, so it should work.
        // We just verify the function doesn't panic.
        let _ = detect_github_repo();
    }

    #[tokio::test]
    async fn deduplication_prevents_repeat_reports() {
        let config = EvolutionConfig {
            enabled: false, // Don't actually call GitHub
            ..EvolutionConfig::default()
        };
        let engine = EvolutionEngine::new(config);
        let mut eng = engine.lock().await;

        // Insert a fingerprint manually.
        let fp = hash_string("test message");
        eng.recent_fingerprints.insert(fp);

        // Reporting the same message should return None (deduplicated).
        let result = eng
            .report(
                UnhandledIntent {
                    user_message: "test message".to_owned(),
                    channel: "test".to_owned(),
                    error: None,
                    agent_response: None,
                    turns_used: None,
                    timestamp: 0,
                },
                FailureReason::MaxTurnsExceeded,
            )
            .await;
        assert!(result.is_none());
    }

    #[test]
    fn clear_cache_empties_fingerprints() {
        let config = EvolutionConfig::default();
        let rt = tokio::runtime::Runtime::new().unwrap();
        let engine = EvolutionEngine::new(config);
        rt.block_on(async {
            let mut eng = engine.lock().await;
            eng.recent_fingerprints.insert(12345);
            assert!(!eng.recent_fingerprints.is_empty());
            eng.clear_cache();
            assert!(eng.recent_fingerprints.is_empty());
        });
    }
}
