<p align="center">
  <img src="assets/logo.png" width="160" alt="OpenIntentOS" />
</p>

<h1 align="center">OpenIntentOS</h1>
<h3 align="center">The Intent-Driven AI Operating System</h3>

<p align="center">
  Built entirely in Rust. &lt;10MB binary. &lt;200ms cold start. 7-provider cascade failover.<br/>
  <strong>One binary. Say what you want. The OS handles the rest.</strong>
</p>

<p align="center">
  <a href="https://openintentos.github.io/OpenIntentOS/">Documentation</a> &bull;
  <a href="https://openintentos.github.io/OpenIntentOS/getting-started.html">Quick Start</a> &bull;
  <a href="https://openintentos.github.io/OpenIntentOS/architecture.html">Architecture</a>
</p>

<p align="center">
  <img src="https://img.shields.io/badge/language-Rust%202024-orange?style=flat-square" alt="Rust" />
  <img src="https://img.shields.io/badge/license-MIT-blue?style=flat-square" alt="MIT" />
  <img src="https://img.shields.io/github/v/release/OpenIntentOS/OpenIntentOS?style=flat-square&color=green" alt="latest release" />
  <img src="https://img.shields.io/badge/binary-~10MB-brightgreen?style=flat-square" alt="Binary size" />
  <img src="https://img.shields.io/badge/cold%20start-%3C200ms-brightgreen?style=flat-square" alt="Cold start" />
  <img src="https://img.shields.io/badge/providers-7%20cascade-brightgreen?style=flat-square" alt="LLM providers" />
  <img src="https://img.shields.io/badge/adapters-20%2B-brightgreen?style=flat-square" alt="Adapters" />
</p>

---

> **v0.1.10 — Early Release (March 2026)**
>
> OpenIntentOS is under active development. Core systems are production-stable. New adapters and capabilities ship weekly. Pin to a specific commit for stability until v1.0. [Report issues here.](https://github.com/OpenIntentOS/OpenIntentOS/issues)

---

## Install

**macOS · Linux · WSL · Raspberry Pi · Android (Termux)**
```bash
curl -fsSL https://raw.githubusercontent.com/OpenIntentOS/OpenIntentOS/main/install.sh | bash
```

**Windows 10 / 11** — open PowerShell and run:
```powershell
irm https://raw.githubusercontent.com/OpenIntentOS/OpenIntentOS/main/install.ps1 | iex
```

That's it. The installer will:
1. Auto-detect your OS and download the right prebuilt binary
2. Walk you through entering API keys — with a direct link for each one
3. Install a system service so the bot survives reboots and auto-restarts on crash
4. Start the bot immediately

**No Rust, no Docker, no terminal experience required.**

| Platform | Architecture | Install command |
|----------|-------------|-----------------|
| macOS | Apple Silicon (M1/M2/M3/M4) | `curl ... \| bash` |
| macOS | Intel | `curl ... \| bash` |
| Linux | x86\_64 (PC / VPS / WSL) | `curl ... \| bash` |
| Linux | ARM64 (Raspberry Pi 4/5 · AWS Graviton) | `curl ... \| bash` |
| Windows 10/11 | x64 | `irm ... \| iex` |
| Windows | ARM64 (Snapdragon laptops) | `irm ... \| iex` |
| Android | Termux (ARM64) | `curl ... \| bash` |

---

## No API Key? Start for Free

You don't need a paid subscription to get started. Pick any one:

### 🇨🇳 China — No VPN Required

| Provider | Free Tier | Register |
|----------|-----------|---------|
| **硅基流动 SiliconFlow** ⭐ | 14M tokens/month, hosts DeepSeek/Qwen/GLM | [siliconflow.cn](https://siliconflow.cn) |
| **DeepSeek** | ¥10 free credit | [platform.deepseek.com](https://platform.deepseek.com) |
| **Moonshot Kimi** | Free trial credit | [platform.moonshot.cn](https://platform.moonshot.cn) |
| **智谱 GLM-4-Flash** | Permanently free | [open.bigmodel.cn](https://open.bigmodel.cn) |

### 🌍 International

| Provider | Free Tier | Register |
|----------|-----------|---------|
| **NVIDIA NIM** ⭐ | 1000 credits/month, 400B+ models | [build.nvidia.com](https://build.nvidia.com) |
| **Google Gemini** | Generous free limits | [aistudio.google.com/apikey](https://aistudio.google.com/apikey) |
| **Groq** | Free, fastest inference | [console.groq.com](https://console.groq.com) |

### Run Locally (No Internet)

```bash
# Install Ollama
curl -fsSL https://ollama.com/install.sh | sh
ollama pull qwen2.5

# OpenIntentOS will auto-detect Ollama — no key needed
openintent serve
```

---

## What is OpenIntentOS?

OpenIntentOS is an **Intent-Driven AI Operating System** — not a chatbot, not a Python wrapper around an LLM, not a "multi-agent framework."

Users express **what they want** in natural language. The system understands the intent, plans execution across tools, manages credentials securely, and delivers results — autonomously, with no babysitting required.

The entire system ships as a **single ~10MB binary**. No Node.js runtime, no Docker pull, no pip install.

---

## What it looks like

```
User: Search GitHub for Rust async benchmarks, summarize the top 3 papers, save to /reports

→ web_search("Rust async runtime benchmarks 2025")           [0.8s]
→ web_fetch("https://arxiv.org/abs/...")                     [1.1s]
→ web_fetch("https://github.com/tokio-rs/tokio/discussions") [0.9s]
→ fs_write_file("reports/rust-async-benchmarks.md", ...)     [<1ms]
✓ Done — summary saved. 3 sources cited. 1,204 tokens used.
```

```
User: Check my unread emails, summarize anything urgent, post to the team Feishu group

→ email_list_inbox(unread=true, limit=20)                    [0.6s]
→ [analyzing 20 emails via ReAct reasoning]
→ feishu_send_message(group="Product Team", text="...")      [0.4s]
✓ Done — 3 urgent threads summarized, posted to Feishu.
```

```
User: 发一条抖音视频，标题是"周报回顾"，文件在 /tmp/weekly.mp4

→ douyin_publish_video(file_path="/tmp/weekly.mp4", caption="周报回顾")   [8.2s]
✓ 发布成功 — item_id: 7xxxxxxxxxxxxxxx
```

---

## Cascading LLM Failover — Seven Providers, Zero Downtime

This is the core reliability feature. When your primary provider fails for any reason — rate limit, auth error, 503, model not found — OpenIntentOS automatically tries the next provider in the chain, transparently, in the same request.

```
Primary fails (429 / 401 / 404 / 502 / 503)
       │
       ├─► DeepSeek               deepseek-chat           (free credit)
       ├─► SiliconFlow 硅基流动   DeepSeek-V3             (free 14M/month, 🇨🇳)
       ├─► Moonshot Kimi           moonshot-v1-8k          (free credit, 🇨🇳)
       ├─► Zhipu GLM-4-Flash       glm-4-flash             (permanently free, 🇨🇳)
       ├─► Google Gemini           gemini-2.5-flash        (free tier)
       ├─► Groq                    llama-3.3-70b-versatile (free tier)
       ├─► NVIDIA NIM              qwen/qwen3.5-397b-a17b  (free tier)
       ├─► NVIDIA NIM              moonshotai/kimi-k2.5
       ├─► NVIDIA NIM              nvidia/nemotron-3-nano-30b-a3b
       └─► Ollama                  qwen2.5:latest          (local, offline)
```

Each provider has a 120-second cooldown after failure. After cascade exhaustion, the bot restores the primary and retries on the next message. Per-chat model overrides are preserved across failovers.

```
WARN  OpenAI 429 rate limit — starting cascade failover
INFO  trying NVIDIA NIM (qwen/qwen3.5-397b-a17b)
INFO  ✓ NVIDIA NIM responded in 1.2s — continuing on this provider
INFO  primary provider restored after 120s cooldown
```

---

## OpenIntentOS vs The Landscape

All data from official documentation and public repositories — February 2026.

#### Cold Start Time (lower is better)

```
OpenIntentOS  ████░░░░░░░░░░░░░░░░░░░░░░░░░░░░░░░░░░  <200ms    ★
LangGraph     █████████████████░░░░░░░░░░░░░░░░░░░░░   2.5s
CrewAI        ████████████████████░░░░░░░░░░░░░░░░░░░   3.0s
AutoGen       ██████████████████████████░░░░░░░░░░░░░   4.0s
OpenClaw      █████████████████████████████████████░░   ~6s
```

#### Binary / Install Size (lower is better)

```
OpenIntentOS  ██░░░░░░░░░░░░░░░░░░░░░░░░░░░░░░░░░░░░   ~10MB    ★
CrewAI        ████████░░░░░░░░░░░░░░░░░░░░░░░░░░░░░░  ~100MB
LangGraph     ████████████░░░░░░░░░░░░░░░░░░░░░░░░░░  ~150MB
AutoGen       ████████████████░░░░░░░░░░░░░░░░░░░░░░  ~200MB
OpenClaw      ████████████████████████████████████░░  ~500MB
```

#### Idle Memory (lower is better)

```
OpenIntentOS  █░░░░░░░░░░░░░░░░░░░░░░░░░░░░░░░░░░░░░  <15MB     ★
LangGraph     █████████████████░░░░░░░░░░░░░░░░░░░░░  ~180MB
CrewAI        ████████████████████░░░░░░░░░░░░░░░░░░  ~200MB
AutoGen       █████████████████████████░░░░░░░░░░░░░  ~250MB
OpenClaw      ████████████████████████████████████░░  ~394MB
```

### Feature Comparison

| Feature | OpenIntentOS | OpenClaw | CrewAI | AutoGen | LangGraph |
|---------|:---:|:---:|:---:|:---:|:---:|
| **Language** | **Rust 2024** | TypeScript | Python | Python | Python |
| **Binary size** | **~10 MB** | ~500 MB | ~100 MB | ~200 MB | ~150 MB |
| **Cold start** | **<200ms** | ~6s | ~3s | ~4s | ~2.5s |
| **Idle RAM** | **<15 MB** | ~394 MB | ~200 MB | ~250 MB | ~180 MB |
| **LLM cascade failover** | **7 providers** | manual | manual | manual | manual |
| **Encrypted vault** | **AES-256-GCM** | config file | none | none | none |
| **Wasm sandbox** | **wasmtime** | none | none | Docker | none |
| **Chat interfaces** | **Telegram · Discord · Feishu · WeChat · DingTalk · WeCom · QQ** | Telegram · Discord | none | none | none |
| **UI** | **CLI · TUI · Web · Desktop** | Web only | none | none | none |
| **SIMD intent router** | **aho-corasick** | none | none | none | none |
| **Autonomous schedules** | **cron adapter** | cron | none | none | none |
| **Local LLM (offline)** | **Ollama fallback** | Ollama | Ollama | Ollama | Ollama |
| **Zero-copy IPC** | **rkyv** | JSON | none | none | none |
| **Hot config reload** | **yes** | no | no | no | no |
| **License** | MIT | MIT | MIT | Apache 2.0 | MIT |

---

## Performance

| Scenario | Target | How |
|----------|--------|-----|
| Cold start | **< 200ms** | Single binary, no runtime, connection warmup |
| Simple intent | **< 2µs** | SIMD aho-corasick pattern match, no LLM call |
| SQLite read (hot) | **< 0.05µs** | moka lock-free cache |
| SQLite read (warm) | **< 5µs** | WAL + mmap 256 MiB |
| IPC message | **< 1µs** | rkyv zero-copy deserialization |
| Pattern matching | **~2 GB/s** | SIMD aho-corasick |
| Vector search | **< 0.5ms** | usearch, Rust native |
| Local classification | **< 15ms** | ONNX Runtime (ort) |
| Memory footprint | **< 15 MB** | Rust zero-overhead + mmap |
| Binary size | **< 10 MB** | LTO + strip + panic=abort |

---

## Architecture — Five-Layer Kernel

```
┌─────────────────────────────────────────────────────────────────┐
│  Layer 5 · Intent Interface                                     │
│  Telegram · Discord · Feishu · WeChat OA · DingTalk · WeCom     │
│  QQ Bot · REST · Cron · Natural language                        │
├─────────────────────────────────────────────────────────────────┤
│  Layer 4 · Agent Runtime  (tokio, async)                        │
│  Planner ── Executor ── Reflector ── LLM Router                 │
│            ReAct Loop  (zero-copy, same runtime)                │
├─────────────────────────────────────────────────────────────────┤
│  Layer 3 · Micro-Kernel                                         │
│  Scheduler (crossbeam)  Auth Vault (AES-256)  3-layer Memory    │
│  IPC Bus (rkyv)         Wasm Sandbox          ABAC Policy       │
│  Intent Router (SIMD)   Service Registry                        │
├─────────────────────────────────────────────────────────────────┤
│  Layer 2 · Adapters  (async trait, tokio)                       │
│  Filesystem  Shell  Web Search  Web Fetch  HTTP  Browser(CDP)   │
│  Email(IMAP) Calendar(CalDAV)  GitHub  Feishu  Slack  Cron      │
│  WeChat OA  DingTalk  WeCom  QQ Bot  Bilibili  Douyin  XHS      │
│  Weibo  Memory  Skills(WeCom·SMS·Weibo·Twitter·Email...)        │
├─────────────────────────────────────────────────────────────────┤
│  Layer 1 · Runtime & Storage                                    │
│  tokio · rusqlite WAL+mmap · moka cache · io_uring/kqueue       │
└─────────────────────────────────────────────────────────────────┘
```

### Crate Graph

```
openintent-cli          (binary — single executable)
├── openintent-kernel   micro-kernel: scheduler, IPC bus, intent router
├── openintent-agent    ReAct loop, LLM client, tool dispatcher
│   └── openintent-kernel
├── openintent-store    SQLite WAL+mmap, 3-layer memory, user accounts
├── openintent-vault    AES-256-GCM encrypted secret storage
├── openintent-intent   intent classification + workflow engine
│   └── openintent-agent
├── openintent-adapters 20+ built-in adapters
│   └── openintent-vault
├── openintent-auth-engine  headless OAuth via CDP
│   └── openintent-vault
├── openintent-ui       iced GPU desktop UI
├── openintent-web      axum HTTP server + SSR
└── openintent-tui      ratatui terminal UI
```

---

## Built-in Adapters

### System & Productivity

| Adapter | Capabilities |
|---------|-------------|
| **Filesystem** | Read, write, list, search files. UTF-8-safe 16 KB truncation on large reads. |
| **Shell** | Execute commands with timeout, capture stdout/stderr. Full subprocess isolation. |
| **Web Search** | DuckDuckGo search with result ranking. No API key required. |
| **Web Fetch** | Full page fetch with text extraction. Handles JS-heavy sites via browser adapter. |
| **HTTP Request** | Arbitrary HTTP API calls — any method, headers, body. |
| **Browser (CDP)** | Chromium DevTools Protocol. Navigate, click, fill, screenshot. Headless OAuth flows. |
| **Email (IMAP/SMTP)** | Read inbox, send messages, manage folders. OAuth 2.0 with auto token refresh. |
| **Calendar (CalDAV)** | Create, read, update events. Works with Apple Calendar, Nextcloud, Google. |
| **GitHub** | List repos, read issues/PRs, post comments. Used for self-repair via evolution engine. |
| **Cron** | Schedule recurring tasks. Persistent across restarts via SQLite. |
| **Memory** | Working, episodic, and semantic memory layers. Vector search via usearch. |

### Messaging & Chat

| Adapter | Capabilities | Auth |
|---------|-------------|------|
| **Telegram** | Send messages, files, inline keyboards. Full bot API. | `TELEGRAM_BOT_TOKEN` |
| **Discord** | Send to channels, DMs, embeds. | `DISCORD_BOT_TOKEN` |
| **Slack** | Post to channels, DMs, threads. | `SLACK_BOT_TOKEN` |
| **Feishu / Lark** | Send to groups and DMs. Enterprise messaging. | `FEISHU_APP_*` |
| **WeChat OA** | Send text/image/news, get followers. Official Account API. | `WECHAT_OA_APP_*` |
| **DingTalk** | Webhook robot or enterprise app. Text, Markdown, action card. | `DINGTALK_*` |
| **WeCom (企业微信)** | Group robot or internal app. Text, Markdown, member list. | `WECOM_CORP_*` |
| **QQ Official Bot** | Channel messages, C2C direct messages, image push. | `QQ_BOT_APP_*` |

### Content & Social

| Adapter | Capabilities | Auth |
|---------|-------------|------|
| **Douyin (抖音)** | OAuth2, video upload & publish, video list & stats. | `DOUYIN_CLIENT_*` |
| **XiaoHongShu (小红书)** | Publish notes, search, stats, comments. HMAC-SHA256 signed. | `XHS_APP_*` |
| **Weibo** | OAuth2, post status, read mentions, reply. | `WEIBO_APP_*` |
| **Bilibili Open Live** | Live room info, send danmaku, danmaku history. | `BILI_ACCESS_KEY_*` |

### Skills (script-based, no Rust required)

| Skill | Capabilities | Auth |
|-------|-------------|------|
| **wecom-bot** | WeCom group robot: text, Markdown, news card. | `WECOM_WEBHOOK_URL` |
| **sms-tencent** | Tencent Cloud SMS, TC3-HMAC-SHA256 signed. | `TENCENTCLOUD_*` |
| **sms-aliyun** | Alibaba Cloud SMS, HMAC-SHA1 signed. | `ALIYUN_*` |
| **weibo-post** | Weibo post/mentions/reply via shell. | `WEIBO_ACCESS_TOKEN` |
| **twitter-manager** | Post tweets and manage Twitter via OAuth1. | `TWITTER_*` |
| **email-automation** | Send email via SMTP with Python. | SMTP env vars |
| **daily-briefing** | Morning digest: weather + news + calendar. | configurable |
| **web-search-plus** | Enhanced web search via shell. | none |
| **weather-check** | Weather lookup by city. | `OPENWEATHER_API_KEY` |
| **ip-lookup** | Geo-locate an IP address. | none |

---

## Bot Commands

| Command | Description |
|---------|-------------|
| `/models` | Inline keyboard of all available model aliases |
| `/model <alias>` | Switch LLM for this chat — persists across restarts |
| `/tokens on\|off` | Toggle token usage display after each response |
| `/status` | Current provider, model, and health |
| `/clear` | Clear conversation history for this chat |
| `/reset` | Restore default provider and model |
| `/help` | List all commands |

### Model Aliases

| Alias | Provider | Model | Free? |
|-------|----------|-------|-------|
| `claude` | Anthropic | claude-sonnet-4-6 | |
| `gpt4o` | OpenAI | gpt-4o | |
| `deepseek` | DeepSeek | deepseek-chat | ✓ credit |
| `siliconflow` / `sf` | SiliconFlow 硅基流动 | DeepSeek-V3 | ✓ 14M/mo 🇨🇳 |
| `sf-qwen` | SiliconFlow | Qwen3-235B | ✓ 🇨🇳 |
| `moonshot` / `kimi-cn` | Moonshot Kimi | moonshot-v1-8k | ✓ credit 🇨🇳 |
| `glm` / `glm-flash` | Zhipu GLM | glm-4-flash | ✓ free 🇨🇳 |
| `tongyi` | Tongyi Qwen | qwen-turbo | ✓ tier 🇨🇳 |
| `nvidia` | NVIDIA NIM | qwen/qwen3.5-397b-a17b | ✓ 1000/mo |
| `gemini` | Google | gemini-2.5-flash | ✓ tier |
| `groq` | Groq | llama-3.3-70b-versatile | ✓ free |
| `grok` | xAI | grok-3 | |
| `mistral` | Mistral | mistral-large | |
| `ollama` / `local` | Ollama | qwen2.5 | ✓ local |

---

## Security

| Layer | Mechanism |
|-------|-----------|
| **Credential storage** | AES-256-GCM encrypted vault. Master key in macOS Keychain / Linux Secret Service. |
| **Transport** | TLS 1.2+ enforced on all outbound connections via reqwest + rustls. |
| **Wasm sandbox** | Third-party plugin code runs in wasmtime with fuel metering. |
| **Path traversal** | Filesystem adapter canonicalises paths and rejects symlink escapes. |
| **Secret in memory** | API keys held in locked memory and zeroed after use. |
| **OAuth flows** | PKCE + state parameter on all OAuth 2.0 flows. Tokens stored encrypted. |
| **Audit log** | Append-only log of every external action (SQLite). |
| **No telemetry** | Fully self-hosted. Zero data leaves your machine except explicit LLM API calls. |

---

## Quick Start

### One-line install (recommended)

**macOS / Linux / WSL / Raspberry Pi / Termux:**
```bash
curl -fsSL https://raw.githubusercontent.com/OpenIntentOS/OpenIntentOS/main/install.sh | bash
```

**Windows 10 / 11** — open PowerShell as normal user (no admin needed):
```powershell
irm https://raw.githubusercontent.com/OpenIntentOS/OpenIntentOS/main/install.ps1 | iex
```

The installer will guide you through everything step by step.

### What the installer does

```
Step 1/5 · Detecting your system
  ✓  macOS (Apple Silicon)

Step 2/5 · Downloading OpenIntentOS
  ✓  Downloaded 9.8 MB binary (v0.1.0)

Step 3/5 · Connect your AI providers
  📱 Telegram Bot Token
     Don't have one? Open Telegram → @BotFather → /newbot
  Enter: ████████████████████████

  🧠 OpenAI API Key  (https://platform.openai.com/api-keys)
  Enter: ████████████████████████

  🧠 NVIDIA NIM Key  (free tier — https://build.nvidia.com)
  Enter: (skipped)

Step 4/5 · Installing system service
  ✓  macOS LaunchAgent installed (auto-starts on login)

Step 5/5 · Verifying bot is running
  ✓  Bot is running

  ✓  OpenIntentOS is installed and running!
     Open Telegram and message your bot to get started.
```

### After install — useful commands

```bash
~/.openintentos/status.sh     # check if bot is running + last 20 log lines
~/.openintentos/restart.sh    # apply config changes without reinstalling
~/.openintentos/uninstall.sh  # remove everything cleanly
tail -f ~/.openintentos/bot.log  # live log stream
```

### Update

```bash
# Run the installer again — existing API keys and data are preserved
curl -fsSL https://raw.githubusercontent.com/OpenIntentOS/OpenIntentOS/main/install.sh | bash
```

### Build from source (developers)

```bash
git clone https://github.com/OpenIntentOS/OpenIntentOS
cd OpenIntentOS
cargo build --release
# Binary: ./target/release/openintent-cli
export TELEGRAM_BOT_TOKEN="..." OPENAI_API_KEY="..."
./target/release/openintent serve
```

---

## Configuration

All runtime behaviour lives in `config/default.toml`. Hot-reloaded on file change — no restart required.

```toml
[provider]
name     = "openai"
base_url = "https://api.openai.com/v1"
model    = "chatgpt-pro"

[bot]
history_window         = 20     # messages of context per chat
show_token_usage       = false  # toggle with /tokens on|off
simple_query_threshold = 120    # chars; short messages use cheap model

[agent]
max_react_turns   = 10
tool_timeout_secs = 30
```

Full reference: [Configuration Docs](https://openintentos.github.io/OpenIntentOS/configuration.html)

---

## Tech Stack

| Component | Technology | Why |
|-----------|-----------|-----|
| Language | **Rust 2024** | Zero-cost abstractions, no GC, no runtime |
| Async | **tokio** | io_uring (Linux), kqueue (macOS) |
| HTTP client | **reqwest** | HTTP/2, connection pooling, rustls |
| Database | **rusqlite WAL+mmap** | <5µs reads, embedded, no daemon |
| Hot cache | **moka** | Lock-free concurrent, no mutex contention |
| Vector search | **usearch** | <0.5ms, Rust native, SIMD |
| Local inference | **ort (ONNX Runtime)** | <15ms classification |
| String matching | **aho-corasick (SIMD)** | ~2 GB/s, zero alloc |
| Serialisation | **rkyv** | Zero-copy deserialization |
| JSON | **simd-json + serde** | SIMD-accelerated parsing |
| Encryption | **ring** | AES-256-GCM, hardware AES-NI |
| Desktop UI | **iced** | Pure Rust GPU-rendered |
| Web UI | **axum + askama** | Lightweight SSR, no JS framework |
| TUI | **ratatui** | Terminal UI for headless servers |
| Browser | **chromiumoxide** | Rust-native CDP client |
| Wasm sandbox | **wasmtime** | Fuel metering, secure isolation |
| Error types | **thiserror** | Library errors; anyhow in CLI only |
| Logging | **tracing** | Structured, async-aware, zero-cost spans |

---

## Documentation

- [Getting Started](https://openintentos.github.io/OpenIntentOS/getting-started.html) — build, configure, run
- [Architecture](https://openintentos.github.io/OpenIntentOS/architecture.html) — five-layer design, crate graph, failover internals
- [Configuration Reference](https://openintentos.github.io/OpenIntentOS/configuration.html) — every config key documented

---

## License

MIT
