//! Embedded single-page HTML frontend.
//!
//! The entire chat UI is contained in a single HTML constant with inline CSS
//! and JavaScript. Supports multi-session management, tool execution indicators,
//! and WebSocket streaming.

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
  --bg-sidebar:#12192e;
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
  --sidebar-w:260px;
}
html,body{height:100%;font-family:-apple-system,BlinkMacSystemFont,"Segoe UI",Roboto,Helvetica,Arial,sans-serif;background:var(--bg);color:var(--text)}
body{display:flex;overflow:hidden}

/* Sidebar */
.sidebar{
  width:var(--sidebar-w);min-width:var(--sidebar-w);height:100%;
  background:var(--bg-sidebar);border-right:1px solid var(--border);
  display:flex;flex-direction:column;transition:margin-left .2s;
  flex-shrink:0;
}
.sidebar.collapsed{margin-left:calc(-1 * var(--sidebar-w))}
.sidebar-header{
  padding:16px;border-bottom:1px solid var(--border);
  display:flex;align-items:center;justify-content:space-between;
}
.sidebar-header h2{font-size:14px;font-weight:600;color:var(--text-muted);text-transform:uppercase;letter-spacing:1px}
.new-session-btn{
  padding:6px 12px;border-radius:8px;border:1px solid var(--border);
  background:transparent;color:var(--accent);cursor:pointer;font-size:13px;
  transition:all .2s;
}
.new-session-btn:hover{background:var(--accent);color:#fff}
.session-list{flex:1;overflow-y:auto;padding:8px}
.session-item{
  padding:10px 12px;border-radius:8px;cursor:pointer;
  margin-bottom:4px;transition:background .15s;
  display:flex;flex-direction:column;gap:2px;
}
.session-item:hover{background:var(--bg-input)}
.session-item.active{background:var(--bg-input);border-left:3px solid var(--accent)}
.session-item .name{font-size:14px;font-weight:500;white-space:nowrap;overflow:hidden;text-overflow:ellipsis}
.session-item .meta{font-size:11px;color:var(--text-muted);display:flex;justify-content:space-between}
.session-item .delete-btn{
  opacity:0;background:none;border:none;color:var(--accent);cursor:pointer;
  font-size:16px;padding:0 4px;transition:opacity .15s;
}
.session-item:hover .delete-btn{opacity:1}
.sidebar-footer{
  padding:12px 16px;border-top:1px solid var(--border);
  font-size:11px;color:var(--text-muted);text-align:center;
}

/* Main area */
.main{flex:1;display:flex;flex-direction:column;min-width:0}

/* Header */
.header{
  display:flex;align-items:center;justify-content:space-between;
  padding:12px 20px;background:var(--bg-secondary);border-bottom:1px solid var(--border);
  flex-shrink:0;
}
.header-left{display:flex;align-items:center;gap:12px}
.toggle-sidebar{
  background:none;border:none;color:var(--text-muted);cursor:pointer;
  font-size:20px;padding:4px;display:flex;align-items:center;
}
.toggle-sidebar:hover{color:var(--text)}
.header h1{font-size:18px;font-weight:600;letter-spacing:.5px}
.header h1 span{color:var(--accent)}
.header-right{display:flex;align-items:center;gap:16px}
.adapter-count{font-size:12px;color:var(--text-muted);background:var(--bg-input);padding:4px 10px;border-radius:12px}
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
.message.user{background:var(--bg-user);align-self:flex-end;border-bottom-right-radius:4px}
.message.assistant{background:var(--bg-assistant);border:1px solid var(--border);border-bottom-left-radius:4px}
.message.assistant p{margin-bottom:8px}
.message.assistant p:last-child{margin-bottom:0}
.message.system-summary{
  background:var(--bg-tool);border:1px dashed var(--border);
  font-size:13px;color:var(--text-muted);font-style:italic;
}

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
.tool-result{
  margin-top:6px;padding:8px 12px;border-radius:6px;
  background:var(--code-bg);font-family:"SF Mono",Monaco,Consolas,monospace;
  font-size:12px;max-height:200px;overflow:auto;
  white-space:pre-wrap;color:var(--text-muted);
}

/* Code blocks */
code{font-family:"SF Mono",Monaco,Consolas,monospace;background:var(--code-bg);padding:2px 6px;border-radius:4px;font-size:13px}
pre{background:var(--code-bg);padding:14px;border-radius:8px;overflow-x:auto;margin:8px 0;border:1px solid var(--border)}
pre code{background:none;padding:0;font-size:13px;line-height:1.5}

/* Input area */
.input-area{
  flex-shrink:0;padding:16px 20px;
  background:var(--bg-secondary);border-top:1px solid var(--border);
}
.input-wrapper{max-width:780px;margin:0 auto;display:flex;gap:10px;align-items:flex-end}
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
.messages::-webkit-scrollbar,.session-list::-webkit-scrollbar{width:6px}
.messages::-webkit-scrollbar-track,.session-list::-webkit-scrollbar-track{background:transparent}
.messages::-webkit-scrollbar-thumb,.session-list::-webkit-scrollbar-thumb{background:var(--border);border-radius:3px}

/* Welcome screen */
.welcome{
  flex:1;display:flex;align-items:center;justify-content:center;
  flex-direction:column;gap:16px;color:var(--text-muted);
}
.welcome h2{font-size:24px;color:var(--text);font-weight:600}
.welcome p{font-size:15px;max-width:500px;text-align:center;line-height:1.6}
.welcome .caps{
  display:flex;gap:12px;flex-wrap:wrap;justify-content:center;margin-top:8px;
}
.welcome .cap{
  padding:8px 16px;border-radius:8px;background:var(--bg-input);
  border:1px solid var(--border);font-size:13px;color:var(--text);
}

/* Responsive */
@media(max-width:768px){
  .sidebar{position:fixed;left:0;top:0;z-index:100;box-shadow:2px 0 20px rgba(0,0,0,.5)}
  .sidebar.collapsed{margin-left:calc(-1 * var(--sidebar-w))}
  .messages{padding:12px 10px}
  .input-area{padding:12px 10px}
  .message{padding:12px 14px;font-size:14px}
}
</style>
</head>
<body>

<div class="sidebar" id="sidebar">
  <div class="sidebar-header">
    <h2>Sessions</h2>
    <button class="new-session-btn" id="newSessionBtn">+ New</button>
  </div>
  <div class="session-list" id="sessionList"></div>
  <div class="sidebar-footer">
    OpenIntentOS v0.1.0
  </div>
</div>

<div class="main">
  <div class="header">
    <div class="header-left">
      <button class="toggle-sidebar" id="toggleSidebar">&#9776;</button>
      <h1><span>Open</span>IntentOS</h1>
    </div>
    <div class="header-right">
      <span class="adapter-count" id="adapterCount"></span>
      <div class="status">
        <div class="status-dot" id="statusDot"></div>
        <span id="statusText">Connecting...</span>
      </div>
    </div>
  </div>

  <div class="messages" id="messages">
    <div class="welcome" id="welcome">
      <h2>Welcome to OpenIntentOS</h2>
      <p>Your AI-powered operating system. Ask me anything or use tools to manage files, run commands, search the web, and more.</p>
      <div class="caps">
        <div class="cap">File Management</div>
        <div class="cap">Shell Commands</div>
        <div class="cap">Web Search</div>
        <div class="cap">HTTP Requests</div>
        <div class="cap">Memory</div>
        <div class="cap">Cron Jobs</div>
      </div>
    </div>
  </div>

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
</div>

<script>
(function() {
  "use strict";

  const messagesEl   = document.getElementById("messages");
  const welcomeEl    = document.getElementById("welcome");
  const inputEl      = document.getElementById("input");
  const sendBtn      = document.getElementById("sendBtn");
  const statusDot    = document.getElementById("statusDot");
  const statusText   = document.getElementById("statusText");
  const sidebar      = document.getElementById("sidebar");
  const sessionList  = document.getElementById("sessionList");
  const newSessionBtn= document.getElementById("newSessionBtn");
  const toggleBtn    = document.getElementById("toggleSidebar");
  const adapterCount = document.getElementById("adapterCount");

  let ws = null;
  let isProcessing = false;
  let currentSessionId = null;
  let sessions = [];

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
      loadSessions();
      loadAdapters();
    };

    ws.onmessage = function(evt) {
      try {
        handleServerMessage(JSON.parse(evt.data));
      } catch(e) {
        console.error("Parse error:", e);
      }
    };

    ws.onclose = function() {
      setStatus("disconnected");
      updateSendButton();
      setTimeout(connect, 3000);
    };

    ws.onerror = function() { setStatus("disconnected"); };
  }

  function setStatus(state) {
    statusDot.className = "status-dot " + state;
    const labels = {connecting:"Connecting...",connected:"Connected",disconnected:"Disconnected"};
    statusText.textContent = labels[state] || state;
  }

  // -------------------------------------------------------------------
  // Sessions API
  // -------------------------------------------------------------------

  async function loadSessions() {
    try {
      const res = await fetch("/api/sessions");
      sessions = await res.json();
      renderSessionList();
      // Auto-select latest session or create one
      if (sessions.length === 0) {
        await createNewSession();
      } else if (!currentSessionId) {
        switchSession(sessions[0].id);
      }
    } catch(e) {
      console.error("Failed to load sessions:", e);
    }
  }

  async function createNewSession() {
    try {
      const res = await fetch("/api/sessions", {
        method: "POST",
        headers: {"Content-Type": "application/json"},
        body: JSON.stringify({name: "Session " + (sessions.length + 1)})
      });
      const session = await res.json();
      sessions.unshift(session);
      renderSessionList();
      switchSession(session.id);
    } catch(e) {
      console.error("Failed to create session:", e);
    }
  }

  async function deleteSession(id) {
    try {
      await fetch("/api/sessions/" + id, {method: "DELETE"});
      sessions = sessions.filter(function(s) { return s.id !== id; });
      renderSessionList();
      if (currentSessionId === id) {
        currentSessionId = null;
        clearMessages();
        if (sessions.length > 0) {
          switchSession(sessions[0].id);
        } else {
          await createNewSession();
        }
      }
    } catch(e) {
      console.error("Failed to delete session:", e);
    }
  }

  async function switchSession(id) {
    currentSessionId = id;
    renderSessionList();
    clearMessages();
    // Load session messages
    try {
      const res = await fetch("/api/sessions/" + id + "/messages");
      const messages = await res.json();
      if (messages.length > 0) {
        welcomeEl.style.display = "none";
        messages.forEach(function(msg) {
          if (msg.role === "user") {
            addMessage("user", escapeHtml(msg.content));
          } else if (msg.role === "assistant") {
            addMessage("assistant", renderMarkdown(msg.content));
          }
        });
      } else {
        welcomeEl.style.display = "";
      }
    } catch(e) {
      console.error("Failed to load messages:", e);
    }
  }

  function renderSessionList() {
    sessionList.innerHTML = "";
    sessions.forEach(function(s) {
      const el = document.createElement("div");
      el.className = "session-item" + (s.id === currentSessionId ? " active" : "");
      const date = new Date(s.created_at * 1000);
      const timeStr = date.toLocaleDateString() + " " + date.toLocaleTimeString([], {hour:"2-digit",minute:"2-digit"});
      el.innerHTML =
        '<div style="display:flex;justify-content:space-between;align-items:center">' +
        '<span class="name">' + escapeHtml(s.name) + '</span>' +
        '<button class="delete-btn" data-id="' + s.id + '">&times;</button>' +
        '</div>' +
        '<div class="meta"><span>' + s.message_count + ' msgs</span><span>' + timeStr + '</span></div>';
      el.addEventListener("click", function(e) {
        if (e.target.classList.contains("delete-btn")) {
          e.stopPropagation();
          deleteSession(e.target.dataset.id);
          return;
        }
        switchSession(s.id);
      });
      sessionList.appendChild(el);
    });
  }

  async function loadAdapters() {
    try {
      const res = await fetch("/api/adapters");
      const adapters = await res.json();
      let toolCount = 0;
      adapters.forEach(function(a) { toolCount += a.tools.length; });
      adapterCount.textContent = toolCount + " tools";
    } catch(e) {
      adapterCount.textContent = "";
    }
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
        if (welcomeEl) welcomeEl.style.display = "none";
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
        // Refresh session list to update message counts
        loadSessions();
        break;
    }
  }

  // -------------------------------------------------------------------
  // DOM helpers
  // -------------------------------------------------------------------

  function clearMessages() {
    // Remove all children except the welcome element
    while (messagesEl.firstChild) {
      messagesEl.removeChild(messagesEl.firstChild);
    }
    messagesEl.appendChild(welcomeEl);
    welcomeEl.style.display = "";
  }

  function addMessage(role, html) {
    if (welcomeEl) welcomeEl.style.display = "none";
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
    requestAnimationFrame(function() { messagesEl.scrollTop = messagesEl.scrollHeight; });
  }

  // -------------------------------------------------------------------
  // Send
  // -------------------------------------------------------------------

  function sendMessage() {
    const text = inputEl.value.trim();
    if (!text || isProcessing || !ws || ws.readyState !== WebSocket.OPEN || !currentSessionId) return;

    if (welcomeEl) welcomeEl.style.display = "none";
    addMessage("user", escapeHtml(text));
    ws.send(JSON.stringify({type: "chat", content: text, session_id: currentSessionId}));

    inputEl.value = "";
    autoResize();
    isProcessing = true;
    updateSendButton();
  }

  function updateSendButton() {
    sendBtn.disabled = !(ws && ws.readyState === WebSocket.OPEN && !isProcessing && currentSessionId);
  }

  // -------------------------------------------------------------------
  // Markdown rendering
  // -------------------------------------------------------------------

  function renderMarkdown(text) {
    let html = escapeHtml(text);
    html = html.replace(/```(\w*)\n([\s\S]*?)```/g, function(_, lang, code) {
      return '<pre><code>' + code + '</code></pre>';
    });
    html = html.replace(/`([^`]+)`/g, '<code>$1</code>');
    html = html.replace(/\*\*(.+?)\*\*/g, '<strong>$1</strong>');
    html = html.replace(/(?<!\*)\*(?!\*)(.+?)(?<!\*)\*(?!\*)/g, '<em>$1</em>');
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
    if (e.key === "Enter" && !e.shiftKey) { e.preventDefault(); sendMessage(); }
  });
  inputEl.addEventListener("input", function() { autoResize(); updateSendButton(); });
  newSessionBtn.addEventListener("click", createNewSession);
  toggleBtn.addEventListener("click", function() { sidebar.classList.toggle("collapsed"); });

  // -------------------------------------------------------------------
  // Init
  // -------------------------------------------------------------------

  connect();
})();
</script>
</body>
</html>
"##;
