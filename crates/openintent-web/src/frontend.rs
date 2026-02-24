//! Embedded single-page HTML frontend.
//!
//! The entire chat UI is contained in a single HTML constant with inline CSS
//! and JavaScript.  No external dependencies are required.

/// The complete HTML frontend as a static string.
pub const INDEX_HTML: &str = r##"<!DOCTYPE html>
<html lang="en">
<head>
<meta charset="utf-8">
<meta name="viewport" content="width=device-width, initial-scale=1">
<title>OpenIntentOS</title>
<style>
*,*::before,*::after{box-sizing:border-box;margin:0;padding:0}
:root{
  --bg:#1a1a2e;
  --bg-secondary:#16213e;
  --bg-input:#0f3460;
  --bg-user:#533483;
  --bg-assistant:#16213e;
  --bg-tool:#0a1628;
  --text:#e4e4e4;
  --text-muted:#8a8a9a;
  --accent:#e94560;
  --accent-hover:#ff6b81;
  --border:#2a2a4a;
  --code-bg:#0d1117;
  --success:#4ecca3;
  --warning:#f0a500;
}
html,body{height:100%;font-family:-apple-system,BlinkMacSystemFont,"Segoe UI",Roboto,Helvetica,Arial,sans-serif;background:var(--bg);color:var(--text)}
body{display:flex;flex-direction:column}

/* Header */
.header{
  display:flex;align-items:center;justify-content:space-between;
  padding:12px 20px;background:var(--bg-secondary);border-bottom:1px solid var(--border);
  flex-shrink:0;
}
.header h1{font-size:18px;font-weight:600;letter-spacing:.5px}
.header h1 span{color:var(--accent)}
.status{display:flex;align-items:center;gap:6px;font-size:13px;color:var(--text-muted)}
.status-dot{width:8px;height:8px;border-radius:50%;background:#555;transition:background .3s}
.status-dot.connected{background:var(--success)}
.status-dot.disconnected{background:var(--accent)}
.status-dot.connecting{background:var(--warning);animation:pulse 1s infinite}
@keyframes pulse{0%,100%{opacity:1}50%{opacity:.4}}

/* Messages area */
.messages{
  flex:1;overflow-y:auto;padding:16px 20px;
  display:flex;flex-direction:column;gap:12px;
}
.message{
  max-width:780px;width:100%;margin:0 auto;
  padding:14px 18px;border-radius:12px;
  line-height:1.6;font-size:15px;
  word-wrap:break-word;overflow-wrap:break-word;
}
.message.user{
  background:var(--bg-user);align-self:flex-end;border-bottom-right-radius:4px;
}
.message.assistant{
  background:var(--bg-assistant);border:1px solid var(--border);border-bottom-left-radius:4px;
}
.message.assistant p{margin-bottom:8px}
.message.assistant p:last-child{margin-bottom:0}

/* Tool indicators */
.tool-indicator{
  max-width:780px;width:100%;margin:0 auto;
  padding:10px 16px;border-radius:8px;
  background:var(--bg-tool);border:1px solid var(--border);
  font-size:13px;color:var(--text-muted);
  display:flex;align-items:center;gap:8px;
}
.tool-indicator .spinner{
  width:14px;height:14px;border:2px solid var(--border);
  border-top-color:var(--accent);border-radius:50%;
  animation:spin .8s linear infinite;flex-shrink:0;
}
@keyframes spin{to{transform:rotate(360deg)}}
.tool-indicator.complete{border-color:var(--success);color:var(--success)}
.tool-indicator.complete .icon{color:var(--success)}
.tool-result{
  margin-top:6px;padding:8px 12px;border-radius:6px;
  background:var(--code-bg);font-family:"SF Mono",Monaco,Consolas,monospace;
  font-size:12px;max-height:200px;overflow:auto;
  white-space:pre-wrap;color:var(--text-muted);
}

/* Code blocks */
code{
  font-family:"SF Mono",Monaco,Consolas,monospace;
  background:var(--code-bg);padding:2px 6px;border-radius:4px;font-size:13px;
}
pre{
  background:var(--code-bg);padding:14px;border-radius:8px;
  overflow-x:auto;margin:8px 0;border:1px solid var(--border);
}
pre code{background:none;padding:0;font-size:13px;line-height:1.5}

/* Input area */
.input-area{
  flex-shrink:0;padding:16px 20px;
  background:var(--bg-secondary);border-top:1px solid var(--border);
}
.input-wrapper{
  max-width:780px;margin:0 auto;display:flex;gap:10px;align-items:flex-end;
}
.input-wrapper textarea{
  flex:1;resize:none;padding:12px 16px;border-radius:12px;
  background:var(--bg-input);border:1px solid var(--border);
  color:var(--text);font-size:15px;font-family:inherit;
  line-height:1.5;min-height:48px;max-height:200px;
  outline:none;transition:border-color .2s;
}
.input-wrapper textarea:focus{border-color:var(--accent)}
.input-wrapper textarea::placeholder{color:var(--text-muted)}
.send-btn{
  width:48px;height:48px;border-radius:12px;border:none;
  background:var(--accent);color:#fff;cursor:pointer;
  display:flex;align-items:center;justify-content:center;
  transition:background .2s;flex-shrink:0;
}
.send-btn:hover:not(:disabled){background:var(--accent-hover)}
.send-btn:disabled{opacity:.4;cursor:not-allowed}
.send-btn svg{width:20px;height:20px}

/* Scrollbar */
.messages::-webkit-scrollbar{width:6px}
.messages::-webkit-scrollbar-track{background:transparent}
.messages::-webkit-scrollbar-thumb{background:var(--border);border-radius:3px}
.messages::-webkit-scrollbar-thumb:hover{background:var(--text-muted)}

/* Responsive */
@media(max-width:600px){
  .messages{padding:12px 10px}
  .input-area{padding:12px 10px}
  .message{padding:12px 14px;font-size:14px}
}
</style>
</head>
<body>

<div class="header">
  <h1><span>Open</span>IntentOS</h1>
  <div class="status">
    <div class="status-dot" id="statusDot"></div>
    <span id="statusText">Connecting...</span>
  </div>
</div>

<div class="messages" id="messages"></div>

<div class="input-area">
  <div class="input-wrapper">
    <textarea id="input" placeholder="Type your message..." rows="1"></textarea>
    <button class="send-btn" id="sendBtn" disabled>
      <svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2"
           stroke-linecap="round" stroke-linejoin="round">
        <line x1="22" y1="2" x2="11" y2="13"/>
        <polygon points="22 2 15 22 11 13 2 9 22 2"/>
      </svg>
    </button>
  </div>
</div>

<script>
(function() {
  "use strict";

  const messagesEl = document.getElementById("messages");
  const inputEl    = document.getElementById("input");
  const sendBtn    = document.getElementById("sendBtn");
  const statusDot  = document.getElementById("statusDot");
  const statusText = document.getElementById("statusText");

  let ws = null;
  let isProcessing = false;

  // -------------------------------------------------------------------
  // WebSocket
  // -------------------------------------------------------------------

  function connect() {
    setStatus("connecting");
    const proto = location.protocol === "https:" ? "wss:" : "ws:";
    ws = new WebSocket(proto + "//" + location.host + "/ws");

    ws.onopen = function() {
      setStatus("connected");
      updateSendButton();
    };

    ws.onmessage = function(evt) {
      try {
        const msg = JSON.parse(evt.data);
        handleServerMessage(msg);
      } catch(e) {
        console.error("Failed to parse server message:", e);
      }
    };

    ws.onclose = function() {
      setStatus("disconnected");
      updateSendButton();
      setTimeout(connect, 3000);
    };

    ws.onerror = function() {
      setStatus("disconnected");
    };
  }

  function setStatus(state) {
    statusDot.className = "status-dot " + state;
    const labels = {connecting:"Connecting...",connected:"Connected",disconnected:"Disconnected"};
    statusText.textContent = labels[state] || state;
  }

  // -------------------------------------------------------------------
  // Message handling
  // -------------------------------------------------------------------

  let currentAssistantEl = null;
  let currentAssistantText = "";
  let activeToolEl = null;

  function handleServerMessage(msg) {
    switch(msg.type) {
      case "text":
        if (!currentAssistantEl) {
          currentAssistantEl = addMessage("assistant", "");
          currentAssistantText = "";
        }
        currentAssistantText += msg.content || "";
        currentAssistantEl.innerHTML = renderMarkdown(currentAssistantText);
        scrollToBottom();
        break;

      case "tool_start":
        activeToolEl = addToolIndicator(msg.tool || "tool", true);
        scrollToBottom();
        break;

      case "tool_end":
        if (activeToolEl) {
          completeToolIndicator(activeToolEl, msg.result || "");
          activeToolEl = null;
        }
        scrollToBottom();
        break;

      case "error":
        addMessage("assistant", "Error: " + (msg.content || "Unknown error"));
        isProcessing = false;
        updateSendButton();
        break;

      case "done":
        currentAssistantEl = null;
        currentAssistantText = "";
        activeToolEl = null;
        isProcessing = false;
        updateSendButton();
        break;
    }
  }

  // -------------------------------------------------------------------
  // DOM helpers
  // -------------------------------------------------------------------

  function addMessage(role, html) {
    const el = document.createElement("div");
    el.className = "message " + role;
    el.innerHTML = html;
    messagesEl.appendChild(el);
    scrollToBottom();
    return el;
  }

  function addToolIndicator(toolName, running) {
    const el = document.createElement("div");
    el.className = "tool-indicator";
    if (running) {
      el.innerHTML = '<div class="spinner"></div><span>Running <strong>' +
        escapeHtml(toolName) + '</strong>...</span>';
    }
    messagesEl.appendChild(el);
    return el;
  }

  function completeToolIndicator(el, result) {
    el.classList.add("complete");
    const nameSpan = el.querySelector("strong");
    const name = nameSpan ? nameSpan.textContent : "tool";
    let html = '<span class="icon">&#10003;</span> <strong>' + escapeHtml(name) + '</strong> completed';
    if (result) {
      let display = result;
      if (display.length > 500) display = display.substring(0, 500) + "...";
      html += '<div class="tool-result">' + escapeHtml(display) + '</div>';
    }
    el.innerHTML = html;
  }

  function scrollToBottom() {
    requestAnimationFrame(function() {
      messagesEl.scrollTop = messagesEl.scrollHeight;
    });
  }

  // -------------------------------------------------------------------
  // Send
  // -------------------------------------------------------------------

  function sendMessage() {
    const text = inputEl.value.trim();
    if (!text || isProcessing || !ws || ws.readyState !== WebSocket.OPEN) return;

    addMessage("user", escapeHtml(text));
    ws.send(JSON.stringify({type: "chat", content: text}));

    inputEl.value = "";
    autoResize();
    isProcessing = true;
    updateSendButton();
  }

  function updateSendButton() {
    const canSend = ws && ws.readyState === WebSocket.OPEN && !isProcessing;
    sendBtn.disabled = !canSend;
  }

  // -------------------------------------------------------------------
  // Markdown rendering (basic)
  // -------------------------------------------------------------------

  function renderMarkdown(text) {
    // Escape HTML first, then apply markdown transforms.
    let html = escapeHtml(text);

    // Code blocks: ```lang\ncode\n```
    html = html.replace(/```(\w*)\n([\s\S]*?)```/g, function(_, lang, code) {
      return '<pre><code>' + code + '</code></pre>';
    });

    // Inline code: `code`
    html = html.replace(/`([^`]+)`/g, '<code>$1</code>');

    // Bold: **text**
    html = html.replace(/\*\*(.+?)\*\*/g, '<strong>$1</strong>');

    // Italic: *text*
    html = html.replace(/(?<!\*)\*(?!\*)(.+?)(?<!\*)\*(?!\*)/g, '<em>$1</em>');

    // Line breaks
    html = html.replace(/\n/g, '<br>');

    return html;
  }

  function escapeHtml(str) {
    const div = document.createElement("div");
    div.appendChild(document.createTextNode(str));
    return div.innerHTML;
  }

  // -------------------------------------------------------------------
  // Auto-resize textarea
  // -------------------------------------------------------------------

  function autoResize() {
    inputEl.style.height = "auto";
    inputEl.style.height = Math.min(inputEl.scrollHeight, 200) + "px";
  }

  // -------------------------------------------------------------------
  // Event listeners
  // -------------------------------------------------------------------

  sendBtn.addEventListener("click", sendMessage);

  inputEl.addEventListener("keydown", function(e) {
    if (e.key === "Enter" && !e.shiftKey) {
      e.preventDefault();
      sendMessage();
    }
  });

  inputEl.addEventListener("input", function() {
    autoResize();
    updateSendButton();
  });

  // -------------------------------------------------------------------
  // Init
  // -------------------------------------------------------------------

  connect();
})();
</script>
</body>
</html>
"##;
