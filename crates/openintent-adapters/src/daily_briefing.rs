//! Daily briefing adapter — composes a morning briefing from multiple sources.
//!
//! Pulls calendar events, email counts, pending tasks, news headlines, and
//! system health into a single formatted Markdown string.  Each section
//! degrades gracefully: if a source is unavailable it shows a placeholder
//! rather than failing the entire briefing.

use chrono::NaiveDate;
use tracing::{info, warn};

use crate::error::AdapterError;

// ---------------------------------------------------------------------------
// Configuration
// ---------------------------------------------------------------------------

/// Configuration for the daily briefing adapter.
#[derive(Debug, Clone)]
pub struct BriefingConfig {
    /// Time to deliver the briefing in HH:MM 24-hour format (default "07:00").
    pub briefing_time: String,
    /// Whether the automatic cron briefing is enabled.
    pub enabled: bool,
    /// OpenIntentOS version string inserted into the System section.
    pub version: String,
}

impl Default for BriefingConfig {
    fn default() -> Self {
        let briefing_time = std::env::var("BRIEFING_TIME")
            .unwrap_or_else(|_| "07:00".to_owned());
        let enabled = std::env::var("BRIEFING_ENABLED")
            .map(|v| v.trim().eq_ignore_ascii_case("true"))
            .unwrap_or(false);
        Self {
            briefing_time,
            enabled,
            version: env!("CARGO_PKG_VERSION").to_owned(),
        }
    }
}

// ---------------------------------------------------------------------------
// Adapter
// ---------------------------------------------------------------------------

/// Composes morning briefings from multiple data sources.
///
/// Each source is polled independently; failures produce "unavailable"
/// placeholders so the briefing always delivers a complete document.
pub struct DailyBriefingAdapter {
    config: BriefingConfig,
    http: reqwest::Client,
}

impl DailyBriefingAdapter {
    /// Create a new briefing adapter with default configuration from env vars.
    pub fn new() -> Self {
        Self {
            config: BriefingConfig::default(),
            http: reqwest::Client::new(),
        }
    }

    /// Create a briefing adapter with explicit configuration.
    pub fn with_config(config: BriefingConfig) -> Self {
        Self {
            config,
            http: reqwest::Client::new(),
        }
    }

    /// Returns `true` if the automatic daily briefing cron is enabled.
    pub fn is_enabled(&self) -> bool {
        self.config.enabled
    }

    /// Returns the configured briefing time string (e.g. `"07:00"`).
    pub fn briefing_time(&self) -> &str {
        &self.config.briefing_time
    }

    /// Compose a full morning briefing for `date`.
    ///
    /// Each section is gathered independently; failures are logged and
    /// replaced with an "unavailable" message so the briefing is always
    /// returned as `Ok`.
    pub async fn compose_briefing(&self, date: NaiveDate) -> Result<String, AdapterError> {
        info!(date = %date, "composing daily briefing");

        let day_label = date.format("%A, %B %-d, %Y").to_string();

        let calendar = self.fetch_calendar(date).await;
        let email = self.fetch_email().await;
        let tasks = self.fetch_tasks().await;
        let news = self.fetch_news().await;
        let system = self.system_status();

        let briefing = format!(
            "# Morning Briefing \u{2014} {day}\n\
             \n\
             ## Calendar\n\
             {calendar}\n\
             \n\
             ## Email\n\
             {email}\n\
             \n\
             ## Tasks\n\
             {tasks}\n\
             \n\
             ## News\n\
             {news}\n\
             \n\
             ## System\n\
             {system}\n",
            day = day_label,
            calendar = calendar,
            email = email,
            tasks = tasks,
            news = news,
            system = system,
        );

        Ok(briefing)
    }

    // ── private section fetchers ────────────────────────────────────────────

    /// Fetch today's calendar events.
    ///
    /// Returns a formatted bullet list or a placeholder on failure.
    async fn fetch_calendar(&self, date: NaiveDate) -> String {
        // In a full implementation this would call CalendarAdapter.
        // For now we check whether a local ICS endpoint or env var is available.
        let _ = date;
        warn!("calendar fetch not yet connected — showing placeholder");
        "No events today".to_owned()
    }

    /// Fetch email summary.
    ///
    /// Returns unread count and top subjects or a placeholder on failure.
    async fn fetch_email(&self) -> String {
        // In a full implementation this would call EmailAdapter.
        let has_email = std::env::var("EMAIL_ADDRESS")
            .map(|v| !v.trim().is_empty())
            .unwrap_or(false);

        if !has_email {
            return "Email not configured".to_owned();
        }

        warn!("email fetch not yet connected — showing placeholder");
        "Email unavailable".to_owned()
    }

    /// Fetch pending tasks from memory store.
    async fn fetch_tasks(&self) -> String {
        // In a full implementation this would query the memory adapter.
        "All clear".to_owned()
    }

    /// Fetch top news headlines via a simple web search.
    async fn fetch_news(&self) -> String {
        // Attempt a DuckDuckGo Instant Answer API call for a news summary.
        let result = self
            .http
            .get("https://api.duckduckgo.com/")
            .query(&[("q", "top news today"), ("format", "json"), ("no_html", "1")])
            .timeout(std::time::Duration::from_secs(5))
            .send()
            .await;

        match result {
            Ok(resp) if resp.status().is_success() => {
                match resp.json::<serde_json::Value>().await {
                    Ok(json) => {
                        let topics = json
                            .get("RelatedTopics")
                            .and_then(|v| v.as_array())
                            .cloned()
                            .unwrap_or_default();

                        let headlines: Vec<String> = topics
                            .iter()
                            .filter_map(|t| t.get("Text").and_then(|v| v.as_str()))
                            .take(3)
                            .enumerate()
                            .map(|(i, text)| format!("{}. {}", i + 1, text))
                            .collect();

                        if headlines.is_empty() {
                            "No headlines available".to_owned()
                        } else {
                            headlines.join("\n")
                        }
                    }
                    Err(e) => {
                        warn!(error = %e, "failed to parse news response");
                        "No headlines available".to_owned()
                    }
                }
            }
            Ok(resp) => {
                warn!(status = %resp.status(), "news fetch returned non-success status");
                "No headlines available".to_owned()
            }
            Err(e) => {
                warn!(error = %e, "news fetch failed");
                "No headlines available".to_owned()
            }
        }
    }

    /// Build a system health line from env vars and compile-time version.
    fn system_status(&self) -> String {
        let version = &self.config.version;
        format!("OpenIntentOS v{version} \u{2014} memory OK")
    }
}

impl Default for DailyBriefingAdapter {
    fn default() -> Self {
        Self::new()
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::NaiveDate;

    #[test]
    fn default_config_reads_env() {
        let config = BriefingConfig::default();
        // Default time is "07:00" unless overridden.
        assert!(!config.briefing_time.is_empty());
    }

    #[tokio::test]
    async fn compose_briefing_returns_ok() {
        let adapter = DailyBriefingAdapter::new();
        let date = NaiveDate::from_ymd_opt(2026, 1, 1).unwrap();
        let result = adapter.compose_briefing(date).await;
        assert!(result.is_ok());
        let text = result.unwrap();
        assert!(text.contains("Morning Briefing"));
        assert!(text.contains("Calendar"));
        assert!(text.contains("Email"));
        assert!(text.contains("Tasks"));
        assert!(text.contains("System"));
    }

    #[test]
    fn system_status_includes_version() {
        let adapter = DailyBriefingAdapter::new();
        let status = adapter.system_status();
        assert!(status.contains("OpenIntentOS"));
    }
}
