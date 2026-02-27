//! First-run setup wizard for OpenIntentOS.
//!
//! Provides HTTP endpoints and an embedded HTML wizard that guides the user
//! through selecting an AI provider and (optionally) connecting Telegram.
//! When no LLM API key is detected, `cmd_serve` redirects to this wizard
//! instead of starting the main web server.

use std::path::Path;
use std::time::Duration;

use axum::Json;
use axum::Router;
use axum::response::{Html, Redirect};
use axum::routing::{get, post};
use tokio::time::timeout;
use tracing::{info, warn};

// ── public types ────────────────────────────────────────────────────────────

/// Response for `/api/setup/status`.
#[derive(serde::Serialize)]
pub struct SetupStatus {
    /// True if at least one LLM API key is present in the environment.
    pub configured: bool,
    /// True if Ollama is listening on 127.0.0.1:11434.
    pub ollama: bool,
}

/// Request body for `/api/setup/save`.
#[derive(serde::Deserialize)]
pub struct SetupPayload {
    /// Provider identifier: "openai", "anthropic", "google", etc.
    pub provider: String,
    /// API key for the selected provider (may be empty for Ollama).
    pub api_key: String,
    /// Telegram bot token (optional; may be empty).
    pub telegram_token: String,
}

/// Response for `/api/setup/save`.
#[derive(serde::Serialize)]
pub struct SetupResult {
    /// Whether the save succeeded.
    pub ok: bool,
    /// Human-readable error message, present only on failure.
    pub error: Option<String>,
}

// ── env-var check ────────────────────────────────────────────────────────────

/// Known LLM API key environment variables.
const LLM_KEY_VARS: &[&str] = &[
    "OPENAI_API_KEY",
    "ANTHROPIC_API_KEY",
    "NVIDIA_API_KEY",
    "GOOGLE_API_KEY",
    "DEEPSEEK_API_KEY",
    "GROQ_API_KEY",
    "OPENROUTER_API_KEY",
    "XAI_API_KEY",
    "MISTRAL_API_KEY",
];

/// Returns `true` if at least one LLM API key environment variable is set and
/// non-empty.
pub fn is_configured() -> bool {
    LLM_KEY_VARS
        .iter()
        .any(|var| std::env::var(var).map(|v| !v.trim().is_empty()).unwrap_or(false))
}

/// Returns `true` if the onboarding wizard has been completed.
pub fn is_onboarding_done() -> bool {
    std::env::var("ONBOARDING_COMPLETE")
        .map(|v| v.trim().eq_ignore_ascii_case("true"))
        .unwrap_or(false)
}

// ── API: GET /api/setup/status ───────────────────────────────────────────────

/// Return the current setup status: whether the system is configured and
/// whether Ollama is reachable.
pub async fn get_status() -> Json<SetupStatus> {
    let configured = is_configured();

    let ollama = timeout(
        Duration::from_secs(1),
        tokio::net::TcpStream::connect("127.0.0.1:11434"),
    )
    .await
    .map(|r| r.is_ok())
    .unwrap_or(false);

    Json(SetupStatus { configured, ollama })
}

// ── API: POST /api/setup/save ────────────────────────────────────────────────

/// Map a provider name to its environment variable key.
fn provider_env_key(provider: &str) -> Option<&'static str> {
    match provider {
        "openai" => Some("OPENAI_API_KEY"),
        "nvidia" => Some("NVIDIA_API_KEY"),
        "google" => Some("GOOGLE_API_KEY"),
        "deepseek" => Some("DEEPSEEK_API_KEY"),
        "groq" => Some("GROQ_API_KEY"),
        "anthropic" => Some("ANTHROPIC_API_KEY"),
        "openrouter" => Some("OPENROUTER_API_KEY"),
        "xai" => Some("XAI_API_KEY"),
        "mistral" => Some("MISTRAL_API_KEY"),
        // Ollama and empty provider need no key.
        "ollama" | "" => None,
        _ => None,
    }
}

/// Build the content of the `.env` file from the setup payload.
pub fn build_env_content(payload: &SetupPayload) -> String {
    let mut lines = vec![
        "# OpenIntentOS Configuration".to_owned(),
        "# Edit this file to update your keys, then run restart.sh".to_owned(),
        String::new(),
    ];

    // Telegram section — always written (may be empty).
    lines.push("# Telegram".to_owned());
    lines.push(format!(
        "TELEGRAM_BOT_TOKEN={}",
        payload.telegram_token.trim()
    ));
    lines.push(String::new());

    // AI provider section.
    lines.push("# AI Provider".to_owned());
    if let Some(env_key) = provider_env_key(&payload.provider) {
        lines.push(format!("{}={}", env_key, payload.api_key.trim()));
    }

    lines.join("\n") + "\n"
}

/// Write the setup configuration to a `.env` file at the given path.
///
/// Extracted for testability — callers can pass any path, including a temp
/// file during tests.
pub fn write_setup_env(path: &Path, payload: &SetupPayload) -> std::io::Result<()> {
    let content = build_env_content(payload);
    std::fs::write(path, content)
}

/// Save configuration to `.env` and schedule a process restart.
pub async fn post_save(Json(payload): Json<SetupPayload>) -> Json<SetupResult> {
    match write_setup_env(Path::new(".env"), &payload) {
        Ok(()) => {
            info!(
                provider = %payload.provider,
                "setup wizard saved .env, scheduling restart"
            );

            // Give the HTTP response time to reach the browser before exiting.
            #[cfg(not(test))]
            tokio::spawn(async {
                tokio::time::sleep(Duration::from_secs(2)).await;
                std::process::exit(0);
            });

            Json(SetupResult {
                ok: true,
                error: None,
            })
        }
        Err(e) => {
            warn!(error = %e, "setup wizard failed to write .env");
            Json(SetupResult {
                ok: false,
                error: Some(format!("Failed to write .env: {e}")),
            })
        }
    }
}

// ── onboarding types ─────────────────────────────────────────────────────────

/// Request body for `/api/onboarding/save`.
#[derive(serde::Deserialize)]
pub struct OnboardingPayload {
    /// Selected use case: "developer", "business", "personal", or "research".
    pub use_case: String,
    /// Whether to enable the morning briefing (07:00 daily).
    pub briefing_enabled: bool,
    /// Telegram bot token, may be empty.
    pub telegram_token: String,
}

/// Response for `/api/onboarding/save`.
#[derive(serde::Serialize)]
pub struct OnboardingResult {
    /// Whether the save succeeded.
    pub ok: bool,
    /// Human-readable error message, present only on failure.
    pub error: Option<String>,
}

// ── API: GET /onboarding ──────────────────────────────────────────────────────

/// Serve the onboarding HTML wizard.
pub async fn get_onboarding() -> Html<&'static str> {
    Html(ONBOARDING_HTML)
}

// ── API: POST /api/onboarding/save ───────────────────────────────────────────

/// Build the onboarding additions string that is appended to the existing `.env`.
///
/// Extracted as a pure function for testability.
pub fn build_onboarding_additions(payload: &OnboardingPayload) -> String {
    let mut additions = String::new();
    additions.push_str("\n# Onboarding\n");
    additions.push_str("ONBOARDING_COMPLETE=true\n");
    additions.push_str(&format!("ONBOARDING_USE_CASE={}\n", payload.use_case.trim()));

    additions.push_str("\n# Daily Briefing\n");
    additions.push_str(&format!(
        "BRIEFING_ENABLED={}\n",
        if payload.briefing_enabled { "true" } else { "false" }
    ));
    if payload.briefing_enabled {
        additions.push_str("BRIEFING_TIME=07:00\n");
    }

    if !payload.telegram_token.trim().is_empty() {
        additions.push_str("\n# Telegram (from onboarding)\n");
        additions.push_str(&format!(
            "TELEGRAM_BOT_TOKEN={}\n",
            payload.telegram_token.trim()
        ));
    }

    additions
}

/// Write onboarding additions to the `.env` file at the given path.
pub fn write_onboarding_env(path: &Path, payload: &OnboardingPayload) -> std::io::Result<()> {
    let existing = std::fs::read_to_string(path).unwrap_or_default();
    let new_content = existing + &build_onboarding_additions(payload);
    std::fs::write(path, new_content)
}

/// Save onboarding choices to `.env` and mark onboarding as complete.
pub async fn post_onboarding_save(
    Json(payload): Json<OnboardingPayload>,
) -> Json<OnboardingResult> {
    match write_onboarding_env(Path::new(".env"), &payload) {
        Ok(()) => {
            info!(use_case = %payload.use_case, "onboarding saved to .env, scheduling restart");

            #[cfg(not(test))]
            tokio::spawn(async {
                tokio::time::sleep(Duration::from_secs(2)).await;
                std::process::exit(0);
            });

            Json(OnboardingResult { ok: true, error: None })
        }
        Err(e) => {
            warn!(error = %e, "onboarding failed to write .env");
            Json(OnboardingResult {
                ok: false,
                error: Some(format!("Failed to write .env: {e}")),
            })
        }
    }
}

// ── root handler ─────────────────────────────────────────────────────────────

/// Redirect `/` to either `/onboarding` (when configured but not onboarded)
/// or `/setup` (when not yet configured).
async fn root_handler() -> Redirect {
    if is_configured() && !is_onboarding_done() {
        Redirect::to("/onboarding")
    } else {
        Redirect::to("/setup")
    }
}

// ── standalone setup server ──────────────────────────────────────────────────

/// Start a minimal HTTP server that only serves the setup wizard.
///
/// Binds to `{bind}:{port}`, serves the wizard at `/setup`, and provides the
/// two setup API endpoints.  Once the user saves their configuration the
/// server will exit after a short delay so the process manager can restart it
/// with the newly written `.env` file.
///
/// When setup is done but onboarding has not been completed, the root `/`
/// redirects to `/onboarding` instead.
///
/// # Errors
///
/// Returns an error if the TCP listener cannot be bound.
pub async fn serve_setup(
    bind: &str,
    port: u16,
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let app = Router::new()
        .route("/", get(root_handler))
        .route("/setup", get(|| async { Html(SETUP_HTML) }))
        .route("/onboarding", get(get_onboarding))
        .route("/api/setup/status", get(get_status))
        .route("/api/setup/save", post(post_save))
        .route("/api/onboarding/save", post(post_onboarding_save));

    let addr = format!("{bind}:{port}");

    println!();
    println!("  OpenIntentOS \u{2014} First Run Setup");
    println!("  Open your browser: http://localhost:{port}");
    println!();

    let listener = tokio::net::TcpListener::bind(&addr).await?;
    axum::serve(listener, app).await?;
    Ok(())
}

// ── embedded HTML wizard ─────────────────────────────────────────────────────

/// The complete first-run setup wizard as a static HTML string.
pub const SETUP_HTML: &str = r##"<!DOCTYPE html>
<html lang="en">
<head>
<meta charset="utf-8">
<meta name="viewport" content="width=device-width, initial-scale=1">
<title>OpenIntentOS — First Run Setup</title>
<link rel="icon" type="image/png" href="data:image/png;base64,iVBORw0KGgoAAAANSUhEUgAAAEAAAABACAYAAACqaXHeAAABY2lDQ1BrQ0dDb2xvclNwYWNlRGlzcGxheVAzAAAokX2QsUvDUBDGv1aloHUQHRwcMolDlJIKuji0FURxCFXB6pS+pqmQxkeSIgU3/4GC/4EKzm4Whzo6OAiik+jm5KTgouV5L4mkInqP435877vjOCA5bnBu9wOoO75bXMorm6UtJfWMBL0gDObxnK6vSv6uP+P9PvTeTstZv///jcGK6TGqn5QZxl0fSKjE+p7PJe8Tj7m0FHFLshXyieRyyOeBZ71YIL4mVljNqBC/EKvlHt3q4brdYNEOcvu06WysyTmUE1jEDjxw2DDQhAId2T/8s4G/gF1yN+FSn4UafOrJkSInmMTLcMAwA5VYQ4ZSk3eO7ncX3U+NtYMnYKEjhLiItZUOcDZHJ2vH2tQ8MDIEXLW54RqB1EeZrFaB11NguASM3lDPtlfNauH26Tww8CjE2ySQOgS6LSE+joToHlPzA3DpfAEDp2ITpJYOWwAAAARjSUNQDA0AAW4D4+8AAACUZVhJZk1NACoAAAAIAAYBBgADAAAAAQACAAABDQACAAAAEAAAAFYBGgAFAAAAAQAAAGYBGwAFAAAAAQAAAG4BKAADAAAAAQACAACHaQAEAAAAAQAAAHYAAAAA5pyq5ZG95ZCN5L2c5ZOBAAAAASwAAAABAAABLAAAAAEAAqACAAQAAAABAAAAQKADAAQAAAABAAAAQAAAAABhe/ibAAAACXBIWXMAAC4jAAAuIwF4pT92AAAEmWlUWHRYTUw6Y29tLmFkb2JlLnhtcAAAAAAAPHg6eG1wbWV0YSB4bWxuczp4PSJhZG9iZTpuczptZXRhLyIgeDp4bXB0az0iWE1QIENvcmUgNi4wLjAiPgogICA8cmRmOlJERiB4bWxuczpyZGY9Imh0dHA6Ly93d3cudzMub3JnLzE5OTkvMDIvMjItcmRmLXN5bnRheC1ucyMiPgogICAgICA8cmRmOkRlc2NyaXB0aW9uIHJkZjphYm91dD0iIgogICAgICAgICAgICB4bWxuczp0aWZmPSJodHRwOi8vbnMuYWRvYmUuY29tL3RpZmYvMS4wLyIKICAgICAgICAgICAgeG1sbnM6ZXhpZj0iaHR0cDovL25zLmFkb2JlLmNvbS9leGlmLzEuMC8iCiAgICAgICAgICAgIHhtbG5zOmRjPSJodHRwOi8vcHVybC5vcmcvZGMvZWxlbWVudHMvMS4xLyIKICAgICAgICAgICAgeG1sbnM6SXB0YzR4bXBFeHQ9Imh0dHA6Ly9pcHRjLm9yZy9zdGQvSXB0YzR4bXBFeHQvMjAwOC0wMi0yOS8iPgogICAgICAgICA8dGlmZjpEb2N1bWVudE5hbWU+5pyq5ZG95ZCN5L2c5ZOBPC90aWZmOkRvY3VtZW50TmFtZT4KICAgICAgICAgPHRpZmY6UmVzb2x1dGlvblVuaXQ+MjwvdGlmZjpSZXNvbHV0aW9uVW5pdD4KICAgICAgICAgPHRpZmY6Q29tcHJlc3Npb24+NTwvdGlmZjpDb21wcmVzc2lvbj4KICAgICAgICAgPHRpZmY6WFJlc29sdXRpb24+MzAwPC90aWZmOlhSZXNvbHV0aW9uPgogICAgICAgICA8dGlmZjpZUmVzb2x1dGlvbj4zMDA8L3RpZmY6WVJlc29sdXRpb24+CiAgICAgICAgIDx0aWZmOlBob3RvbWV0cmljSW50ZXJwcmV0YXRpb24+MjwvdGlmZjpQaG90b21ldHJpY0ludGVycHJldGF0aW9uPgogICAgICAgICA8ZXhpZjpQaXhlbFhEaW1lbnNpb24+MjUwMDwvZXhpZjpQaXhlbFhEaW1lbnNpb24+CiAgICAgICAgIDxleGlmOlBpeGVsWURpbWVuc2lvbj4yNTAwPC9leGlmOlBpeGVsWURpbWVuc2lvbj4KICAgICAgICAgPGRjOnRpdGxlPgogICAgICAgICAgICA8cmRmOkFsdD4KICAgICAgICAgICAgICAgPHJkZjpsaSB4bWw6bGFuZz0ieC1kZWZhdWx0Ij7mnKrlkb3lkI3kvZzlk4E8L3JkZjpsaT4KICAgICAgICAgICAgPC9yZGY6QWx0PgogICAgICAgICA8L2RjOnRpdGxlPgogICAgICAgICA8SXB0YzR4bXBFeHQ6QXJ0d29ya1RpdGxlPuacquWRveWQjeS9nOWTgTwvSXB0YzR4bXBFeHQ6QXJ0d29ya1RpdGxlPgogICAgICA8L3JkZjpEZXNjcmlwdGlvbj4KICAgPC9yZGY6UkRGPgo8L3g6eG1wbWV0YT4Kxt05LAAAGXxJREFUeAHFWwlwnsV5fv5LvyTrlmXJ94nvGwwkNsZgu2DCOQTCEShNAk1I0xyFwuSYZKaZdFpCk6YNTaEzSZPJUdIkDeYI4Ui4AwGMjQ/ZsuVLlmXJknVf/9Xn2fff/9BhJUymXfF9++7ue+/77u63vwnU725K4SwloLFAAKlUCoIDgSCSqaSjCBFOpOFgGkfMDAZS/Avoj4RJ0qtozMOBNM3IfkfPTsnMhcfC87zydQk6WskXvYrwJE8tTxOk/kE3OsHLWEyA9P8yPJ5mZ53TPE3Dea2chmNNj8lD8hzfznvmRfObPOhFBYkiWI/vz40A+p0jFgGqDY+RpQaLaHzL5AnHZs342lvhZNoYL9PQ6I2TRalRG736xdtHgGAvf1wHOMUYNlJbAl0IOTibAmyy3wxzIojv6EhF0ImRup5e+Co+BIXrYWF7Xt4VLgWc6hwVQxbxEmytLI3GMumYwWEKOAqv/0hdvPtE/R6KKfEeCP8PSSbScdwIcL6iu7MpYN638FSvX2A0JxaOEqYns3ARdhFgKGyxnQ5hD1syGY1JE2x8xMvje965/GyB8+mowM6FxYMz7NJX0nwKiJMV8R7TAdLX5R9xxcS1MnQcSYejcLJwNpydso5WgozQ47meDD1HM3AuL1sNbChL7yCnjiDL5Cw92xlegp3WaelZOSbfvZ1lYzpAw2a2ICsy1ooJFizn+N7cWrA9eouTtVVbS5CVfDprGa1oRtOLKouVq4uw/YjBevs1zI8ZvcfDeBFANdOaKuQUkiqqLAWsQ7BfsKw/iyMME0MObBiFcXGw+hy9+nwKGFZuCjliIZCZRkVjfI3GInQs2OQL37garYfFQyNjRoAb9FJIIdA3CWbgADtdf86gQAlR7eFMg30QkUp+lYfiaPlS7dEz3NL9noVwRsM2346eg6o9nsc15uNEgEdybNKUPr/EysMpapeBJYSJ54X5HBQvecRGrOFhh5vJ25G80jTpKTM10u8xacamNwrTzenCV67OY0aAQtZZ4oLEQkhd9qhtWhmUhb0AP6624fhANR7ZdxZzFC+xpfYOI22wxzGJnndWvufmay/fao/vpZtrRjkgw84BprgcoqZ70rBjOgJWn4pzoIGOBsEgwmGuF1xMlJPJhJamFELBEFLkkUpy9hKs41KKMiVIYJp/Ks1Q3QJVq5wNNgzDcbg5+EbvuI1eA8wvCifpkHYAibMnQZ2+DEsYWZgt9mdGCARCzLGCEAY6h9C05zSO721D69EudHcMEjmJaHEBKqZOwowlVZi5dAqqZpSShifNWO5J0kwRZ7+ie5laDseCReH1N+pc/bPbtRw4KgI8gXOzt4ad3uuZ8bMAmuVIYQgdx7rx+n834M0nG9F8uAvD4DSzKERljBmUQhHh8tooFqybgvM+sBBLN85xjosNx4U+oog6q9jZ9Mody4VzGQb853AGgcr705O8qH4XtmzkwmIivMz5nbAiQDOIRAAv/3AffvXIDrS39SAaDiHA8E+yf5CP1K+uTKKyOomaaQl0tIXIi3OcTCA2HMDKS6di5QcuwKwVUxEfTji+kifTvS5qhygzISVHwG4bZbeixtuiSCG6s8FHjeOY6wBjZQaLqTPICc0JIX61ZY3OhakcjYwPxvDCv72C3a/Uo25mEr2dERQWp1BYlMTAQBB102OIxwM4czqM+l0RtJ+yaJA8FR1mk4yUwpIQPvBXK7HlL9bScexLJp3y491H5N8HaDLMAXKYJs4ckIUly439qRwQ5iwP9sXx6288j90vNKJ/IIKeziCNZUBQhUgohemzk5i1IIYjB8OYPosDbgVMYccbUQxxWZDxUlaFqwDp4thyxyJcf98GGsQRLpZ/cgfsT98IyTeSIQXkGQcpbNTD//zemauApYCFVpih/6O/fRHP/rKeC0vY8bHZBIompTBtRhJDQ8CRI2Fw7XdOKWVU1LJ/1bohNOwpwMH6MIaJIzoVRWSMf1vuWIgPfX4Tdwmpkpt2uRFot1MZ/ceIAEdNBEsBcWe6q8o8NFiD9jAQuVLrUcj7WnD2EW4SwYIg3uRi95tf7kcBjefi74wQX+50WLoyjq4zARyj8QWUpnHVgwMBNDaEsP2/ip0O197Si8oaOceKjInw77nvHcCz39uBYFQ5P1qvXN08nIvn4azeskErhP9+TAt8L1UgFEBvSz+e+PYOkluOZfhwKjds5uzuDaOtlWcBZ2ZmlNjpbYgW79kZxmOPlmDj1gEsXx3jjqFRceRiR8rt33wHh3c2u93BDfyJXu47R2or7BXS2YcXhro0HPUIR/2GG41GsOMXh3CsuZNq+uC1qJo7P4F9OwvQ3RVws342nbUfD/bBRUNBFLjyuj4kab2LIuo30J/AY//0FheUJA9QQfdECyMo4lmiIMqki4QQLQy7diRCTaijFsax9TfdZXNYAkwM3wwLFfM9A4Rt5xz2KWTUT1IX9g6PW9dA1yBefewge/ONLy3nys/V/1DDxMaLl84I+rLUiv+71wPYuDGIFWvj2P02XUO1InTh3ldasOulRqzecg4Gegax58VjOPB6C1oPd2N4II7iygLMWl6NlZfMxrSFVVyAuZRq4SR/098iVLCK+sc/CDmUs79CkSCO7GjH8YMdVC/rAG1YcxfGse8diwmJ0xNPG5kfKRzhTGz54BKcu20GyicX48iuM/jRAztw/vltWHFuCm+/yVkmB9qCV366H1MXTMYPPv8S9r3W7Pj6SZKM13/ViKce2on3XX8OrvzUWkyqLEKCZ4nxSsB2AQsJLQoqCm9BmQigqzIRwLDSYqJSyPB7mrn/46//jgqaLyVq+gzOJteG40eZvdz+qmcFMWtZJabOrMWel1pxYHdLFp9RdM3dyxCqaMeOl+sRKQjjig9vwKKli3H/5Y9j2YqTOH2qAPv3FdHJKVTWBlBZF0X9zj5Ex0ks2TFMdy9YUYuPPLAJU+dXYpinSueotC2yT+30LuDMpcEyeuxHBmvGI8yzSJE9ah/d066hTNGiUj0lhdMtTIpgHMHJJ9CNA9i581XsqX8Vd35jDTbfsMSFfIK7/dzF1ejoa8Sj334Gh/eeRNuJbhw70ImKaQW49fPn4q3Xo9wqh1BYqJMBD1oDMZxp6mZKjB+8MixKjEPvtuI7n3gOHS297pCWa5sM1V+GS9oFzhCDFbaCmH/REIaYYwffPoVju07jzMk+hNk3bUE5mg+2Z8Jf2EVFxnhgiKt+eRtSkR7SgkZMxjUfuRIDgwnc+Y/vx+6XTqClpQclk5nbbx/kSTHKWYrhfZedi9vv+SDhPixbX8vTYyGdmcBFm7vx5BOVqKhOIMSVqz3f705P/5IedD21L8CBw614gcfya+49D7EBSwVvV2YNEGCP3hYarmZaa3Xd9ewxPPXwThx5pw3DSftAkZDSAvBkJ0wf/gGe9uJob9XZno6bxEDk6SrB4+CM+VOx4Yrz0HzkFIrLQ1i+fhpO/GwvYkM6J1rKafXe+epebP/PZ1BUEsT8BfPp3CT27y5GWVU3yovj7E+irTmSppDsbNHaE6PhZYWDWLysHXMWAkPhqaibN8l9brsUILpZaYsjv9KtqYXI1CACu9xpkMAvH3gTTz2y0x1AtBL7XJfYmE5mwQTCdJSWBfk8yb2r83QABUyPmumVOH6sy+X1njcO4F/u/y7HE7jnn29xIalVv/XwIGZeMAOnT77LlAnjeEMzHvrS9/H3P/kkXvufwzSIM56KoLMtwl2hH93dQW6J/ucOM14u1IxXlQ1g/cXNuPymFGqWrUZs0rlIhGZwNwghFiOWbJRtJPN7GoPJwlz54bcHsQ3TgMcefBPbH37bbUEyfmThZ7tzWt30BJqOhxxGaXmS4RvizMcwd95CpkAfDu85yT1+CL9+9CXc883bOBth7H3tpOPb0daHySersf6y83HyRDOPzVFc9qH16G6K4ucPveocrolp2F+ELVedQTwRQe9AiLSmt2Z9wcIebLvuONZtYtDPu4xrzkVoj5cgNcR4CAjDTn4yXE/GTjYya0CucbrEaOD++qtHdjklM1GSi0RYLjl6KMxQ47mNDghHUujlDOnCR4Le/W0Hbv+7a9Ez0IqBvkFcsHUlFiybh299/Bk0N3XROHPq/rfa0XGiHIsvWI3ymkn4zX+04Z0XeOhh8XM9zM/oMLU9w0jIbrjasVIoKYnhwJ4ynO4uw5rNnahbfpSLLOM/7STHaIyXc8j+PSf0DeS2Pu2zKlGerB756+fx4pMN9JAEMrRpUgHfI4s2l2Ur+zjrARxpKMbcc7hY1tupQKt8SWkhtnx4CRadX4eu9n688Og+7HnjlHOs5xUpSGFy7RCOHI+wS+dzfj3mRJxCvK5uGItW9eO3T1dQp7SiaQbKe+knLcuL+3H7XU3Yenst2qIf5WmyjCM+ApQCxFK4s7iU8J/D2vvdXk9vxLhS/9227agqP4xrP3QCZeXDePbJmXjxN3VOUNi909JVcSKXru5Da3OUq3kQxxqz50KlmE55EskjCTZf3YOnH6shiSkRo3qr1/Zhw9YefOsf6rh95RunVoAzv3hFH5qPRtHZkXuMkvD8YgEP3PeFN3HejefiVPhOShgedaYRlTvO55OzRUfEBpOYOacHn32wCGtvuwkLL9uKv/nqUdz35bewbHkHz9taULjKRyiOtY6vRxoKseqCHrqGbYr0RekT5ZwV8kkEopgxN4my0njGTEXQhZt6seuN4hwqo5bxGr/k8i4M9IbQ3pEf/l5Gbu1/sP/Rd5cg0fJ7ym7i8Oj1y9MEDjAF1NBHg4sANdiTGjqN0KRq5nMhjeSHRfw4qhKPI9izE52tQ5yNAvzg4QVcnEp5mrLv+5KyONZc2I/Geoo9pnOarbViqaIvvG1X8aOJa8XPf17lcnnpon7c+pft+Nr903hkza42CvuCgiTWbehBE2UdPWQnQeM08Vv0X33wdcy7/GacCWyRQewZnQJhnw+aNw8rNFBcw21Oc8mrGraHAjU4GbkTRVPOoDD573j863Hsry/jvNq6IGN7u8PY8btJnNFuTCpN4PCBQm4/SgdzhFbuF54pw933ncK1qQ5094Rw+dVd+P5Dk7mG2OeyJOqkMWfuEGqnDaGluQBHaPzIvJ/IBdoWW08WYkFg0OziPu0cQMKMnYRzF9Q8ninewblQcL0yQGHPG9ze5/GdLw7j5demZYz3hHJCP3eBZx6rdKnxZ9d1YtosHmOLky6UpcLAYBAPPzCFKzpviaYN49tfq8W++mLHQrNWy74Nl3bzQKVbomI01I82XiHrHy97ZK0lMczFNenunF2Qj0Rx7bCbbYJKAW+w+kyADFfYaCSEstA+PP2dd/DCS7OZ02N/YSn7NVv1PL1pXaibOYxLr+xCa0uEHzURDPUH0H0mjOe3VzA64I62c2cPopaXpRXVcbS1hNF0KIqjXPDkUL/fO23Fm+lsM8kbZN0yE0s4PveFJ92joQSdyAu1wBRiqM82VNnii3aBnBTQ9uC3ubQDmNsmjN/04WG0vvU0fvaDKRQ4cZET4kMBHD1YyM/lqEuJqTNiKJsap1NiqKmNo+VEmDtM0l2La02pf6cIvf28Qif7kYZLoiJo4ZJe3HF3AwrK+Z3QO4h3Xk7isZ/NRk+/zqimv/BqqgZQNzuE3uQU9nLRHScFuMFMXDT7hckG/PS7PIoOaqvyjpqYVrOo0sd837/PMlkGqlcxJ05qC3Zhm8Znc1TRLO/cXYJnn6rGXV9JoLP0blxz0etYe9FrePDLM9F8ssQ5Qak0Y1YPCiqnMgIqKWx8fflznd0FnK2O8IOoo/EYfv9yyZgzM0rTMTpkpMzXzPrah7jaUlEzN1ER7lPbZ2H/y00oiB3EKdyKKZfchXu/2oTyUi54zo08W6xrR6J0DVsFtt+PsNO+C3QfwKmY6AmFYzj4Zjs6B6J5ueaV1WzqNDbESLEnNyM91vi1Zmzr5ScxY3ofg3XcddkxkIuE03R0EqKJBv77gUF0Di/C9IuuxKYtLdxqgyiNDmP1+jh6AmtpXGxM+3xn9mOIns18GFGIjLK2vNSPY3s72eb374gigcWRBNasPoXlK9tROXkYp1pK8Ysfz3HH44nnVHJ42NnWgqtvasYXPrUCPT36SpAG4xcb1SUJqVPD6AuvwaSatzgR2l346bxgNTpTddTOf75n+eXaGVYoqLiQoCIedoD61ccc6ul0kO926mkmLryQQfjRRsxZWY34pNWIR6Yh1fwcnnt8EK3txRMaopCtLBvC9Jk9KF16JT5+XyO+8eUq9zvieM7TYjeldpCf6Dw9cvcSXixehlVbZ2PKD4/w90n+wFJ6sbvuymhNJFvQVWdL3iLoB3wtNLcfBItQUFpGo3lvTaNVZPwHbziED3+6G30Vt+BkcD03xiL6KoSqgp1c3Qdxig6YqCh1lK+T65Joil+EVdfPwrW7nsBPHp0z5mKrtaJ0Ei9Y5g5iKHiOmxzpm4gnUL1qM+791+1oaqpGLDwTgYQuLNLW+ADIMU4O4RrAgEg/OiGN9SR4ITF9xVyi22qqsL94YzNu+9wA2iruR0dgM+nomOQAHcDR4smoqBqacFHT7JcWxnD9LYcwGF2DYZ42u7AW13yyFEsWtLt1ZaQD5fhFSztQNWsK+kIr6ICY01nb3MDwJNS8/yasu3krHUJ1cmzzNubWSqD0DyMWFhYio9/xWArLNi9G7RQebUmkReaGj51BZ9XnqPR03tgMukiRc91sFC3CjDm2Io80ILetD52b7ziEectSOBO9gS26OMHLlGm34pZPdPOmKf+wpUkM8TR6xXXH0F+yhRFXQgqtICZXH8VJ6hrn74u+b2Qt+b5PMHc/is08HOToyIeuREVdJW784jp+yADTpnKRWb4Ng6k5nA9+0GboGVJMjuHIUsxdVelgCRmraLfYeNEpXHdzI9oLrkcsJF68Tg8kMJCYhfkbz8WCOV3kJnWtiOaqaxqxZksFugIbHf9c2QZ7/XPtGg3LSOGPOAn6RNHKzIchZN6iV3lHsHLbYnyat7cHX25Ab8EFjDGGOXF8ceFFysFYKeasW4iqkkZ095ZQ0SyOcJX3C+Z24e5730Vv6VXoil6BYA6vQIJxVroEs+bXY1+jNEi57XXD+5rx5585jdNF9/DKTUdlXrlk5Fv6ZnURlemvPm+Lhz2erWi+NW5tsxDnDe7iTfNw+f1bGWz8TM4IzyfUP4Iqm7sc6y7qV3zkDWrPrygbxGe/uA/hOVfjdPQWaqe1Jeskd4MTrkDpZLlOFyr8mWzZaXzmK8fQU/MpDHDxs2/GPNbvqcGTID9X3cPzPmv/g2IubKdEw1OO8Z6ReAz3DK3nYXWICMPB2bjklhkoDnNhTKumOhBM4uOfPYja9TeiPXIbTRMfz8vDdEewChUzapTVqKkewGe+1ID4zI/R+NWcecaQo/H4kut55OuSj+fxrVYKBLO/mfNSgzMx8vf17LjuC+zxeB7X9+fWw8Mp1J6/Geu3JjiDVhQNGy9pw8obr0RrYgsnfoA8Fcaet9+FkkyjCOadP5NH5zhW8ZBVs3Q1zqTOJ4193+fvVqIXreeTW3ueo2ulxR+VAmk7/qAqQMOGMA1b71qC8qJ+zmMARZE4Nn3sYvRELmH6cKk+S0lyX69dvgw33dGGc1YF0BG9ljR2qjsL2R89xPsA84Fqn60KDRVbRAi7/wzPxtKLY5pWuAo14SvMtbcKI8lfhGpXvB9rLu3Ec0/wmFxeior559lv/JKXlmk0kmnnDKcTO2M8yl7zuXWsy9Cb4HYb0I7j50x7uGhULKQp2bVMl7T+aRSXfmaI09HI8lJg7H964sPc1z7Usu3ccPOwhaMOJ8OJImy66xLU1pago6MPjTtPujtKT+9rC18fpulUTMbQEd7GiHk/f1HSjuP5q/a4JsvrlY8zEi+XhpNFHpl/Jyivi1gl40Eh6I9elAA5UxEg2PBG0LDfz6ZtiZobLq78lam9qQetx7swc2E1isp5+U3cfJkjedkM5usi+fl44+viI8BCwBkrC/ifYBXZknMlJkQfQiK2kDYH+ABirwtBOcqHnQnIKCouZCwKwyKc4L8RmF6KKbPKmBbKAG6G5CM8k+nDeSQvkyFecq1GLQUNT7BPCOM1kt7ji14ac5z/GT8bC3tvSIBCVsUbb2PG1OM5VZwHRcF9Ou1Nx5SwZ66ZEexLkjfMOp9r1o3G03uZFp6SZrycJMLmTC9HDsvCxMnIJ78MrH6j9/L9mNo+agTzPsCKEzoC9mPqzsJZw8ajyeKOz9vz9Lieq+c5Vm3csrp4Z43mZcZ73n7c0+fWI1LAZtuHsBhYChiJRi0ErT1R2AnLh51xtvRQr0ouvfjm4ngH5Ovi5VtUeBqvl9/H8mjS6Sh5XhfBKmozBazhBapl4ZP1tKY/jUZAwq3D0aQHHJymdYa4/vQgBTk5HPC8c+WIXZbGZOXy87BonGjP1rdZiz7PFuLkouU13ABfJMq/Ektz0M/FwsnkjdqeOz9HPSxHOGcIN43vatf2I2x4PA7m/W82ebwMy7AJC1d/o3RxvUIzme6dxs3Avu3Q0rjSzEquZpl/IaJw8CHiauFSeLpXYPrxPX5MIwY7IA+2ldsobdSoszReVpbreLyMSz59Fje/3/hLouc/EpaLhMU7QQ0RkbUn831yqPr8mMHeqGy/p1ftimdk1I6J7/K8hDcSNmLrHxMeQeMYpA35Q3g5nl6RtG7/C/sDMzm+dusZAAAAAElFTkSuQmCC">
<style>
*,*::before,*::after{box-sizing:border-box;margin:0;padding:0}
:root{
  --bg:#1a1a2e;
  --bg2:#16213e;
  --bg3:#12192e;
  --accent:#e94560;
  --green:#4ecca3;
  --text:#e4e4e4;
  --muted:#8a8a9a;
  --border:#2a2a4a;
}
body{background:var(--bg);color:var(--text);font-family:-apple-system,BlinkMacSystemFont,'Segoe UI',sans-serif;min-height:100vh;display:flex;align-items:center;justify-content:center;padding:1rem}
.card{background:var(--bg2);border:1px solid var(--border);border-radius:12px;padding:2rem;width:100%;max-width:480px;box-shadow:0 8px 40px rgba(0,0,0,.4)}
/* dots */
.dots{display:flex;justify-content:center;gap:.6rem;margin-bottom:2rem}
.dot{width:10px;height:10px;border-radius:50%;background:var(--border);transition:background .25s}
.dot.active{background:var(--accent)}
.dot.done{background:var(--green)}
/* headings */
h2{font-size:1.4rem;font-weight:700;margin-bottom:.4rem}
.subtitle{color:var(--muted);font-size:.9rem;margin-bottom:1.5rem}
/* provider grid */
.grid{display:grid;grid-template-columns:1fr 1fr;gap:.6rem;margin-bottom:1.5rem}
.provider-btn{background:var(--bg3);border:2px solid var(--border);border-radius:8px;padding:.75rem .6rem;cursor:pointer;text-align:left;color:var(--text);transition:border-color .2s,background .2s}
.provider-btn:hover{border-color:var(--accent);background:var(--bg)}
.provider-btn.selected{border-color:var(--accent);background:rgba(233,69,96,.1)}
.provider-name{font-weight:600;font-size:.9rem}
.provider-badge{font-size:.72rem;padding:.15rem .4rem;border-radius:4px;margin-top:.25rem;display:inline-block}
.badge-free{background:rgba(78,204,163,.15);color:var(--green)}
.badge-paid{background:rgba(233,69,96,.15);color:var(--accent)}
/* ollama badge */
.ollama-detect{background:rgba(78,204,163,.1);border:1px solid rgba(78,204,163,.3);color:var(--green);border-radius:6px;padding:.5rem .75rem;font-size:.85rem;margin-bottom:1rem}
/* inputs */
.field{margin-bottom:1.25rem}
label{display:block;font-size:.85rem;color:var(--muted);margin-bottom:.4rem}
input[type=password],input[type=text]{width:100%;background:var(--bg3);border:1px solid var(--border);border-radius:6px;padding:.65rem .75rem;color:var(--text);font-size:.95rem;outline:none}
input:focus{border-color:var(--accent)}
.hint{font-size:.78rem;color:var(--muted);margin-top:.35rem}
.hint a{color:var(--accent);text-decoration:none}
/* buttons */
.actions{display:flex;gap:.75rem;justify-content:flex-end;margin-top:.25rem}
.btn{padding:.6rem 1.2rem;border-radius:6px;border:none;cursor:pointer;font-size:.9rem;font-weight:600;transition:opacity .2s}
.btn-primary{background:var(--accent);color:#fff}
.btn-primary:disabled{opacity:.4;cursor:not-allowed}
.btn-secondary{background:transparent;color:var(--muted);border:1px solid var(--border)}
.btn-secondary:hover{color:var(--text);border-color:var(--text)}
/* telegram steps */
.tg-steps{list-style:none;counter-reset:tg;margin-bottom:1.25rem}
.tg-steps li{counter-increment:tg;display:flex;gap:.75rem;align-items:flex-start;padding:.5rem 0;font-size:.9rem;color:var(--text)}
.tg-steps li::before{content:counter(tg);background:var(--bg3);border:1px solid var(--border);border-radius:50%;min-width:1.6rem;height:1.6rem;display:flex;align-items:center;justify-content:center;font-size:.8rem;font-weight:700;color:var(--accent)}
.tg-steps code{background:var(--bg3);border:1px solid var(--border);border-radius:4px;padding:.1rem .35rem;font-size:.82rem;font-family:'JetBrains Mono',monospace}
/* done screen */
.done-icon{font-size:3.5rem;text-align:center;margin-bottom:.75rem}
.done-title{color:var(--green);font-size:1.5rem;font-weight:700;text-align:center;margin-bottom:.4rem}
.done-sub{color:var(--muted);text-align:center;font-size:.9rem;margin-bottom:.5rem}
.done-count{color:var(--muted);text-align:center;font-size:.85rem}
/* step visibility */
.step{display:none}
.step.active{display:block}
</style>
</head>
<body>
<div class="card">
  <!-- logo -->
  <div style="text-align:center;margin-bottom:1rem"><img src="data:image/png;base64,iVBORw0KGgoAAAANSUhEUgAAAQAAAAEACAYAAABccqhmAAABY2lDQ1BrQ0dDb2xvclNwYWNlRGlzcGxheVAzAAAokX2QsUvDUBDGv1aloHUQHRwcMolDlJIKuji0FURxCFXB6pS+pqmQxkeSIgU3/4GC/4EKzm4Whzo6OAiik+jm5KTgouV5L4mkInqP435877vjOCA5bnBu9wOoO75bXMorm6UtJfWMBL0gDObxnK6vSv6uP+P9PvTeTstZv///jcGK6TGqn5QZxl0fSKjE+p7PJe8Tj7m0FHFLshXyieRyyOeBZ71YIL4mVljNqBC/EKvlHt3q4brdYNEOcvu06WysyTmUE1jEDjxw2DDQhAId2T/8s4G/gF1yN+FSn4UafOrJkSInmMTLcMAwA5VYQ4ZSk3eO7ncX3U+NtYMnYKEjhLiItZUOcDZHJ2vH2tQ8MDIEXLW54RqB1EeZrFaB11NguASM3lDPtlfNauH26Tww8CjE2ySQOgS6LSE+joToHlPzA3DpfAEDp2ITpJYOWwAAAARjSUNQDA0AAW4D4+8AAACUZVhJZk1NACoAAAAIAAYBBgADAAAAAQACAAABDQACAAAAEAAAAFYBGgAFAAAAAQAAAGYBGwAFAAAAAQAAAG4BKAADAAAAAQACAACHaQAEAAAAAQAAAHYAAAAA5pyq5ZG95ZCN5L2c5ZOBAAAAASwAAAABAAABLAAAAAEAAqACAAQAAAABAAABAKADAAQAAAABAAABAAAAAABAwKBiAAAACXBIWXMAAC4jAAAuIwF4pT92AAAEmWlUWHRYTUw6Y29tLmFkb2JlLnhtcAAAAAAAPHg6eG1wbWV0YSB4bWxuczp4PSJhZG9iZTpuczptZXRhLyIgeDp4bXB0az0iWE1QIENvcmUgNi4wLjAiPgogICA8cmRmOlJERiB4bWxuczpyZGY9Imh0dHA6Ly93d3cudzMub3JnLzE5OTkvMDIvMjItcmRmLXN5bnRheC1ucyMiPgogICAgICA8cmRmOkRlc2NyaXB0aW9uIHJkZjphYm91dD0iIgogICAgICAgICAgICB4bWxuczp0aWZmPSJodHRwOi8vbnMuYWRvYmUuY29tL3RpZmYvMS4wLyIKICAgICAgICAgICAgeG1sbnM6ZXhpZj0iaHR0cDovL25zLmFkb2JlLmNvbS9leGlmLzEuMC8iCiAgICAgICAgICAgIHhtbG5zOmRjPSJodHRwOi8vcHVybC5vcmcvZGMvZWxlbWVudHMvMS4xLyIKICAgICAgICAgICAgeG1sbnM6SXB0YzR4bXBFeHQ9Imh0dHA6Ly9pcHRjLm9yZy9zdGQvSXB0YzR4bXBFeHQvMjAwOC0wMi0yOS8iPgogICAgICAgICA8dGlmZjpEb2N1bWVudE5hbWU+5pyq5ZG95ZCN5L2c5ZOBPC90aWZmOkRvY3VtZW50TmFtZT4KICAgICAgICAgPHRpZmY6UmVzb2x1dGlvblVuaXQ+MjwvdGlmZjpSZXNvbHV0aW9uVW5pdD4KICAgICAgICAgPHRpZmY6Q29tcHJlc3Npb24+NTwvdGlmZjpDb21wcmVzc2lvbj4KICAgICAgICAgPHRpZmY6WFJlc29sdXRpb24+MzAwPC90aWZmOlhSZXNvbHV0aW9uPgogICAgICAgICA8dGlmZjpZUmVzb2x1dGlvbj4zMDA8L3RpZmY6WVJlc29sdXRpb24+CiAgICAgICAgIDx0aWZmOlBob3RvbWV0cmljSW50ZXJwcmV0YXRpb24+MjwvdGlmZjpQaG90b21ldHJpY0ludGVycHJldGF0aW9uPgogICAgICAgICA8ZXhpZjpQaXhlbFhEaW1lbnNpb24+MjUwMDwvZXhpZjpQaXhlbFhEaW1lbnNpb24+CiAgICAgICAgIDxleGlmOlBpeGVsWURpbWVuc2lvbj4yNTAwPC9leGlmOlBpeGVsWURpbWVuc2lvbj4KICAgICAgICAgPGRjOnRpdGxlPgogICAgICAgICAgICA8cmRmOkFsdD4KICAgICAgICAgICAgICAgPHJkZjpsaSB4bWw6bGFuZz0ieC1kZWZhdWx0Ij7mnKrlkb3lkI3kvZzlk4E8L3JkZjpsaT4KICAgICAgICAgICAgPC9yZGY6QWx0PgogICAgICAgICA8L2RjOnRpdGxlPgogICAgICAgICA8SXB0YzR4bXBFeHQ6QXJ0d29ya1RpdGxlPuacquWRveWQjeS9nOWTgTwvSXB0YzR4bXBFeHQ6QXJ0d29ya1RpdGxlPgogICAgICA8L3JkZjpEZXNjcmlwdGlvbj4KICAgPC9yZGY6UkRGPgo8L3g6eG1wbWV0YT4Kxt05LAAAQABJREFUeAHsfQeA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" alt="OpenIntentOS" style="width:80px;height:80px;border-radius:50%;margin-bottom:.5rem"></div>
  <!-- step dots -->
  <div class="dots">
    <div class="dot active" id="d1"></div>
    <div class="dot" id="d2"></div>
    <div class="dot" id="d3"></div>
  </div>

  <!-- step 1: choose LLM provider -->
  <div class="step active" id="step1">
    <h2>Choose an LLM provider</h2>
    <p class="subtitle">Pick one to get started. You can add more later.</p>
    <div id="ollama-badge" style="display:none" class="ollama-detect">&#10003; Ollama detected &mdash; no API key needed</div>
    <div class="grid" id="provider-grid">
      <button class="provider-btn" onclick="pick('openai')">
        <div class="provider-name">OpenAI (GPT-4)</div>
        <span class="provider-badge badge-paid">Paid</span>
      </button>
      <button class="provider-btn" onclick="pick('google')">
        <div class="provider-name">Google Gemini</div>
        <span class="provider-badge badge-free">Free tier</span>
      </button>
      <button class="provider-btn" onclick="pick('groq')">
        <div class="provider-name">Groq (fast &amp; free)</div>
        <span class="provider-badge badge-free">Free tier</span>
      </button>
      <button class="provider-btn" onclick="pick('deepseek')">
        <div class="provider-name">DeepSeek</div>
        <span class="provider-badge badge-paid">Affordable</span>
      </button>
      <button class="provider-btn" onclick="pick('nvidia')">
        <div class="provider-name">NVIDIA NIM</div>
        <span class="provider-badge badge-free">Free credits</span>
      </button>
      <button class="provider-btn" id="ollama-btn" style="display:none" onclick="pick('ollama')">
        <div class="provider-name">Ollama (local)</div>
        <span class="provider-badge badge-free">Free</span>
      </button>
    </div>
    <div class="actions">
      <button class="btn btn-secondary" onclick="skipProvider()">Skip for now</button>
      <button class="btn btn-primary" id="btn-next1" disabled onclick="goStep(2)">Next &rarr;</button>
    </div>
  </div>

  <!-- step 2: API key -->
  <div class="step" id="step2">
    <h2 id="key-title">Enter your API key</h2>
    <p class="subtitle" id="key-sub">Paste the key for your chosen provider.</p>
    <div class="field">
      <label id="key-label">API Key</label>
      <input type="password" id="key-input" placeholder="sk-..." oninput="validateKey()">
      <div class="hint" id="key-hint"></div>
    </div>
    <div class="actions">
      <button class="btn btn-secondary" onclick="goStep(1)">&larr; Back</button>
      <button class="btn btn-primary" id="btn-next2" disabled onclick="goStep(3)">Next &rarr;</button>
    </div>
  </div>

  <!-- step 3: Telegram -->
  <div class="step" id="step3">
    <h2>Connect Telegram <span style="color:var(--muted);font-size:.9rem;font-weight:400">(optional)</span></h2>
    <p class="subtitle">Get a bot token so OpenIntentOS can respond to messages.</p>
    <ol class="tg-steps">
      <li>Open Telegram and search <strong>@BotFather</strong></li>
      <li>Send the command <code>/newbot</code> and follow the prompts</li>
      <li>Copy the token BotFather gives you and paste it below</li>
    </ol>
    <div class="field">
      <label>Bot Token</label>
      <input type="password" id="tg-input" placeholder="123456789:ABC...">
    </div>
    <div class="actions">
      <button class="btn btn-secondary" onclick="finish()">Skip, finish &rarr;</button>
      <button class="btn btn-primary" onclick="finish()">Finish setup &#10003;</button>
    </div>
  </div>

  <!-- step 4: done -->
  <div class="step" id="step4">
    <div class="done-icon">&#10003;</div>
    <div class="done-title">All set!</div>
    <p class="done-sub">Configuration saved. Starting up&hellip;</p>
    <p class="done-count" id="countdown">Redirecting in 4&hellip;</p>
  </div>
</div>

<script>
(function () {
  var selectedProvider = '';
  var skipKey = false;
  var ollamaAvailable = false;

  // Provider → { label, placeholder, hint }
  var providerMeta = {
    openai:    { label: 'OpenAI API Key', placeholder: 'sk-...', hint: '<a href="https://platform.openai.com/api-keys" target="_blank">Get a key at platform.openai.com</a>' },
    google:    { label: 'Google AI API Key', placeholder: 'AIza...', hint: '<a href="https://aistudio.google.com/app/apikey" target="_blank">Get a key at aistudio.google.com</a>' },
    groq:      { label: 'Groq API Key', placeholder: 'gsk_...', hint: '<a href="https://console.groq.com/keys" target="_blank">Get a key at console.groq.com</a>' },
    deepseek:  { label: 'DeepSeek API Key', placeholder: 'sk-...', hint: '<a href="https://platform.deepseek.com/api_keys" target="_blank">Get a key at platform.deepseek.com</a>' },
    nvidia:    { label: 'NVIDIA NIM API Key', placeholder: 'nvapi-...', hint: '<a href="https://build.nvidia.com" target="_blank">Get free credits at build.nvidia.com</a>' },
    anthropic: { label: 'Anthropic API Key', placeholder: 'sk-ant-...', hint: '<a href="https://console.anthropic.com/settings/keys" target="_blank">Get a key at console.anthropic.com</a>' },
  };

  // Fetch status on load
  fetch('/api/setup/status')
    .then(function (r) { return r.json(); })
    .then(function (s) {
      if (s.ollama) {
        ollamaAvailable = true;
        document.getElementById('ollama-badge').style.display = 'block';
        document.getElementById('ollama-btn').style.display = 'block';
      }
    })
    .catch(function () {});

  window.pick = function (provider) {
    selectedProvider = provider;
    // Highlight selected button
    var btns = document.querySelectorAll('.provider-btn');
    btns.forEach(function (b) { b.classList.remove('selected'); });
    event.currentTarget.classList.add('selected');
    document.getElementById('btn-next1').disabled = false;
  };

  window.skipProvider = function () {
    selectedProvider = '';
    skipKey = true;
    goStep(3);
  };

  window.goStep = function (n) {
    // Hide all
    [1, 2, 3, 4].forEach(function (i) {
      document.getElementById('step' + i).classList.remove('active');
    });

    // Update dots
    var dotMap = { 1: 'd1', 2: 'd2', 3: 'd3' };
    ['d1', 'd2', 'd3'].forEach(function (id, idx) {
      var dot = document.getElementById(id);
      dot.classList.remove('active', 'done');
      if (idx + 1 < n) dot.classList.add('done');
      else if (idx + 1 === n) dot.classList.add('active');
    });

    if (n === 2) {
      // Skip key step for Ollama or skipped
      if (selectedProvider === 'ollama' || skipKey) {
        document.getElementById('step3').classList.add('active');
        document.getElementById('d2').classList.remove('active');
        document.getElementById('d2').classList.add('done');
        document.getElementById('d3').classList.add('active');
        return;
      }
      populateKeyStep();
    }

    document.getElementById('step' + n).classList.add('active');
  };

  function populateKeyStep() {
    var meta = providerMeta[selectedProvider] || { label: 'API Key', placeholder: 'paste key here', hint: '' };
    document.getElementById('key-title').textContent = 'Enter your ' + (selectedProvider.charAt(0).toUpperCase() + selectedProvider.slice(1)) + ' key';
    document.getElementById('key-label').textContent = meta.label;
    document.getElementById('key-input').placeholder = meta.placeholder;
    document.getElementById('key-hint').innerHTML = meta.hint;
    document.getElementById('key-input').value = '';
    document.getElementById('btn-next2').disabled = true;
  }

  window.validateKey = function () {
    var val = document.getElementById('key-input').value;
    document.getElementById('btn-next2').disabled = val.length < 8;
  };

  window.finish = function () {
    var apiKey = (selectedProvider && selectedProvider !== 'ollama')
      ? (document.getElementById('key-input').value || '')
      : '';
    var tgToken = document.getElementById('tg-input').value || '';

    fetch('/api/setup/save', {
      method: 'POST',
      headers: { 'Content-Type': 'application/json' },
      body: JSON.stringify({ provider: selectedProvider, api_key: apiKey, telegram_token: tgToken })
    })
      .then(function (r) { return r.json(); })
      .then(function (res) {
        if (res.ok) {
          showDone();
        } else {
          alert('Error saving configuration: ' + (res.error || 'unknown error'));
        }
      })
      .catch(function (e) {
        alert('Network error: ' + e);
      });
  };

  function showDone() {
    [1, 2, 3, 4].forEach(function (i) {
      document.getElementById('step' + i).classList.remove('active');
    });
    document.getElementById('step4').classList.add('active');
    ['d1', 'd2', 'd3'].forEach(function (id) {
      var dot = document.getElementById(id);
      dot.classList.remove('active');
      dot.classList.add('done');
    });

    var secs = 4;
    var el = document.getElementById('countdown');
    var iv = setInterval(function () {
      secs -= 1;
      if (secs <= 0) {
        clearInterval(iv);
        window.location.href = '/';
      } else {
        el.textContent = 'Redirecting in ' + secs + '\u2026';
      }
    }, 1000);
  }
})();
</script>
</body>
</html>
"##;

// ── embedded onboarding wizard HTML ──────────────────────────────────────────

/// The 4-step onboarding wizard served at `/onboarding`.
pub const ONBOARDING_HTML: &str = r##"<!DOCTYPE html>
<html lang="en">
<head>
<meta charset="utf-8">
<meta name="viewport" content="width=device-width, initial-scale=1">
<title>OpenIntentOS — Onboarding</title>
<link rel="icon" type="image/png" href="data:image/png;base64,iVBORw0KGgoAAAANSUhEUgAAAEAAAABACAYAAACqaXHeAAABY2lDQ1BrQ0dDb2xvclNwYWNlRGlzcGxheVAzAAAokX2QsUvDUBDGv1aloHUQHRwcMolDlJIKuji0FURxCFXB6pS+pqmQxkeSIgU3/4GC/4EKzm4Whzo6OAiik+jm5KTgouV5L4mkInqP435877vjOCA5bnBu9wOoO75bXMorm6UtJfWMBL0gDObxnK6vSv6uP+P9PvTeTstZv///jcGK6TGqn5QZxl0fSKjE+p7PJe8Tj7m0FHFLshXyieRyyOeBZ71YIL4mVljNqBC/EKvlHt3q4brdYNEOcvu06WysyTmUE1jEDjxw2DDQhAId2T/8s4G/gF1yN+FSn4UafOrJkSInmMTLcMAwA5VYQ4ZSk3eO7ncX3U+NtYMnYKEjhLiItZUOcDZHJ2vH2tQ8MDIEXLW54RqB1EeZrFaB11NguASM3lDPtlfNauH26Tww8CjE2ySQOgS6LSE+joToHlPzA3DpfAEDp2ITpJYOWwAAAARjSUNQDA0AAW4D4+8AAACUZVhJZk1NACoAAAAIAAYBBgADAAAAAQACAAABDQACAAAAEAAAAFYBGgAFAAAAAQAAAGYBGwAFAAAAAQAAAG4BKAADAAAAAQACAACHaQAEAAAAAQAAAHYAAAAA5pyq5ZG95ZCN5L2c5ZOBAAAAASwAAAABAAABLAAAAAEAAqACAAQAAAABAAAAQKADAAQAAAABAAAAQAAAAABhe/ibAAAACXBIWXMAAC4jAAAuIwF4pT92AAAEmWlUWHRYTUw6Y29tLmFkb2JlLnhtcAAAAAAAPHg6eG1wbWV0YSB4bWxuczp4PSJhZG9iZTpuczptZXRhLyIgeDp4bXB0az0iWE1QIENvcmUgNi4wLjAiPgogICA8cmRmOlJERiB4bWxuczpyZGY9Imh0dHA6Ly93d3cudzMub3JnLzE5OTkvMDIvMjItcmRmLXN5bnRheC1ucyMiPgogICAgICA8cmRmOkRlc2NyaXB0aW9uIHJkZjphYm91dD0iIgogICAgICAgICAgICB4bWxuczp0aWZmPSJodHRwOi8vbnMuYWRvYmUuY29tL3RpZmYvMS4wLyIKICAgICAgICAgICAgeG1sbnM6ZXhpZj0iaHR0cDovL25zLmFkb2JlLmNvbS9leGlmLzEuMC8iCiAgICAgICAgICAgIHhtbG5zOmRjPSJodHRwOi8vcHVybC5vcmcvZGMvZWxlbWVudHMvMS4xLyIKICAgICAgICAgICAgeG1sbnM6SXB0YzR4bXBFeHQ9Imh0dHA6Ly9pcHRjLm9yZy9zdGQvSXB0YzR4bXBFeHQvMjAwOC0wMi0yOS8iPgogICAgICAgICA8dGlmZjpEb2N1bWVudE5hbWU+5pyq5ZG95ZCN5L2c5ZOBPC90aWZmOkRvY3VtZW50TmFtZT4KICAgICAgICAgPHRpZmY6UmVzb2x1dGlvblVuaXQ+MjwvdGlmZjpSZXNvbHV0aW9uVW5pdD4KICAgICAgICAgPHRpZmY6Q29tcHJlc3Npb24+NTwvdGlmZjpDb21wcmVzc2lvbj4KICAgICAgICAgPHRpZmY6WFJlc29sdXRpb24+MzAwPC90aWZmOlhSZXNvbHV0aW9uPgogICAgICAgICA8dGlmZjpZUmVzb2x1dGlvbj4zMDA8L3RpZmY6WVJlc29sdXRpb24+CiAgICAgICAgIDx0aWZmOlBob3RvbWV0cmljSW50ZXJwcmV0YXRpb24+MjwvdGlmZjpQaG90b21ldHJpY0ludGVycHJldGF0aW9uPgogICAgICAgICA8ZXhpZjpQaXhlbFhEaW1lbnNpb24+MjUwMDwvZXhpZjpQaXhlbFhEaW1lbnNpb24+CiAgICAgICAgIDxleGlmOlBpeGVsWURpbWVuc2lvbj4yNTAwPC9leGlmOlBpeGVsWURpbWVuc2lvbj4KICAgICAgICAgPGRjOnRpdGxlPgogICAgICAgICAgICA8cmRmOkFsdD4KICAgICAgICAgICAgICAgPHJkZjpsaSB4bWw6bGFuZz0ieC1kZWZhdWx0Ij7mnKrlkb3lkI3kvZzlk4E8L3JkZjpsaT4KICAgICAgICAgICAgPC9yZGY6QWx0PgogICAgICAgICA8L2RjOnRpdGxlPgogICAgICAgICA8SXB0YzR4bXBFeHQ6QXJ0d29ya1RpdGxlPuacquWRveWQjeS9nOWTgTwvSXB0YzR4bXBFeHQ6QXJ0d29ya1RpdGxlPgogICAgICA8L3JkZjpEZXNjcmlwdGlvbj4KICAgPC9yZGY6UkRGPgo8L3g6eG1wbWV0YT4Kxt05LAAAGXxJREFUeAHFWwlw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">
<style>
*,*::before,*::after{box-sizing:border-box;margin:0;padding:0}
:root{
  --bg:#1a1a2e;
  --bg2:#16213e;
  --bg3:#12192e;
  --accent:#e94560;
  --green:#4ecca3;
  --text:#e4e4e4;
  --muted:#8a8a9a;
  --border:#2a2a4a;
}
body{background:var(--bg);color:var(--text);font-family:-apple-system,BlinkMacSystemFont,'Segoe UI',sans-serif;min-height:100vh;display:flex;align-items:center;justify-content:center;padding:1rem}
.card{background:var(--bg2);border:1px solid var(--border);border-radius:12px;padding:2rem;width:100%;max-width:500px;box-shadow:0 8px 40px rgba(0,0,0,.4)}
.dots{display:flex;justify-content:center;gap:.6rem;margin-bottom:2rem}
.dot{width:10px;height:10px;border-radius:50%;background:var(--border);transition:background .25s}
.dot.active{background:var(--accent)}
.dot.done{background:var(--green)}
h2{font-size:1.4rem;font-weight:700;margin-bottom:.4rem}
.subtitle{color:var(--muted);font-size:.9rem;margin-bottom:1.5rem}
.use-case-grid{display:grid;grid-template-columns:1fr 1fr;gap:.6rem;margin-bottom:1.5rem}
.uc-btn{background:var(--bg3);border:2px solid var(--border);border-radius:8px;padding:.9rem .75rem;cursor:pointer;text-align:left;color:var(--text);transition:border-color .2s,background .2s}
.uc-btn:hover{border-color:var(--accent);background:var(--bg)}
.uc-btn.selected{border-color:var(--accent);background:rgba(233,69,96,.1)}
.uc-icon{font-size:1.4rem;margin-bottom:.3rem}
.uc-name{font-weight:600;font-size:.9rem}
.uc-desc{font-size:.78rem;color:var(--muted);margin-top:.2rem}
.plugin-list{list-style:none;margin-bottom:1.5rem}
.plugin-list li{display:flex;align-items:center;gap:.75rem;padding:.55rem 0;border-bottom:1px solid var(--border);font-size:.9rem}
.plugin-list li:last-child{border-bottom:none}
.plugin-icon{font-size:1.1rem;min-width:1.5rem;text-align:center}
.toggle{position:relative;display:inline-block;width:44px;height:24px;margin-left:auto;flex-shrink:0}
.toggle input{opacity:0;width:0;height:0}
.slider{position:absolute;cursor:pointer;inset:0;background:var(--border);border-radius:12px;transition:.3s}
.slider::before{content:'';position:absolute;height:18px;width:18px;left:3px;bottom:3px;background:var(--muted);border-radius:50%;transition:.3s}
input:checked+.slider{background:var(--accent)}
input:checked+.slider::before{transform:translateX(20px);background:#fff}
.briefing-box{background:var(--bg3);border:1px solid var(--border);border-radius:8px;padding:1rem;margin-bottom:1.5rem;display:flex;align-items:center;gap:1rem}
.briefing-text{flex:1}
.briefing-title{font-weight:600;font-size:.95rem}
.briefing-sub{font-size:.8rem;color:var(--muted);margin-top:.2rem}
.field{margin-bottom:1.25rem}
label{display:block;font-size:.85rem;color:var(--muted);margin-bottom:.4rem}
input[type=password],input[type=text]{width:100%;background:var(--bg3);border:1px solid var(--border);border-radius:6px;padding:.65rem .75rem;color:var(--text);font-size:.95rem;outline:none}
input:focus{border-color:var(--accent)}
.hint{font-size:.78rem;color:var(--muted);margin-top:.35rem}
.actions{display:flex;gap:.75rem;justify-content:flex-end;margin-top:.25rem}
.btn{padding:.6rem 1.2rem;border-radius:6px;border:none;cursor:pointer;font-size:.9rem;font-weight:600;transition:opacity .2s}
.btn-primary{background:var(--accent);color:#fff}
.btn-primary:disabled{opacity:.4;cursor:not-allowed}
.btn-secondary{background:transparent;color:var(--muted);border:1px solid var(--border)}
.btn-secondary:hover{color:var(--text);border-color:var(--text)}
.done-icon{font-size:3.5rem;text-align:center;margin-bottom:.75rem}
.done-title{color:var(--green);font-size:1.5rem;font-weight:700;text-align:center;margin-bottom:.4rem}
.done-sub{color:var(--muted);text-align:center;font-size:.9rem;margin-bottom:.5rem}
.done-count{color:var(--muted);text-align:center;font-size:.85rem}
.step{display:none}
.step.active{display:block}
</style>
</head>
<body>
<div class="card">
  <!-- logo -->
  <div style="text-align:center;margin-bottom:1rem"><img src="data:image/png;base64,iVBORw0KGgoAAAANSUhEUgAAAQAAAAEACAYAAABccqhmAAABY2lDQ1BrQ0dDb2xvclNwYWNlRGlzcGxheVAzAAAokX2QsUvDUBDGv1aloHUQHRwcMolDlJIKuji0FURxCFXB6pS+pqmQxkeSIgU3/4GC/4EKzm4Whzo6OAiik+jm5KTgouV5L4mkInqP435877vjOCA5bnBu9wOoO75bXMorm6UtJfWMBL0gDObxnK6vSv6uP+P9PvTeTstZv///jcGK6TGqn5QZxl0fSKjE+p7PJe8Tj7m0FHFLshXyieRyyOeBZ71YIL4mVljNqBC/EKvlHt3q4brdYNEOcvu06WysyTmUE1jEDjxw2DDQhAId2T/8s4G/gF1yN+FSn4UafOrJkSInmMTLcMAwA5VYQ4ZSk3eO7ncX3U+NtYMnYKEjhLiItZUOcDZHJ2vH2tQ8MDIEXLW54RqB1EeZrFaB11NguASM3lDPtlfNauH26Tww8CjE2ySQOgS6LSE+joToHlPzA3DpfAEDp2ITpJYOWwAAAARjSUNQDA0AAW4D4+8AAACUZVhJZk1NACoAAAAIAAYBBgADAAAAAQACAAABDQACAAAAEAAAAFYBGgAFAAAAAQAAAGYBGwAFAAAAAQAAAG4BKAADAAAAAQACAACHaQAEAAAAAQAAAHYAAAAA5pyq5ZG95ZCN5L2c5ZOBAAAAASwAAAABAAABLAAAAAEAAqACAAQAAAABAAABAKADAAQAAAABAAABAAAAAABAwKBiAAAACXBIWXMAAC4jAAAuIwF4pT92AAAEmWlUWHRYTUw6Y29tLmFkb2JlLnhtcAAAAAAAPHg6eG1wbWV0YSB4bWxuczp4PSJhZG9iZTpuczptZXRhLyIgeDp4bXB0az0iWE1QIENvcmUgNi4wLjAiPgogICA8cmRmOlJERiB4bWxuczpyZGY9Imh0dHA6Ly93d3cudzMub3JnLzE5OTkvMDIvMjItcmRmLXN5bnRheC1ucyMiPgogICAgICA8cmRmOkRlc2NyaXB0aW9uIHJkZjphYm91dD0iIgogICAgICAgICAgICB4bWxuczp0aWZmPSJodHRwOi8vbnMuYWRvYmUuY29tL3RpZmYvMS4wLyIKICAgICAgICAgICAgeG1sbnM6ZXhpZj0iaHR0cDovL25zLmFkb2JlLmNvbS9leGlmLzEuMC8iCiAgICAgICAgICAgIHhtbG5zOmRjPSJodHRwOi8vcHVybC5vcmcvZGMvZWxlbWVudHMvMS4xLyIKICAgICAgICAgICAgeG1sbnM6SXB0YzR4bXBFeHQ9Imh0dHA6Ly9pcHRjLm9yZy9zdGQvSXB0YzR4bXBFeHQvMjAwOC0wMi0yOS8iPgogICAgICAgICA8dGlmZjpEb2N1bWVudE5hbWU+5pyq5ZG95ZCN5L2c5ZOBPC90aWZmOkRvY3VtZW50TmFtZT4KICAgICAgICAgPHRpZmY6UmVzb2x1dGlvblVuaXQ+MjwvdGlmZjpSZXNvbHV0aW9uVW5pdD4KICAgICAgICAgPHRpZmY6Q29tcHJlc3Npb24+NTwvdGlmZjpDb21wcmVzc2lvbj4KICAgICAgICAgPHRpZmY6WFJlc29sdXRpb24+MzAwPC90aWZmOlhSZXNvbHV0aW9uPgogICAgICAgICA8dGlmZjpZUmVzb2x1dGlvbj4zMDA8L3RpZmY6WVJlc29sdXRpb24+CiAgICAgICAgIDx0aWZmOlBob3RvbWV0cmljSW50ZXJwcmV0YXRpb24+MjwvdGlmZjpQaG90b21ldHJpY0ludGVycHJldGF0aW9uPgogICAgICAgICA8ZXhpZjpQaXhlbFhEaW1lbnNpb24+MjUwMDwvZXhpZjpQaXhlbFhEaW1lbnNpb24+CiAgICAgICAgIDxleGlmOlBpeGVsWURpbWVuc2lvbj4yNTAwPC9leGlmOlBpeGVsWURpbWVuc2lvbj4KICAgICAgICAgPGRjOnRpdGxlPgogICAgICAgICAgICA8cmRmOkFsdD4KICAgICAgICAgICAgICAgPHJkZjpsaSB4bWw6bGFuZz0ieC1kZWZhdWx0Ij7mnKrlkb3lkI3kvZzlk4E8L3JkZjpsaT4KICAgICAgICAgICAgPC9yZGY6QWx0PgogICAgICAgICA8L2RjOnRpdGxlPgogICAgICAgICA8SXB0YzR4bXBFeHQ6QXJ0d29ya1RpdGxlPuacquWRveWQjeS9nOWTgTwvSXB0YzR4bXBFeHQ6QXJ0d29ya1RpdGxlPgogICAgICA8L3JkZjpEZXNjcmlwdGlvbj4KICAgPC9yZGY6UkRGPgo8L3g6eG1wbWV0YT4Kxt05LAAAQABJREFUeAHsfQeA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" alt="OpenIntentOS" style="width:80px;height:80px;border-radius:50%;margin-bottom:.5rem"></div>
  <div class="dots">
    <div class="dot active" id="d1"></div>
    <div class="dot" id="d2"></div>
    <div class="dot" id="d3"></div>
    <div class="dot" id="d4"></div>
  </div>

  <!-- Step 1: Use case -->
  <div class="step active" id="step1">
    <h2>Welcome to OpenIntentOS</h2>
    <p class="subtitle">Let&rsquo;s personalise your workspace. What&rsquo;s your primary use case?</p>
    <div class="use-case-grid">
      <button class="uc-btn" onclick="pickUseCase('developer',this)">
        <div class="uc-icon">&#128187;</div>
        <div class="uc-name">Developer workflow</div>
        <div class="uc-desc">Code, git, CI/CD, PRs</div>
      </button>
      <button class="uc-btn" onclick="pickUseCase('business',this)">
        <div class="uc-icon">&#128188;</div>
        <div class="uc-name">Business productivity</div>
        <div class="uc-desc">Email, calendar, docs</div>
      </button>
      <button class="uc-btn" onclick="pickUseCase('personal',this)">
        <div class="uc-icon">&#127968;</div>
        <div class="uc-name">Personal automation</div>
        <div class="uc-desc">Tasks, reminders, habits</div>
      </button>
      <button class="uc-btn" onclick="pickUseCase('research',this)">
        <div class="uc-icon">&#128270;</div>
        <div class="uc-name">Research &amp; analysis</div>
        <div class="uc-desc">Web search, summarise, report</div>
      </button>
    </div>
    <div class="actions">
      <button class="btn btn-primary" id="btn-uc-next" disabled onclick="goStep(2)">Next &rarr;</button>
    </div>
  </div>

  <!-- Step 2: Plugins -->
  <div class="step" id="step2">
    <h2>Recommended plugins</h2>
    <p class="subtitle">Toggle the plugins you want enabled. You can change these later.</p>
    <ul class="plugin-list" id="plugin-list"></ul>
    <div class="actions">
      <button class="btn btn-secondary" onclick="goStep(1)">&larr; Back</button>
      <button class="btn btn-primary" onclick="goStep(3)">Next &rarr;</button>
    </div>
  </div>

  <!-- Step 3: Morning briefing + Telegram -->
  <div class="step" id="step3">
    <h2>Daily briefing &amp; notifications</h2>
    <p class="subtitle">Configure how OpenIntentOS keeps you informed.</p>
    <div class="briefing-box">
      <div class="briefing-text">
        <div class="briefing-title">Morning briefing at 7 am</div>
        <div class="briefing-sub">Summarises tasks, emails, calendar and news every morning.</div>
      </div>
      <label class="toggle">
        <input type="checkbox" id="briefing-toggle" checked>
        <span class="slider"></span>
      </label>
    </div>
    <div class="field">
      <label>Telegram bot token <span style="color:var(--muted)">(optional)</span></label>
      <input type="password" id="tg-token" placeholder="123456789:ABC...">
      <div class="hint">Control OpenIntentOS via Telegram. Leave blank to skip.</div>
    </div>
    <div class="actions">
      <button class="btn btn-secondary" onclick="goStep(2)">&larr; Back</button>
      <button class="btn btn-primary" onclick="finish()">Finish setup &#10003;</button>
    </div>
  </div>

  <!-- Step 4: Done -->
  <div class="step" id="step4">
    <div class="done-icon">&#10003;</div>
    <div class="done-title">You&rsquo;re all set!</div>
    <p class="done-sub">Your workspace is ready. Starting up&hellip;</p>
    <p class="done-count" id="countdown">Redirecting in 4&hellip;</p>
  </div>
</div>

<script>
(function () {
  var selectedUseCase = '';
  var pluginStates = {};

  var pluginsByUseCase = {
    developer: [
      { id: 'git-helper',     icon: '&#128187;', name: 'Git Helper',      desc: 'Commit, PR, and branch assistance' },
      { id: 'code-review',    icon: '&#128269;', name: 'Code Review',     desc: 'AI-powered code review' },
      { id: 'daily-briefing', icon: '&#9728;',   name: 'Daily Briefing',  desc: 'Morning summary of tasks and PRs' },
    ],
    business: [
      { id: 'email-manager',  icon: '&#128140;', name: 'Email Manager',   desc: 'Smart email triage and drafts' },
      { id: 'calendar-sync',  icon: '&#128197;', name: 'Calendar Sync',   desc: 'Manage your schedule with AI' },
      { id: 'daily-briefing', icon: '&#9728;',   name: 'Daily Briefing',  desc: 'Morning summary of your day' },
    ],
    personal: [
      { id: 'daily-briefing', icon: '&#9728;',   name: 'Daily Briefing',  desc: 'Morning summary and reminders' },
      { id: 'task-tracker',   icon: '&#9989;',   name: 'Task Tracker',    desc: 'Manage personal to-dos' },
    ],
    research: [
      { id: 'web-research',   icon: '&#127760;', name: 'Web Research',    desc: 'Deep web search and synthesis' },
      { id: 'summarizer',     icon: '&#128221;', name: 'Summarizer',      desc: 'Condense long documents' },
      { id: 'daily-briefing', icon: '&#9728;',   name: 'Daily Briefing',  desc: 'Morning news and topic digest' },
    ],
  };

  window.pickUseCase = function (uc, btn) {
    selectedUseCase = uc;
    document.querySelectorAll('.uc-btn').forEach(function (b) { b.classList.remove('selected'); });
    btn.classList.add('selected');
    document.getElementById('btn-uc-next').disabled = false;
  };

  window.goStep = function (n) {
    [1, 2, 3, 4].forEach(function (i) {
      document.getElementById('step' + i).classList.remove('active');
    });
    ['d1','d2','d3','d4'].forEach(function (id, idx) {
      var dot = document.getElementById(id);
      dot.classList.remove('active','done');
      if (idx + 1 < n) dot.classList.add('done');
      else if (idx + 1 === n) dot.classList.add('active');
    });

    if (n === 2) buildPluginList();
    document.getElementById('step' + n).classList.add('active');
  };

  function buildPluginList() {
    var plugins = pluginsByUseCase[selectedUseCase] || [];
    var list = document.getElementById('plugin-list');
    list.innerHTML = '';
    plugins.forEach(function (p) {
      if (!(p.id in pluginStates)) pluginStates[p.id] = true;
      var li = document.createElement('li');
      li.innerHTML =
        '<span class="plugin-icon">' + p.icon + '</span>' +
        '<span><strong>' + p.name + '</strong><br><span style="color:var(--muted);font-size:.8rem">' + p.desc + '</span></span>' +
        '<label class="toggle">' +
        '<input type="checkbox" id="plug-' + p.id + '"' + (pluginStates[p.id] ? ' checked' : '') + '>' +
        '<span class="slider"></span>' +
        '</label>';
      li.querySelector('input').addEventListener('change', function (e) {
        pluginStates[p.id] = e.target.checked;
      });
      list.appendChild(li);
    });
  }

  window.finish = function () {
    var briefingEnabled = document.getElementById('briefing-toggle').checked;
    var tgToken = document.getElementById('tg-token').value || '';

    fetch('/api/onboarding/save', {
      method: 'POST',
      headers: { 'Content-Type': 'application/json' },
      body: JSON.stringify({
        use_case: selectedUseCase,
        briefing_enabled: briefingEnabled,
        telegram_token: tgToken,
      }),
    })
      .then(function (r) { return r.json(); })
      .then(function (res) {
        if (res.ok) {
          showDone();
        } else {
          alert('Error saving configuration: ' + (res.error || 'unknown error'));
        }
      })
      .catch(function (e) {
        alert('Network error: ' + e);
      });
  };

  function showDone() {
    [1,2,3,4].forEach(function (i) {
      document.getElementById('step' + i).classList.remove('active');
    });
    document.getElementById('step4').classList.add('active');
    ['d1','d2','d3','d4'].forEach(function (id) {
      var dot = document.getElementById(id);
      dot.classList.remove('active');
      dot.classList.add('done');
    });

    var secs = 4;
    var el = document.getElementById('countdown');
    var iv = setInterval(function () {
      secs -= 1;
      if (secs <= 0) {
        clearInterval(iv);
        window.location.href = 'http://localhost:23517';
      } else {
        el.textContent = 'Redirecting in ' + secs + '\u2026';
      }
    }, 1000);
  }
})();
</script>
</body>
</html>
"##;
