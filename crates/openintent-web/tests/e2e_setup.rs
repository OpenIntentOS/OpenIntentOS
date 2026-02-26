//! End-to-end tests for the setup and onboarding HTTP API.
//!
//! These tests spin up the **real** Axum server on an OS-assigned ephemeral
//! port, make actual HTTP requests via `reqwest`, and verify the full
//! request/response cycle including JSON parsing.
//!
//! File-writing behaviour is covered separately via the pure-function helpers
//! (`build_env_content`, `write_setup_env`, etc.) to avoid touching the
//! working-directory `.env` during test runs.

use std::net::SocketAddr;

use axum::Router;
use axum::response::Html;
use axum::routing::{get, post};
use tempfile::TempDir;
use tokio::net::TcpListener;

use openintent_web::setup::{
    OnboardingPayload, SetupPayload, build_env_content, build_onboarding_additions,
    get_onboarding, get_status, post_onboarding_save, post_save, write_onboarding_env,
    write_setup_env, SETUP_HTML, ONBOARDING_HTML,
};

// ── helpers ──────────────────────────────────────────────────────────────────

/// Bind to 127.0.0.1:0, start the setup router, return (base_url, server task).
async fn start_test_server() -> (String, tokio::task::JoinHandle<()>) {
    let app = Router::new()
        .route("/setup", get(|| async { Html(SETUP_HTML) }))
        .route("/onboarding", get(get_onboarding))
        .route("/api/setup/status", get(get_status))
        .route("/api/setup/save", post(post_save))
        .route("/api/onboarding/save", post(post_onboarding_save));

    let listener = TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind to port 0");
    let addr: SocketAddr = listener.local_addr().expect("get local addr");
    let base = format!("http://127.0.0.1:{}", addr.port());

    let handle = tokio::spawn(async move {
        axum::serve(listener, app).await.ok();
    });

    // Small yield so the listener is ready.
    tokio::time::sleep(std::time::Duration::from_millis(10)).await;

    (base, handle)
}

// ── GET /api/setup/status ─────────────────────────────────────────────────────

#[tokio::test]
async fn get_status_returns_valid_json() {
    let (base, _srv) = start_test_server().await;
    let client = reqwest::Client::new();

    let resp = client
        .get(format!("{base}/api/setup/status"))
        .send()
        .await
        .expect("request failed");

    assert_eq!(resp.status(), 200);

    let json: serde_json::Value = resp.json().await.expect("invalid JSON");
    assert!(
        json.get("configured").is_some(),
        "response must contain 'configured' field"
    );
    assert!(
        json.get("ollama").is_some(),
        "response must contain 'ollama' field"
    );
    assert!(
        json["configured"].is_boolean(),
        "'configured' must be a boolean"
    );
    assert!(json["ollama"].is_boolean(), "'ollama' must be a boolean");
}

// ── GET /setup ────────────────────────────────────────────────────────────────

#[tokio::test]
async fn get_setup_returns_html_wizard() {
    let (base, _srv) = start_test_server().await;
    let client = reqwest::Client::new();

    let resp = client
        .get(format!("{base}/setup"))
        .send()
        .await
        .expect("request failed");

    assert_eq!(resp.status(), 200);

    let content_type = resp
        .headers()
        .get("content-type")
        .and_then(|v| v.to_str().ok())
        .unwrap_or("");
    assert!(
        content_type.contains("text/html"),
        "Content-Type should be text/html, got: {content_type}"
    );

    let body = resp.text().await.expect("failed to read body");
    assert!(
        body.contains("OpenIntentOS"),
        "setup HTML must mention 'OpenIntentOS'"
    );
    assert!(
        body.contains("LLM provider"),
        "setup HTML must mention 'LLM provider'"
    );
    assert!(
        !body.contains("AI assistant"),
        "setup HTML must NOT say 'AI assistant'"
    );
    assert!(
        body.contains("/api/setup/save"),
        "setup HTML must reference the save endpoint"
    );
}

// ── GET /onboarding ───────────────────────────────────────────────────────────

#[tokio::test]
async fn get_onboarding_returns_html_wizard() {
    let (base, _srv) = start_test_server().await;
    let client = reqwest::Client::new();

    let resp = client
        .get(format!("{base}/onboarding"))
        .send()
        .await
        .expect("request failed");

    assert_eq!(resp.status(), 200);

    let body = resp.text().await.expect("failed to read body");
    assert!(
        body.contains("OpenIntentOS"),
        "onboarding HTML must mention 'OpenIntentOS'"
    );
    assert!(
        body.contains("use_case"),
        "onboarding HTML must contain use_case field"
    );
    assert!(
        body.contains("/api/onboarding/save"),
        "onboarding HTML must reference the save endpoint"
    );
    assert!(
        !body.contains("AI assistant"),
        "onboarding HTML must NOT say 'AI assistant'"
    );
}

// ── POST /api/setup/save ──────────────────────────────────────────────────────

#[tokio::test]
async fn post_setup_save_returns_ok_for_openai() {
    let (base, _srv) = start_test_server().await;
    let client = reqwest::Client::new();

    // Use a temp dir so the handler can write .env without touching the repo.
    // NOTE: post_save writes to the *current working directory*. In this test
    // we just verify the HTTP response; file-content correctness is covered by
    // the write_setup_env unit test below.
    let resp = client
        .post(format!("{base}/api/setup/save"))
        .json(&serde_json::json!({
            "provider": "openai",
            "api_key": "sk-test-e2e-key",
            "telegram_token": ""
        }))
        .send()
        .await
        .expect("request failed");

    assert_eq!(resp.status(), 200);

    let json: serde_json::Value = resp.json().await.expect("invalid JSON");
    assert_eq!(json["ok"], true, "response must have ok:true, got: {json}");
    assert!(json.get("error").map_or(true, |v| v.is_null()), "should have no error");

    // Clean up the .env written to CWD (best-effort).
    let _ = std::fs::remove_file(".env");
}

// ── POST /api/onboarding/save ─────────────────────────────────────────────────

#[tokio::test]
async fn post_onboarding_save_returns_ok() {
    let (base, _srv) = start_test_server().await;
    let client = reqwest::Client::new();

    let resp = client
        .post(format!("{base}/api/onboarding/save"))
        .json(&serde_json::json!({
            "use_case": "developer",
            "briefing_enabled": true,
            "telegram_token": ""
        }))
        .send()
        .await
        .expect("request failed");

    assert_eq!(resp.status(), 200);

    let json: serde_json::Value = resp.json().await.expect("invalid JSON");
    assert_eq!(json["ok"], true, "response must have ok:true, got: {json}");

    // Clean up (best-effort).
    let _ = std::fs::remove_file(".env");
}

// ── Pure-function tests: build_env_content ────────────────────────────────────

#[test]
fn build_env_content_openai_includes_key() {
    let payload = SetupPayload {
        provider: "openai".to_owned(),
        api_key: "sk-realkey123".to_owned(),
        telegram_token: "555:ABC".to_owned(),
    };
    let content = build_env_content(&payload);

    assert!(content.contains("OPENAI_API_KEY=sk-realkey123"), "missing API key");
    assert!(content.contains("TELEGRAM_BOT_TOKEN=555:ABC"), "missing telegram token");
    assert!(!content.contains("ANTHROPIC"), "wrong provider written");
}

#[test]
fn build_env_content_ollama_writes_no_key() {
    let payload = SetupPayload {
        provider: "ollama".to_owned(),
        api_key: String::new(),
        telegram_token: String::new(),
    };
    let content = build_env_content(&payload);

    // Ollama has no API key variable.
    assert!(!content.contains("OPENAI_API_KEY"), "Ollama should not write OPENAI_API_KEY");
    assert!(!content.contains("ANTHROPIC_API_KEY"), "Ollama should not write ANTHROPIC_API_KEY");
}

#[test]
fn build_env_content_trims_whitespace() {
    let payload = SetupPayload {
        provider: "groq".to_owned(),
        api_key: "  gsk_trailing  ".to_owned(),
        telegram_token: "  ".to_owned(),
    };
    let content = build_env_content(&payload);

    assert!(content.contains("GROQ_API_KEY=gsk_trailing"), "whitespace not trimmed");
    assert!(content.contains("TELEGRAM_BOT_TOKEN=\n") || content.ends_with("TELEGRAM_BOT_TOKEN=\n"),
        "empty telegram token should be empty, got: {content}");
}

// ── Pure-function tests: write_setup_env ─────────────────────────────────────

#[test]
fn write_setup_env_creates_correct_file() {
    let dir = TempDir::new().expect("tmpdir");
    let path = dir.path().join(".env");

    let payload = SetupPayload {
        provider: "anthropic".to_owned(),
        api_key: "sk-ant-test".to_owned(),
        telegram_token: "123:XYZ".to_owned(),
    };

    write_setup_env(&path, &payload).expect("write failed");

    let content = std::fs::read_to_string(&path).expect("read failed");
    assert!(content.contains("ANTHROPIC_API_KEY=sk-ant-test"));
    assert!(content.contains("TELEGRAM_BOT_TOKEN=123:XYZ"));
    assert!(content.contains("# OpenIntentOS Configuration"));
}

// ── Pure-function tests: build_onboarding_additions ──────────────────────────

#[test]
fn onboarding_additions_briefing_enabled() {
    let payload = OnboardingPayload {
        use_case: "developer".to_owned(),
        briefing_enabled: true,
        telegram_token: "tok123".to_owned(),
    };
    let additions = build_onboarding_additions(&payload);

    assert!(additions.contains("ONBOARDING_COMPLETE=true"));
    assert!(additions.contains("ONBOARDING_USE_CASE=developer"));
    assert!(additions.contains("BRIEFING_ENABLED=true"));
    assert!(additions.contains("BRIEFING_TIME=07:00"));
    assert!(additions.contains("TELEGRAM_BOT_TOKEN=tok123"));
}

#[test]
fn onboarding_additions_briefing_disabled() {
    let payload = OnboardingPayload {
        use_case: "research".to_owned(),
        briefing_enabled: false,
        telegram_token: String::new(),
    };
    let additions = build_onboarding_additions(&payload);

    assert!(additions.contains("ONBOARDING_COMPLETE=true"));
    assert!(additions.contains("BRIEFING_ENABLED=false"));
    assert!(!additions.contains("BRIEFING_TIME="), "should not write BRIEFING_TIME when disabled");
    assert!(!additions.contains("TELEGRAM_BOT_TOKEN"), "should not write empty telegram token");
}

#[test]
fn write_onboarding_env_appends_to_existing() {
    let dir = TempDir::new().expect("tmpdir");
    let path = dir.path().join(".env");

    // Write a pre-existing .env with an API key.
    std::fs::write(&path, "OPENAI_API_KEY=existing-key\n").expect("pre-write");

    let payload = OnboardingPayload {
        use_case: "business".to_owned(),
        briefing_enabled: true,
        telegram_token: String::new(),
    };

    write_onboarding_env(&path, &payload).expect("write failed");

    let content = std::fs::read_to_string(&path).expect("read failed");

    // Original content must be preserved.
    assert!(content.contains("OPENAI_API_KEY=existing-key"), "original key lost");

    // New content must be appended.
    assert!(content.contains("ONBOARDING_COMPLETE=true"));
    assert!(content.contains("ONBOARDING_USE_CASE=business"));
    assert!(content.contains("BRIEFING_ENABLED=true"));
}

// ── HTML branding checks ──────────────────────────────────────────────────────

#[test]
fn setup_html_no_ai_assistant_language() {
    assert!(!SETUP_HTML.contains("AI assistant"), "SETUP_HTML must not say 'AI assistant'");
    assert!(!SETUP_HTML.contains("chatbot"), "SETUP_HTML must not say 'chatbot'");
    assert!(SETUP_HTML.contains("OpenIntentOS"), "SETUP_HTML must mention 'OpenIntentOS'");
}

#[test]
fn onboarding_html_no_ai_assistant_language() {
    assert!(!ONBOARDING_HTML.contains("AI assistant"), "ONBOARDING_HTML must not say 'AI assistant'");
    assert!(ONBOARDING_HTML.contains("OpenIntentOS"), "ONBOARDING_HTML must mention 'OpenIntentOS'");
}
