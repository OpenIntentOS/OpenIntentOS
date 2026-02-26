//! End-to-end tests for the daily briefing adapter.
//!
//! These tests exercise the real `compose_briefing` pipeline including the
//! live HTTP call to the DuckDuckGo Instant Answer API for news headlines.
//!
//! Network-dependent tests are NOT marked `#[ignore]` because the news fetch
//! degrades gracefully — if DuckDuckGo is unreachable the test still passes
//! (the section falls back to "No headlines available").

use chrono::NaiveDate;
use openintent_adapters::daily_briefing::DailyBriefingAdapter;

// ── markdown structure ────────────────────────────────────────────────────────

/// The briefing must be a valid Markdown document with the correct H1 title
/// and H2 section headings.
#[tokio::test]
async fn briefing_has_correct_markdown_structure() {
    let adapter = DailyBriefingAdapter::new();
    let date = NaiveDate::from_ymd_opt(2026, 1, 15).unwrap();
    let text = adapter.compose_briefing(date).await.expect("briefing failed");

    // H1 title
    assert!(
        text.starts_with("# Morning Briefing"),
        "must start with '# Morning Briefing', got:\n{text}"
    );

    // All H2 sections present
    for section in ["## Calendar", "## Email", "## Tasks", "## News", "## System"] {
        assert!(
            text.contains(section),
            "missing section '{section}' in:\n{text}"
        );
    }
}

// ── date formatting ───────────────────────────────────────────────────────────

/// The date header must include the full weekday name, month name, and year.
#[tokio::test]
async fn briefing_date_is_formatted_correctly() {
    let adapter = DailyBriefingAdapter::new();

    // 2026-01-15 is a Thursday.
    let date = NaiveDate::from_ymd_opt(2026, 1, 15).unwrap();
    let text = adapter.compose_briefing(date).await.expect("briefing failed");

    assert!(text.contains("Thursday"), "must contain weekday 'Thursday'");
    assert!(text.contains("January"), "must contain month 'January'");
    assert!(text.contains("2026"), "must contain year '2026'");
    assert!(text.contains("15"), "must contain day '15'");
}

// ── system status section ─────────────────────────────────────────────────────

/// The System section must identify OpenIntentOS and include a version number.
#[tokio::test]
async fn briefing_system_section_identifies_product() {
    let adapter = DailyBriefingAdapter::new();
    let date = NaiveDate::from_ymd_opt(2026, 3, 1).unwrap();
    let text = adapter.compose_briefing(date).await.expect("briefing failed");

    let system_start = text.find("## System").expect("System section missing");
    let system_section = &text[system_start..];

    assert!(
        system_section.contains("OpenIntentOS"),
        "System section must mention 'OpenIntentOS'"
    );
    // Version is embedded at compile time via CARGO_PKG_VERSION.
    assert!(
        system_section.contains("v0.") || system_section.contains("v1."),
        "System section must contain a version number, got: {system_section}"
    );
    assert!(
        system_section.contains("memory OK"),
        "System section must report memory status"
    );
}

// ── news section (live network call) ─────────────────────────────────────────

/// The news section must contain either real headlines or the graceful fallback.
/// This test makes a real HTTP request to DuckDuckGo.
#[tokio::test]
async fn briefing_news_section_present_and_non_empty() {
    let adapter = DailyBriefingAdapter::new();
    let date = NaiveDate::from_ymd_opt(2026, 3, 1).unwrap();
    let text = adapter.compose_briefing(date).await.expect("briefing failed");

    let news_start = text.find("## News").expect("News section missing");
    let system_start = text.find("## System").unwrap_or(text.len());
    let news_section = &text[news_start..system_start].trim();

    // Section must have content after the heading line.
    let content_lines: Vec<&str> = news_section
        .lines()
        .skip(1) // skip "## News"
        .filter(|l| !l.trim().is_empty())
        .collect();

    assert!(
        !content_lines.is_empty(),
        "news section must have at least one content line, got: {news_section}"
    );

    // Content is either real headlines or the fallback message — both are valid.
    let joined = content_lines.join(" ");
    let is_headlines = joined.chars().any(|c| c.is_alphabetic());
    assert!(
        is_headlines,
        "news section must contain readable text, got: {joined}"
    );
}

// ── email section respects env var ────────────────────────────────────────────

/// When `EMAIL_ADDRESS` is not set, the email section must show the
/// "not configured" message rather than silently returning empty.
#[tokio::test]
async fn briefing_email_section_shows_not_configured_without_env() {
    // Ensure EMAIL_ADDRESS is not set.
    // SAFETY: test is not running in parallel with other tests that mutate this var.
    unsafe { std::env::remove_var("EMAIL_ADDRESS") };

    let adapter = DailyBriefingAdapter::new();
    let date = NaiveDate::from_ymd_opt(2026, 3, 1).unwrap();
    let text = adapter.compose_briefing(date).await.expect("briefing failed");

    let email_start = text.find("## Email").expect("Email section missing");
    let tasks_start = text.find("## Tasks").unwrap_or(text.len());
    let email_section = &text[email_start..tasks_start];

    assert!(
        email_section.contains("not configured") || email_section.contains("unavailable"),
        "email section should show 'not configured' when EMAIL_ADDRESS is unset, got: {email_section}"
    );
}

// ── calendar section graceful fallback ───────────────────────────────────────

/// Without a calendar backend configured, the section must show a placeholder.
#[tokio::test]
async fn briefing_calendar_section_shows_placeholder() {
    let adapter = DailyBriefingAdapter::new();
    let date = NaiveDate::from_ymd_opt(2026, 3, 1).unwrap();
    let text = adapter.compose_briefing(date).await.expect("briefing failed");

    let cal_start = text.find("## Calendar").expect("Calendar section missing");
    let email_start = text.find("## Email").unwrap_or(text.len());
    let cal_section = &text[cal_start..email_start];

    // Should show placeholder ("No events today") rather than panic or empty.
    assert!(
        !cal_section.trim_start_matches("## Calendar").trim().is_empty(),
        "calendar section must not be empty"
    );
}

// ── tasks section ─────────────────────────────────────────────────────────────

/// Without a task backend, the section must show a non-empty placeholder.
#[tokio::test]
async fn briefing_tasks_section_shows_placeholder() {
    let adapter = DailyBriefingAdapter::new();
    let date = NaiveDate::from_ymd_opt(2026, 3, 1).unwrap();
    let text = adapter.compose_briefing(date).await.expect("briefing failed");

    let tasks_start = text.find("## Tasks").expect("Tasks section missing");
    let news_start = text.find("## News").unwrap_or(text.len());
    let tasks_section = &text[tasks_start..news_start];

    assert!(
        tasks_section.contains("All clear") || tasks_section.contains("unavailable"),
        "tasks section must show a placeholder, got: {tasks_section}"
    );
}
