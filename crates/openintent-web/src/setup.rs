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
fn build_env_content(payload: &SetupPayload) -> String {
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

/// Save configuration to `.env` and schedule a process restart.
pub async fn post_save(Json(payload): Json<SetupPayload>) -> Json<SetupResult> {
    let content = build_env_content(&payload);

    match std::fs::write(Path::new(".env"), &content) {
        Ok(()) => {
            info!(
                provider = %payload.provider,
                "setup wizard saved .env, scheduling restart"
            );

            // Give the HTTP response time to reach the browser before exiting.
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

// ── standalone setup server ──────────────────────────────────────────────────

/// Start a minimal HTTP server that only serves the setup wizard.
///
/// Binds to `{bind}:{port}`, serves the wizard at `/setup`, and provides the
/// two setup API endpoints.  Once the user saves their configuration the
/// server will exit after a short delay so the process manager can restart it
/// with the newly written `.env` file.
///
/// # Errors
///
/// Returns an error if the TCP listener cannot be bound.
pub async fn serve_setup(
    bind: &str,
    port: u16,
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let app = Router::new()
        .route("/", get(|| async { Redirect::to("/setup") }))
        .route("/setup", get(|| async { Html(SETUP_HTML) }))
        .route("/api/setup/status", get(get_status))
        .route("/api/setup/save", post(post_save));

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
  <!-- step dots -->
  <div class="dots">
    <div class="dot active" id="d1"></div>
    <div class="dot" id="d2"></div>
    <div class="dot" id="d3"></div>
  </div>

  <!-- step 1: choose AI provider -->
  <div class="step active" id="step1">
    <h2>Choose your AI</h2>
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
