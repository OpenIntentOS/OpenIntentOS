//! ChatGPT Pro setup wizard.
//!
//! Provides a guided web page at `/setup/chatgpt` that walks users through
//! authenticating with their ChatGPT Pro subscription.  Two methods:
//!
//! - **One-click (bookmarklet):** User drags a bookmarklet to their browser
//!   bar, logs in to ChatGPT, clicks the bookmarklet → token sent automatically.
//! - **Manual paste:** User copies session JSON and pastes it into the page.

use std::path::Path;

use axum::Json;
use axum::extract::Query;
use axum::response::Html;
use serde_json::Value;
use tracing::{info, warn};

// ── types ───────────────────────────────────────────────────────────────────

/// Request body for `POST /api/setup/chatgpt`.
#[derive(serde::Deserialize)]
pub struct ChatGptSetupPayload {
    /// Raw JSON string pasted by the user from the session endpoint.
    pub session_json: String,
}

/// Response for `POST /api/setup/chatgpt`.
#[derive(serde::Serialize)]
pub struct ChatGptSetupResult {
    pub ok: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

// ── GET /setup/chatgpt ──────────────────────────────────────────────────────

/// Serve the ChatGPT Pro setup wizard HTML page.
pub async fn get_chatgpt_setup() -> Html<&'static str> {
    Html(CHATGPT_SETUP_HTML)
}

// ── POST /api/setup/chatgpt ─────────────────────────────────────────────────

/// Parse the pasted session JSON, extract the access token, and write to `.env`.
pub async fn post_chatgpt_setup(
    Json(payload): Json<ChatGptSetupPayload>,
) -> Json<ChatGptSetupResult> {
    let trimmed = payload.session_json.trim();

    // Parse JSON.
    let v: Value = match serde_json::from_str(trimmed) {
        Ok(v) => v,
        Err(e) => {
            return Json(ChatGptSetupResult {
                ok: false,
                error: Some(format!("Invalid JSON: {e}")),
            });
        }
    };

    // Extract accessToken.
    let access_token = match v["accessToken"].as_str() {
        Some(t) if !t.is_empty() => t,
        _ => {
            return Json(ChatGptSetupResult {
                ok: false,
                error: Some(
                    "Missing `accessToken` field. Make sure you copied the full page content."
                        .into(),
                ),
            });
        }
    };

    // Write to .env.
    match write_chatgpt_env(Path::new(".env"), access_token) {
        Ok(()) => {
            info!("ChatGPT Pro access token saved to .env");

            // Schedule restart so the new config takes effect.
            #[cfg(not(test))]
            tokio::spawn(async {
                tokio::time::sleep(std::time::Duration::from_secs(2)).await;
                std::process::exit(0);
            });

            Json(ChatGptSetupResult {
                ok: true,
                error: None,
            })
        }
        Err(e) => {
            warn!(error = %e, "failed to write ChatGPT config to .env");
            Json(ChatGptSetupResult {
                ok: false,
                error: Some(format!("Failed to write .env: {e}")),
            })
        }
    }
}

// ── GET /api/setup/chatgpt/callback ────────────────────────────────────────

/// Query parameters for the bookmarklet callback.
#[derive(serde::Deserialize)]
pub struct ChatGptCallbackParams {
    /// The access token extracted by the bookmarklet.
    pub token: String,
}

/// Receive the access token via bookmarklet redirect, save it, and show success.
pub async fn chatgpt_callback(
    Query(params): Query<ChatGptCallbackParams>,
) -> Html<String> {
    let token = params.token.trim();
    if token.is_empty() {
        return Html(CALLBACK_ERROR_HTML.replace("{ERROR}", "Empty token received."));
    }

    match write_chatgpt_env(Path::new(".env"), token) {
        Ok(()) => {
            info!("ChatGPT Pro access token saved via bookmarklet callback");

            #[cfg(not(test))]
            tokio::spawn(async {
                tokio::time::sleep(std::time::Duration::from_secs(2)).await;
                std::process::exit(0);
            });

            Html(CALLBACK_SUCCESS_HTML.to_owned())
        }
        Err(e) => {
            warn!(error = %e, "failed to write ChatGPT config via callback");
            Html(CALLBACK_ERROR_HTML.replace("{ERROR}", &format!("Failed to write .env: {e}")))
        }
    }
}

// ── .env writing ────────────────────────────────────────────────────────────

/// Append or update `CHATGPT_SESSION_TOKEN` in the `.env` file.
///
/// If the file already contains the variable, replace it; otherwise append.
pub fn write_chatgpt_env(path: &Path, access_token: &str) -> std::io::Result<()> {
    let existing = std::fs::read_to_string(path).unwrap_or_default();

    let key = "CHATGPT_SESSION_TOKEN";
    let new_line = format!("{key}={access_token}");

    let updated = if existing.contains(key) {
        // Replace existing line.
        existing
            .lines()
            .map(|line| {
                if line.starts_with(key) {
                    new_line.as_str()
                } else {
                    line
                }
            })
            .collect::<Vec<_>>()
            .join("\n")
            + "\n"
    } else {
        // Append new section.
        let mut content = existing;
        if !content.ends_with('\n') && !content.is_empty() {
            content.push('\n');
        }
        content.push_str("\n# ChatGPT Pro\n");
        content.push_str(&new_line);
        content.push('\n');

        // Also set the provider if not already set.
        if !content.contains("OPENINTENT_PROVIDER") {
            content.push_str("OPENINTENT_PROVIDER=chatgpt-web\n");
        }

        content
    };

    std::fs::write(path, updated)
}

// ── embedded HTML ───────────────────────────────────────────────────────────

/// ChatGPT Pro setup wizard HTML page (bookmarklet + manual paste).
pub const CHATGPT_SETUP_HTML: &str = r##"<!DOCTYPE html>
<html lang="en">
<head>
<meta charset="utf-8">
<meta name="viewport" content="width=device-width, initial-scale=1">
<title>OpenIntentOS — ChatGPT Pro Setup</title>
<style>
*,*::before,*::after{box-sizing:border-box;margin:0;padding:0}
:root{
  --bg:#1a1a2e;--bg2:#16213e;--bg3:#12192e;
  --accent:#e94560;--green:#4ecca3;--blue:#4e9bff;
  --text:#e4e4e4;--muted:#8a8a9a;--border:#2a2a4a;
}
body{background:var(--bg);color:var(--text);font-family:-apple-system,BlinkMacSystemFont,'Segoe UI',sans-serif;min-height:100vh;display:flex;align-items:center;justify-content:center;padding:1rem}
.card{background:var(--bg2);border:1px solid var(--border);border-radius:12px;padding:2rem;width:100%;max-width:560px;box-shadow:0 8px 40px rgba(0,0,0,.4)}
h2{font-size:1.4rem;font-weight:700;margin-bottom:.4rem}
.subtitle{color:var(--muted);font-size:.9rem;margin-bottom:1.5rem}
.tabs{display:flex;gap:0;margin-bottom:1.2rem}
.tab{flex:1;padding:.6rem;text-align:center;font-size:.88rem;font-weight:600;cursor:pointer;border:1px solid var(--border);background:var(--bg3);color:var(--muted);transition:all .2s}
.tab:first-child{border-radius:8px 0 0 8px}
.tab:last-child{border-radius:0 8px 8px 0}
.tab.active{background:var(--accent);color:#fff;border-color:var(--accent)}
.tab-panel{display:none}
.tab-panel.active{display:block}
.steps{list-style:none;counter-reset:s;margin-bottom:1rem}
.steps li{counter-increment:s;display:flex;gap:.75rem;align-items:flex-start;padding:.65rem 0;font-size:.9rem;line-height:1.5}
.steps li::before{content:counter(s);background:var(--bg3);border:1px solid var(--border);border-radius:50%;min-width:2rem;height:2rem;display:flex;align-items:center;justify-content:center;font-size:.85rem;font-weight:700;color:var(--accent);flex-shrink:0}
.btn{display:inline-block;padding:.55rem 1rem;border-radius:6px;border:none;cursor:pointer;font-size:.88rem;font-weight:600;text-decoration:none;transition:opacity .2s}
.btn-accent{background:var(--accent);color:#fff}
.btn-green{background:var(--green);color:#fff}
.btn-blue{background:var(--blue);color:#fff}
.btn:hover{opacity:.85}
.btn:disabled{opacity:.4;cursor:not-allowed}
.bookmarklet-link{display:inline-block;padding:.55rem 1.2rem;border-radius:6px;background:var(--green);color:#fff;font-size:.88rem;font-weight:700;text-decoration:none;cursor:grab;transition:opacity .2s;user-select:none}
.bookmarklet-link:hover{opacity:.85}
.bookmarklet-link:active{cursor:grabbing}
textarea{width:100%;height:100px;background:var(--bg3);border:1px solid var(--border);border-radius:6px;padding:.65rem .75rem;color:var(--text);font-size:.85rem;font-family:monospace;resize:vertical;outline:none}
textarea:focus{border-color:var(--accent)}
.hint{font-size:.78rem;color:var(--muted);margin-top:.35rem}
.error-msg{color:var(--accent);font-size:.85rem;margin-top:.5rem;display:none}
.success{display:none;text-align:center;padding:1.5rem 0}
.success .icon{font-size:3rem;margin-bottom:.5rem}
.success h3{color:var(--green);font-size:1.3rem;margin-bottom:.3rem}
.success p{color:var(--muted);font-size:.9rem}
.actions{display:flex;gap:.75rem;justify-content:flex-end;margin-top:1rem}
</style>
</head>
<body>
<div class="card">
  <h2>ChatGPT Pro Setup</h2>
  <p class="subtitle">Connect your ChatGPT Pro subscription ($200/mo plan)</p>

  <div id="wizard">
    <div class="tabs">
      <div class="tab active" onclick="switchTab('quick')">One-Click</div>
      <div class="tab" onclick="switchTab('manual')">Manual Paste</div>
    </div>

    <!-- ══ Tab 1: One-click bookmarklet ══ -->
    <div class="tab-panel active" id="panel-quick">
      <ol class="steps">
        <li>
          <div>
            <strong>Save the bookmarklet</strong><br>
            <span style="color:var(--muted)">Drag this button to your bookmarks bar:</span><br>
            <a class="bookmarklet-link" id="bookmarklet-btn" href="#" style="margin-top:.5rem">&#128273; Get ChatGPT Token</a>
            <div class="hint">Right-click → "Bookmark This Link" also works.</div>
          </div>
        </li>
        <li>
          <div>
            <strong>Log in to ChatGPT</strong><br>
            <span style="color:var(--muted)">Open chatgpt.com and sign in with your Pro account.</span><br>
            <a class="btn btn-accent" href="https://chatgpt.com" target="_blank" style="margin-top:.5rem">Open ChatGPT</a>
          </div>
        </li>
        <li>
          <div>
            <strong>Click the bookmarklet</strong><br>
            <span style="color:var(--muted)">While on chatgpt.com, click the bookmarklet in your bookmarks bar. The token will be sent automatically.</span>
          </div>
        </li>
      </ol>
    </div>

    <!-- ══ Tab 2: Manual paste ══ -->
    <div class="tab-panel" id="panel-manual">
      <ol class="steps">
        <li>
          <div>
            <strong>Log in to ChatGPT</strong><br>
            <span style="color:var(--muted)">Open chatgpt.com and sign in with your Pro account.</span><br>
            <a class="btn btn-accent" href="https://chatgpt.com" target="_blank" style="margin-top:.5rem">Open ChatGPT</a>
          </div>
        </li>
        <li>
          <div>
            <strong>Open the session page</strong><br>
            <span style="color:var(--muted)">After logging in, click below to open the session endpoint.</span><br>
            <a class="btn btn-accent" href="https://chatgpt.com/api/auth/session" target="_blank" style="margin-top:.5rem">Open Session Page</a>
          </div>
        </li>
        <li>
          <div style="width:100%">
            <strong>Paste the JSON here</strong><br>
            <span style="color:var(--muted)">Select all (Ctrl+A / Cmd+A) on that page, copy, then paste below.</span><br>
            <textarea id="json-input" placeholder='{"user":{...},"accessToken":"eyJ..."}'
                      oninput="validateJson()" style="margin-top:.5rem"></textarea>
            <div class="hint">We only extract the access token. Nothing else is stored.</div>
            <div class="error-msg" id="error-msg"></div>
            <div class="actions">
              <button class="btn btn-green" id="btn-save" disabled onclick="saveConfig()">Save</button>
            </div>
          </div>
        </li>
      </ol>
    </div>
  </div>

  <div class="success" id="success">
    <div class="icon">&#10003;</div>
    <h3>Setup Complete!</h3>
    <p>ChatGPT Pro has been configured. The system will restart automatically.</p>
    <p id="countdown" style="margin-top:.5rem;color:var(--muted);font-size:.85rem">Redirecting in 4...</p>
  </div>
</div>

<script>
(function(){
  // Build the bookmarklet href dynamically using this page's origin.
  var origin = window.location.origin;
  var bmCode = "javascript:void(fetch('/api/auth/session')"
    + ".then(function(r){return r.json()})"
    + ".then(function(d){if(d.accessToken){"
    + "window.location='" + origin + "/api/setup/chatgpt/callback?token='+encodeURIComponent(d.accessToken)"
    + "}else{alert('Please log in to ChatGPT first.')}})"
    + ".catch(function(){alert('Error: make sure you are on chatgpt.com and logged in.')}))";
  document.getElementById('bookmarklet-btn').href = bmCode;

  // Tab switching.
  window.switchTab = function(tab) {
    document.querySelectorAll('.tab').forEach(function(el,i){
      el.classList.toggle('active', (tab==='quick' ? i===0 : i===1));
    });
    document.getElementById('panel-quick').classList.toggle('active', tab==='quick');
    document.getElementById('panel-manual').classList.toggle('active', tab==='manual');
  };

  // Manual paste validation.
  window.validateJson = function() {
    var raw = document.getElementById('json-input').value.trim();
    var errEl = document.getElementById('error-msg');
    var btn = document.getElementById('btn-save');
    errEl.style.display = 'none';
    btn.disabled = true;
    if (!raw) return;
    try {
      var obj = JSON.parse(raw);
      if (!obj.accessToken) {
        errEl.textContent = 'No "accessToken" found. Make sure you copied the full page.';
        errEl.style.display = 'block';
        return;
      }
      btn.disabled = false;
    } catch(e) {
      errEl.textContent = 'Invalid JSON format. Please copy the entire page content.';
      errEl.style.display = 'block';
    }
  };

  window.saveConfig = function() {
    var raw = document.getElementById('json-input').value.trim();
    var errEl = document.getElementById('error-msg');
    var btn = document.getElementById('btn-save');
    btn.disabled = true;
    fetch('/api/setup/chatgpt', {
      method: 'POST',
      headers: {'Content-Type': 'application/json'},
      body: JSON.stringify({session_json: raw})
    })
    .then(function(r){ return r.json(); })
    .then(function(res){
      if (res.ok) { showSuccess(); }
      else {
        errEl.textContent = res.error || 'Unknown error';
        errEl.style.display = 'block';
        btn.disabled = false;
      }
    })
    .catch(function(e){
      errEl.textContent = 'Network error: ' + e;
      errEl.style.display = 'block';
      btn.disabled = false;
    });
  };

  function showSuccess() {
    document.getElementById('wizard').style.display = 'none';
    document.getElementById('success').style.display = 'block';
    var secs = 4;
    var el = document.getElementById('countdown');
    var iv = setInterval(function(){
      secs--;
      if (secs <= 0) { clearInterval(iv); window.location.href = '/'; }
      else { el.textContent = 'Redirecting in ' + secs + '...'; }
    }, 1000);
  }
})();
</script>
</body>
</html>
"##;

/// Success HTML returned by the bookmarklet callback.
const CALLBACK_SUCCESS_HTML: &str = r##"<!DOCTYPE html>
<html><head><meta charset="utf-8"><title>Setup Complete</title></head>
<body style="background:#1a1a2e;color:#e4e4e4;display:flex;align-items:center;justify-content:center;height:100vh;font-family:sans-serif">
<div style="text-align:center">
  <div style="font-size:3rem">&#10003;</div>
  <h2 style="color:#4ecca3;margin:.5rem 0">Setup Complete!</h2>
  <p style="color:#8a8a9a">ChatGPT Pro has been configured. The system will restart.</p>
  <p style="color:#8a8a9a;margin-top:.5rem;font-size:.85rem">You can close this tab.</p>
</div>
<script>setTimeout(function(){try{window.close()}catch(e){}},3000)</script>
</body></html>
"##;

/// Error HTML template for the bookmarklet callback (`{ERROR}` is replaced).
const CALLBACK_ERROR_HTML: &str = r##"<!DOCTYPE html>
<html><head><meta charset="utf-8"><title>Setup Error</title></head>
<body style="background:#1a1a2e;color:#e4e4e4;display:flex;align-items:center;justify-content:center;height:100vh;font-family:sans-serif">
<div style="text-align:center">
  <div style="font-size:3rem">&#10007;</div>
  <h2 style="color:#e94560;margin:.5rem 0">Setup Failed</h2>
  <p style="color:#8a8a9a">{ERROR}</p>
  <p style="color:#8a8a9a;margin-top:.5rem;font-size:.85rem"><a href="/setup/chatgpt" style="color:#4e9bff">Try again</a></p>
</div>
</body></html>
"##;

// ── tests ───────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;
    use tempfile::NamedTempFile;

    #[test]
    fn write_chatgpt_env_creates_new_section() {
        let mut f = NamedTempFile::new().unwrap();
        writeln!(f, "# existing\nOPENAI_API_KEY=sk-test").unwrap();

        write_chatgpt_env(f.path(), "eyJ_test_token").unwrap();

        let content = std::fs::read_to_string(f.path()).unwrap();
        assert!(content.contains("CHATGPT_SESSION_TOKEN=eyJ_test_token"));
        assert!(content.contains("OPENINTENT_PROVIDER=chatgpt-web"));
        // Existing content preserved.
        assert!(content.contains("OPENAI_API_KEY=sk-test"));
    }

    #[test]
    fn write_chatgpt_env_replaces_existing() {
        let mut f = NamedTempFile::new().unwrap();
        writeln!(f, "CHATGPT_SESSION_TOKEN=old_token\nOTHER=val").unwrap();

        write_chatgpt_env(f.path(), "new_token").unwrap();

        let content = std::fs::read_to_string(f.path()).unwrap();
        assert!(content.contains("CHATGPT_SESSION_TOKEN=new_token"));
        assert!(!content.contains("old_token"));
        assert!(content.contains("OTHER=val"));
    }

    #[test]
    fn write_chatgpt_env_empty_file() {
        let f = NamedTempFile::new().unwrap();

        write_chatgpt_env(f.path(), "token123").unwrap();

        let content = std::fs::read_to_string(f.path()).unwrap();
        assert!(content.contains("CHATGPT_SESSION_TOKEN=token123"));
        assert!(content.contains("OPENINTENT_PROVIDER=chatgpt-web"));
    }
}
