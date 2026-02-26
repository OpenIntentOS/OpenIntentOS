//! First-time onboarding flow for OpenIntentOS.
//!
//! Guides new users through selecting a use case, enabling plugins,
//! and configuring optional features like the daily briefing and Telegram.
//! Writes results to `.env` and marks onboarding complete.

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
/// Should be called after setup when `!is_onboarding_done()`.
pub fn run_onboarding() -> anyhow::Result<()> {
    let stdin = io::stdin();
    let stdout = io::stdout();
    let mut out = stdout.lock();

    writeln!(out)?;
    writeln!(out, "  ╔══════════════════════════════════════╗")?;
    writeln!(out, "  ║    Welcome to OpenIntentOS           ║")?;
    writeln!(out, "  ║    Let's set up your workspace.      ║")?;
    writeln!(out, "  ╚══════════════════════════════════════╝")?;
    writeln!(out)?;

    // Step 1: Use case selection
    let use_case = step_use_case(&mut stdin.lock(), &mut out)?;
    writeln!(out)?;

    // Step 2: Recommend plugins based on use case
    let plugins = step_plugins(&use_case, &mut stdin.lock(), &mut out)?;
    writeln!(out)?;

    // Step 3: Morning briefing
    let briefing_enabled = step_briefing(&mut stdin.lock(), &mut out)?;
    writeln!(out)?;

    // Step 4: Telegram
    let telegram_token = step_telegram(&mut stdin.lock(), &mut out)?;
    writeln!(out)?;

    // Persist choices to .env
    write_onboarding_env(briefing_enabled, &telegram_token)?;

    // Summary
    writeln!(out, "  ═══════════════════════════════════════")?;
    writeln!(out, "  Configuration Summary")?;
    writeln!(out, "  ─────────────────────────────────────")?;
    writeln!(out, "  Use case:         {}", use_case_label(&use_case))?;
    if !plugins.is_empty() {
        writeln!(out, "  Plugins enabled:  {}", plugins.join(", "))?;
    }
    writeln!(
        out,
        "  Morning briefing: {}",
        if briefing_enabled { "enabled (07:00)" } else { "disabled" }
    )?;
    writeln!(
        out,
        "  Telegram bot:     {}",
        if telegram_token.is_empty() { "not configured" } else { "configured" }
    )?;
    writeln!(out, "  ═══════════════════════════════════════")?;
    writeln!(out)?;
    writeln!(out, "  Onboarding complete! Run `openintent serve` to start.")?;
    writeln!(out)?;

    info!("onboarding complete");
    Ok(())
}

// ---------------------------------------------------------------------------
// Steps
// ---------------------------------------------------------------------------

fn step_use_case(
    stdin: &mut dyn BufRead,
    out: &mut dyn Write,
) -> anyhow::Result<String> {
    writeln!(out, "  Step 1/4: What is your primary use case?")?;
    writeln!(out)?;
    writeln!(out, "    (1) Developer workflow")?;
    writeln!(out, "    (2) Business productivity")?;
    writeln!(out, "    (3) Personal automation")?;
    writeln!(out, "    (4) Research & analysis")?;
    writeln!(out)?;
    write!(out, "  Enter 1-4: ")?;
    out.flush()?;

    let mut line = String::new();
    stdin.read_line(&mut line)?;
    let choice = line.trim();

    let use_case = match choice {
        "1" => "developer",
        "2" => "business",
        "3" => "personal",
        "4" => "research",
        _ => {
            writeln!(out, "  Invalid choice. Defaulting to personal automation.")?;
            "personal"
        }
    };

    writeln!(out, "  Selected: {}", use_case_label(use_case))?;
    Ok(use_case.to_owned())
}

fn step_plugins(
    use_case: &str,
    stdin: &mut dyn BufRead,
    out: &mut dyn Write,
) -> anyhow::Result<Vec<String>> {
    writeln!(out, "  Step 2/4: Recommended plugins for your use case")?;
    writeln!(out)?;

    let recommended = recommended_plugins(use_case);
    if recommended.is_empty() {
        writeln!(out, "  No specific plugins recommended. You can install skills later.")?;
        return Ok(Vec::new());
    }

    writeln!(out, "  Based on your use case, we recommend:")?;
    for (i, plugin) in recommended.iter().enumerate() {
        writeln!(out, "    ({}) {}", i + 1, plugin)?;
    }
    writeln!(out)?;
    write!(out, "  Enable all recommended plugins? [Y/n]: ")?;
    out.flush()?;

    let mut line = String::new();
    stdin.read_line(&mut line)?;
    let answer = line.trim().to_lowercase();

    if answer.is_empty() || answer == "y" || answer == "yes" {
        writeln!(out, "  Plugins will be enabled on first use.")?;
        Ok(recommended.iter().map(|s| s.to_string()).collect())
    } else {
        writeln!(out, "  Skipped. You can enable plugins later via `openintent skills install`.")?;
        Ok(Vec::new())
    }
}

fn step_briefing(
    stdin: &mut dyn BufRead,
    out: &mut dyn Write,
) -> anyhow::Result<bool> {
    writeln!(out, "  Step 3/4: Morning Briefing")?;
    writeln!(out)?;
    writeln!(
        out,
        "  Would you like a daily briefing at 7am?"
    )?;
    writeln!(out, "  It summarizes your tasks, emails, and news.")?;
    writeln!(out)?;
    write!(out, "  Enable morning briefing? [y/N]: ")?;
    out.flush()?;

    let mut line = String::new();
    stdin.read_line(&mut line)?;
    let answer = line.trim().to_lowercase();

    let enabled = answer == "y" || answer == "yes";
    if enabled {
        writeln!(out, "  Morning briefing enabled at 07:00.")?;
    } else {
        writeln!(out, "  Skipped.")?;
    }
    Ok(enabled)
}

fn step_telegram(
    stdin: &mut dyn BufRead,
    out: &mut dyn Write,
) -> anyhow::Result<String> {
    writeln!(out, "  Step 4/4: Telegram Control (optional)")?;
    writeln!(out)?;
    writeln!(out, "  Do you want to control OpenIntentOS via Telegram?")?;
    writeln!(out, "  Enter your bot token, or press Enter to skip.")?;
    writeln!(out)?;
    write!(out, "  Bot token: ")?;
    out.flush()?;

    let mut line = String::new();
    stdin.read_line(&mut line)?;
    let token = line.trim().to_owned();

    if token.is_empty() {
        writeln!(out, "  Skipped.")?;
    } else {
        writeln!(out, "  Telegram bot token saved.")?;
    }
    Ok(token)
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

fn use_case_label(use_case: &str) -> &str {
    match use_case {
        "developer" => "Developer workflow",
        "business" => "Business productivity",
        "personal" => "Personal automation",
        "research" => "Research & analysis",
        _ => "Custom",
    }
}

fn recommended_plugins(use_case: &str) -> Vec<&'static str> {
    match use_case {
        "developer" => vec!["git-helper", "code-review", "daily-briefing"],
        "business" => vec!["email-manager", "calendar-sync", "daily-briefing"],
        "personal" => vec!["daily-briefing", "task-tracker"],
        "research" => vec!["web-research", "summarizer", "daily-briefing"],
        _ => vec!["daily-briefing"],
    }
}

/// Append onboarding settings to `.env` and mark onboarding as complete.
fn write_onboarding_env(briefing_enabled: bool, telegram_token: &str) -> anyhow::Result<()> {
    let env_path = Path::new(".env");

    // Read existing content (if any).
    let existing = if env_path.exists() {
        std::fs::read_to_string(env_path)?
    } else {
        String::new()
    };

    // Build lines to append.
    let mut additions = String::new();

    additions.push_str("\n# Onboarding\n");
    additions.push_str("ONBOARDING_COMPLETE=true\n");

    additions.push_str("\n# Daily Briefing\n");
    additions.push_str(&format!(
        "BRIEFING_ENABLED={}\n",
        if briefing_enabled { "true" } else { "false" }
    ));
    if briefing_enabled {
        additions.push_str("BRIEFING_TIME=07:00\n");
    }

    if !telegram_token.is_empty() {
        additions.push_str("\n# Telegram (from onboarding)\n");
        additions.push_str(&format!("TELEGRAM_BOT_TOKEN={telegram_token}\n"));
    }

    let new_content = existing + &additions;
    std::fs::write(env_path, new_content)?;

    // Apply to current process so the check works immediately.
    unsafe {
        std::env::set_var("ONBOARDING_COMPLETE", "true");
    }

    info!("onboarding settings written to .env");
    Ok(())
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn use_case_labels_are_not_empty() {
        for case in &["developer", "business", "personal", "research"] {
            assert!(!use_case_label(case).is_empty());
        }
    }

    #[test]
    fn recommended_plugins_non_empty_for_known_cases() {
        for case in &["developer", "business", "personal", "research"] {
            assert!(!recommended_plugins(case).is_empty());
        }
    }

    #[test]
    fn is_onboarding_done_reads_env() {
        unsafe { std::env::remove_var("ONBOARDING_COMPLETE") };
        assert!(!is_onboarding_done());
        unsafe { std::env::set_var("ONBOARDING_COMPLETE", "true") };
        assert!(is_onboarding_done());
        unsafe { std::env::remove_var("ONBOARDING_COMPLETE") };
    }
}
