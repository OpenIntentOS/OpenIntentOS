# OpenIntentOS — Master Plan (Full Rust Edition)

> An AI-native micro operating system built entirely in Rust for extreme performance.
> Runs on any device, connects to LLMs, manages account authorizations,
> and autonomously operates daily workflows.

## 1. Vision

OpenIntentOS is an **Intent-Driven AI Operating System** — not a chatbot, not a framework.

Users express **what they want** in natural language. The system understands the intent,
orchestrates tools, manages credentials, and executes — with minimal human intervention.

**Design philosophy: Every microsecond matters. Full Rust. Zero compromise.**

### Core Differentiators

| Aspect           | OpenClaw              | OpenIntentOS                        |
|------------------|-----------------------|-------------------------------------|
| Nature           | Message-driven agent  | Intent-driven OS                    |
| Language         | TypeScript/Node.js    | **Full Rust**                       |
| Interaction      | Reactive (responds)   | Proactive (senses + acts)           |
| Architecture     | Gateway single process| Micro-kernel + async workers        |
| Deployment       | Self-hosted server    | Any device (phone/Pi/PC/cloud)      |
| Auth UX          | Config files + OAuth  | AI-driven headless auth (CDP)       |
| UI               | Web only              | **Native GPU UI** + Web + TUI       |
| IPC              | JSON over WebSocket   | **Zero-copy shared memory**         |
| Binary size      | ~200MB (Node.js)      | **~10MB** (single binary)           |
| Cold start       | ~3s                   | **< 200ms**                         |
| Memory           | ~300MB                | **< 15MB**                          |
| Plugin sandbox   | Node.js child process | **Wasm (wasmtime)**                 |

---

## 2. Architecture

```
┌──────────────────────────────────────────────────────────────────┐
│  Layer 5: Intent Interface                                       │
│  Natural language / Voice / GUI (GPUI) / API / Cron triggers     │
├──────────────────────────────────────────────────────────────────┤
│  Layer 4: Agent Runtime (Rust, tokio)                            │
│  ┌───────────┐ ┌───────────┐ ┌───────────┐ ┌──────────────┐    │
│  │ Planner   │ │ Executor  │ │ Reflector │ │ LLM Router   │    │
│  │           │ │           │ │           │ │ local/cloud   │    │
│  └─────┬─────┘ └─────┬─────┘ └─────┬─────┘ └──────┬───────┘    │
│        └─────────────┼─────────────┘               │            │
│                 ReAct Loop (zero-copy, same async runtime)       │
├──────────────────────────────────────────────────────────────────┤
│  Layer 3: Micro-Kernel (Rust)                                    │
│  ┌──────────┐ ┌──────────┐ ┌──────────┐ ┌───────────────────┐  │
│  │Scheduler │ │Auth Vault│ │ Memory   │ │ Service Registry  │  │
│  │lock-free │ │AES-256   │ │ 3-layer  │ │ adapter lifecycle │  │
│  │crossbeam │ │+keychain │ │ W/E/S    │ │ health+reconnect  │  │
│  └──────────┘ └──────────┘ └──────────┘ └───────────────────┘  │
│  ┌──────────┐ ┌──────────┐ ┌──────────┐ ┌───────────────────┐  │
│  │ IPC Bus  │ │ Sandbox  │ │ Policy   │ │ Intent Router     │  │
│  │zero-copy │ │wasmtime  │ │ engine   │ │ SIMD aho-corasick │  │
│  │ rkyv     │ │isolation │ │ ABAC     │ │ exact→pat→LLM     │  │
│  └──────────┘ └──────────┘ └──────────┘ └───────────────────┘  │
├──────────────────────────────────────────────────────────────────┤
│  Layer 2: Adapters (Rust async, tokio)                           │
│  ┌───────┐┌───────┐┌───────┐┌────────┐┌─────────┐┌──────────┐ │
│  │Feishu ││GitHub ││Calendar││ Email  ││Browser  ││Filesystem│ │
│  │       ││       ││(CalDAV)││(IMAP)  ││(CDP)    ││          │ │
│  └───────┘└───────┘└───────┘└────────┘└─────────┘└──────────┘ │
│  ┌───────┐┌───────┐┌───────┐┌────────┐                        │
│  │Notion ││Slack  ││DingTalk││ Shell  │  + Wasm plugin adapters│
│  └───────┘└───────┘└───────┘└────────┘                        │
├──────────────────────────────────────────────────────────────────┤
│  Layer 1: Runtime & Storage                                      │
│  tokio │ rusqlite WAL+mmap │ moka cache │ io_uring/kqueue       │
└──────────────────────────────────────────────────────────────────┘
```

---

## 3. Tech Stack

| Component         | Technology                          | Rationale                             |
|-------------------|-------------------------------------|---------------------------------------|
| Language          | Rust (2024 edition)                 | Zero-cost abstractions, no GC         |
| Async runtime     | tokio                               | Industry standard, io_uring support   |
| HTTP client       | reqwest                             | Async, HTTP/2, connection pooling     |
| WebSocket         | tokio-tungstenite                   | Native async WebSocket                |
| Database          | rusqlite (WAL+mmap)                 | < 5μs reads, embedded                 |
| Hot cache         | moka                                | Lock-free concurrent cache            |
| Vector search     | usearch                             | < 0.5ms, Rust native                  |
| Local inference   | ort (ONNX Runtime) / candle         | < 15ms classification                 |
| String matching   | aho-corasick (SIMD)                 | ~2GB/s throughput                      |
| Serialization     | rkyv (zero-copy)                    | 0 allocation deserialization          |
| JSON              | simd-json + serde                   | SIMD-accelerated JSON parsing         |
| Encryption        | ring                                | AES-256-GCM, hardware AES-NI         |
| Desktop UI        | iced                                | Pure Rust GPU-rendered UI             |
| Web UI            | axum + askama (templates)           | Lightweight, SSR                      |
| TUI               | ratatui                             | Terminal UI for headless devices      |
| Headless browser  | chromiumoxide                       | Rust-native CDP client                |
| Wasm sandbox      | wasmtime                            | Secure plugin execution               |
| CLI               | clap                                | Argument parsing                      |
| Logging           | tracing                             | Structured, async-aware logging       |
| Error handling    | thiserror + anyhow                  | Ergonomic error types                 |
| Config            | toml                                | Human-readable configuration          |
| Distribution      | Single binary (cargo build --release)| ~10MB, all platforms                 |

---

## 4. Project Structure

```
OpenIntentOS/
├── crates/
│   ├── openintent-kernel/       # Micro-kernel core
│   │   └── src/
│   │       ├── lib.rs           # Kernel public API
│   │       ├── scheduler.rs     # Lock-free task scheduler (crossbeam)
│   │       ├── ipc.rs           # Zero-copy message bus (rkyv)
│   │       ├── router.rs        # SIMD intent router (aho-corasick)
│   │       └── registry.rs      # Service/adapter registry
│   ├── openintent-vault/        # Encrypted credential store
│   │   └── src/
│   │       ├── lib.rs
│   │       ├── crypto.rs        # AES-256-GCM (ring)
│   │       ├── keychain.rs      # OS keychain integration
│   │       ├── store.rs         # SQLite credential store
│   │       └── policy.rs        # Permission policy engine
│   ├── openintent-store/        # Storage engine
│   │   └── src/
│   │       ├── lib.rs
│   │       ├── db.rs            # SQLite WAL+mmap setup
│   │       ├── migration.rs     # Schema migrations
│   │       ├── memory.rs        # 3-layer memory manager
│   │       └── cache.rs         # moka hot cache layer
│   ├── openintent-agent/        # Agent runtime
│   │   └── src/
│   │       ├── lib.rs
│   │       ├── runtime.rs       # ReAct loop
│   │       ├── planner.rs       # Task decomposition
│   │       ├── executor.rs      # Step execution
│   │       ├── reflector.rs     # Result evaluation
│   │       └── llm/
│   │           ├── mod.rs       # LLM module
│   │           ├── client.rs    # Multi-provider LLM client
│   │           ├── router.rs    # Model routing (local/cloud)
│   │           ├── streaming.rs # SSE stream parser
│   │           └── types.rs     # Message/tool call types
│   ├── openintent-intent/       # Intent parsing & workflows
│   │   └── src/
│   │       ├── lib.rs
│   │       ├── parser.rs        # NL → structured intent
│   │       ├── workflow.rs      # Persistent workflow engine
│   │       └── trigger.rs       # Cron / event / manual triggers
│   ├── openintent-adapters/     # Built-in adapters
│   │   └── src/
│   │       ├── lib.rs           # Adapter trait + registry
│   │       ├── traits.rs        # Core adapter traits
│   │       ├── filesystem.rs    # Local filesystem operations
│   │       ├── shell.rs         # Shell command execution
│   │       ├── browser.rs       # CDP browser automation
│   │       ├── http_client.rs   # Generic HTTP/REST adapter
│   │       └── email.rs         # IMAP/SMTP adapter
│   ├── openintent-skills/       # Skill system (OpenClaw-compatible)
│   │   └── src/
│   │       ├── lib.rs           # Public API
│   │       ├── error.rs         # Error types
│   │       ├── types.rs         # SkillDefinition, metadata types
│   │       ├── parser.rs        # SKILL.md parser (YAML + markdown)
│   │       ├── loader.rs        # Filesystem skill loader
│   │       ├── registry.rs      # ClawHub registry client
│   │       ├── manager.rs       # Install, remove, list, update
│   │       └── adapter.rs       # SkillAdapter (Adapter trait bridge)
│   ├── openintent-auth-engine/  # Headless auth automation
│   │   └── src/
│   │       ├── lib.rs
│   │       ├── headless.rs      # CDP browser control
│   │       ├── page_analyzer.rs # DOM-based page state detection
│   │       ├── device_code.rs   # RFC 8628 device code flow
│   │       └── flow.rs          # Auth flow orchestration
│   ├── openintent-ui/           # Native desktop UI (iced)
│   │   └── src/
│   │       ├── lib.rs
│   │       ├── app.rs           # Main application
│   │       ├── launcher.rs      # Quick-launch input bar
│   │       ├── chat.rs          # Chat/conversation view
│   │       ├── tasks.rs         # Task status panel
│   │       ├── accounts.rs      # Account management
│   │       ├── theme.rs         # Theming
│   │       └── widgets/         # Custom widgets
│   ├── openintent-web/          # Web UI (axum + askama)
│   │   └── src/
│   │       ├── lib.rs
│   │       ├── server.rs        # Axum HTTP server
│   │       ├── ws.rs            # WebSocket handler
│   │       ├── api.rs           # REST API endpoints
│   │       └── templates/       # HTML templates
│   ├── openintent-tui/          # Terminal UI (ratatui)
│   │   └── src/
│   │       ├── lib.rs
│   │       └── app.rs           # TUI application
│   └── openintent-cli/          # CLI entry point
│       └── src/
│           └── main.rs          # Binary entry, arg parsing
├── config/
│   ├── IDENTITY.md              # System identity definition
│   ├── SOUL.md                  # Behavioral guidelines
│   └── default.toml             # Default configuration
├── skills/                      # Installed skills (gitignored, user-specific)
├── data/                        # Runtime data (gitignored)
├── Cargo.toml                   # Workspace root
├── rust-toolchain.toml          # Pin Rust version
├── .gitignore
├── CLAUDE.md                    # Development guidelines
├── LICENSE
├── README.md
└── PLAN.md                      # This file
```

---

## 5. Core Module Design

### 5.1 Micro-Kernel

#### Scheduler (lock-free)
```rust
// crossbeam SegQueue + priority levels
// Task states: Pending → Queued → Running → Done/Failed/Cancelled
// Supports: immediate, delayed (tokio::time), cron (cron crate), event-triggered
```

#### Intent Router (SIMD-accelerated)
```
Level 1: Exact Match       (< 0.01ms)   aho-corasick multi-pattern
Level 2: Pattern Match     (< 0.1ms)    Compiled regex + slot extraction
Level 3: Local Classifier  (< 50ms)     ONNX Runtime tiny model
Level 4: Cloud LLM         (< 2000ms)   Complex reasoning fallback
```

#### IPC Bus (zero-copy)
```rust
// rkyv for zero-copy deserialization between components
// tokio::sync::broadcast for pub/sub events
// No serialization overhead for in-process communication
```

### 5.2 Auth Vault

- AES-256-GCM encryption via `ring` (hardware AES-NI acceleration)
- Master key: macOS Keychain / Windows DPAPI / Linux Secret Service
- Automatic token refresh with advisory file locks
- Credential types: OAuth2, API Key, Cookie/Session

### 5.3 Auth Engine (AI-driven, headless)

Three engines, auto-selected:

1. **Headless AI Auth** (primary): `chromiumoxide` CDP client drives OAuth
   - Reuses system Chrome/Chromium/Edge (no bundled browser)
   - DOM rule engine detects page state (login/consent/2FA/callback)
   - User only confirms permission scope, AI handles the rest
2. **Inline WebView** (fallback): For CAPTCHA/biometric flows
3. **Device Code Flow** (headless): RFC 8628 for Pi/VPS/SSH

### 5.4 Agent Runtime

```rust
// ReAct loop — entirely in Rust, same tokio runtime
async fn react_loop(ctx: &mut AgentContext) -> Result<AgentResponse> {
    loop {
        let response = ctx.llm.stream_chat(&ctx.messages).await?;
        match response {
            LlmResponse::ToolCalls(calls) => {
                let results = execute_tools_parallel(calls, &ctx.adapters).await?;
                ctx.messages.extend(results); // zero-copy append
            }
            LlmResponse::Text(text) => return Ok(AgentResponse::new(text)),
        }
    }
}
```

### 5.5 LLM Router

```
Local (ONNX / candle):
├── intent-classifier   (50MB, < 20ms)
├── entity-extractor    (80MB, < 30ms)
└── embedding-model     (30MB, < 10ms)

Cloud (reqwest SSE):
├── Anthropic Claude (Opus/Sonnet/Haiku)
├── OpenAI GPT-4o
├── DeepSeek
└── Ollama (local large models)

Routing: local-resolvable → Haiku → Opus (escalation)
```

### 5.6 Memory System

| Layer | Storage | Latency | Lifetime |
|-------|---------|---------|----------|
| Working | In-memory (RAM) | < 0.001ms | Single task |
| Episodic | SQLite JSONL | < 5μs | 30 days |
| Semantic | SQLite + usearch vectors | < 1ms | Permanent |

Hot data cached in moka (lock-free LRU, < 0.05μs).

### 5.7 Adapter System

```rust
#[async_trait]
pub trait Adapter: Send + Sync {
    fn id(&self) -> &str;
    fn adapter_type(&self) -> AdapterType;

    async fn connect(&mut self) -> Result<()>;
    async fn disconnect(&mut self) -> Result<()>;
    async fn health_check(&self) -> Result<HealthStatus>;

    fn tools(&self) -> &[ToolDefinition];
    async fn execute_tool(&self, name: &str, params: Value) -> Result<Value>;

    fn required_auth(&self) -> Option<AuthRequirement>;
}
```

### 5.8 UI Layer

| Mode | Technology | Use case | Latency |
|------|-----------|----------|---------|
| Desktop | iced (GPU) | Primary daily use | < 3ms/frame |
| Web | axum + SSR | Remote/mobile access | standard |
| TUI | ratatui | SSH / headless | instant |

---

## 6. Database Schema

### Main Database (data/openintent.db)

```sql
-- SQLite pragmas for performance
PRAGMA journal_mode = WAL;
PRAGMA synchronous = NORMAL;
PRAGMA mmap_size = 268435456;
PRAGMA cache_size = -64000;
PRAGMA temp_store = MEMORY;

CREATE TABLE workflows (
    id          TEXT PRIMARY KEY,
    name        TEXT NOT NULL,
    description TEXT,
    intent_raw  TEXT NOT NULL,
    steps       TEXT NOT NULL,       -- JSON
    trigger     TEXT,                -- JSON: cron/event/manual
    enabled     BOOLEAN DEFAULT 1,
    created_at  INTEGER NOT NULL,
    updated_at  INTEGER NOT NULL
);

CREATE TABLE tasks (
    id           TEXT PRIMARY KEY,
    workflow_id  TEXT REFERENCES workflows(id),
    status       TEXT NOT NULL CHECK(status IN ('pending','running','completed','failed','cancelled')),
    input        TEXT,               -- JSON
    output       TEXT,               -- JSON
    error        TEXT,
    started_at   INTEGER,
    completed_at INTEGER,
    created_at   INTEGER NOT NULL
);
CREATE INDEX idx_tasks_status ON tasks(status);
CREATE INDEX idx_tasks_workflow ON tasks(workflow_id);

CREATE TABLE episodes (
    id        INTEGER PRIMARY KEY AUTOINCREMENT,
    task_id   TEXT NOT NULL REFERENCES tasks(id),
    type      TEXT NOT NULL CHECK(type IN ('observation','action','result','reflection')),
    content   TEXT NOT NULL,         -- JSON
    timestamp INTEGER NOT NULL
);
CREATE INDEX idx_episodes_task ON episodes(task_id);

CREATE TABLE memories (
    id           INTEGER PRIMARY KEY AUTOINCREMENT,
    category     TEXT NOT NULL CHECK(category IN ('preference','knowledge','pattern','skill')),
    content      TEXT NOT NULL,
    embedding    BLOB,               -- f32 vector
    importance   REAL DEFAULT 0.5,
    access_count INTEGER DEFAULT 0,
    created_at   INTEGER NOT NULL,
    updated_at   INTEGER NOT NULL
);

CREATE TABLE adapters (
    id          TEXT PRIMARY KEY,
    type        TEXT NOT NULL,
    config      TEXT,                -- JSON (non-sensitive only)
    status      TEXT DEFAULT 'disconnected',
    last_health INTEGER,
    created_at  INTEGER NOT NULL
);
```

### Vault Database (data/vault.db)

```sql
CREATE TABLE credentials (
    provider   TEXT PRIMARY KEY,
    type       TEXT NOT NULL CHECK(type IN ('oauth','api_key','cookie','keychain')),
    data       BLOB NOT NULL,        -- AES-256-GCM encrypted
    scopes     TEXT,
    user_label TEXT,
    expires_at INTEGER,
    created_at INTEGER NOT NULL,
    updated_at INTEGER NOT NULL
);

CREATE TABLE policies (
    id         INTEGER PRIMARY KEY AUTOINCREMENT,
    provider   TEXT NOT NULL,
    action     TEXT NOT NULL,
    resource   TEXT NOT NULL DEFAULT '*',
    decision   TEXT NOT NULL CHECK(decision IN ('allow','confirm','deny')),
    rate_limit INTEGER,
    created_at INTEGER NOT NULL
);

CREATE TABLE audit_log (
    id        INTEGER PRIMARY KEY AUTOINCREMENT,
    provider  TEXT NOT NULL,
    action    TEXT NOT NULL,
    resource  TEXT,
    decision  TEXT NOT NULL,
    detail    TEXT,
    timestamp INTEGER NOT NULL
);
CREATE INDEX idx_audit_timestamp ON audit_log(timestamp);
```

---

## 7. Performance Targets

| Scenario                           | Target     | Technique                            |
|------------------------------------|------------|--------------------------------------|
| Launcher popup (⌘+Space)          | < 100ms    | iced GPU, tray resident              |
| Simple command ("open Feishu")     | < 5ms      | SIMD router, no LLM                 |
| Daily task ("send msg to X")       | < 50ms     | Local NER + direct adapter call      |
| Medium task ("summarize emails")   | < 2s       | Haiku streaming + speculative exec   |
| Complex task ("analyze code")      | < 5s       | Opus + parallel tool calls           |
| Cold start                         | < 200ms    | Single binary, connection warmup     |
| Memory footprint                   | < 15MB     | Rust zero-overhead + mmap            |
| Binary size                        | < 10MB     | Static link, LTO, strip              |
| Intent routing (L1)                | < 0.01ms   | aho-corasick SIMD                    |
| SQLite read (hot)                  | < 0.05μs   | moka cache                           |
| SQLite read (warm)                 | < 5μs      | WAL + mmap                           |
| IPC message                        | < 0.001ms  | rkyv zero-copy                       |

---

## 8. Security Model

1. **Identity first** — Who can talk to the OS
2. **Scope next** — What the agent can do per-service
3. **Model last** — Assume LLM can be manipulated; limit blast radius

- All credentials AES-256-GCM encrypted at rest
- Master key from OS keychain (never in files)
- Wasm sandbox for all third-party plugins
- Per-action policies: allow / confirm / deny
- Full audit log of every external action
- No telemetry, fully self-hosted

---

## 9. Roadmap

### Phase 0: Foundation (Week 1-2)
- [x] Project planning and architecture
- [ ] Cargo workspace with all crates scaffolded
- [ ] Core types and error handling
- [ ] SQLite setup with migrations
- [ ] Basic kernel (scheduler, IPC bus)
- [ ] Tracing/logging infrastructure

### Phase 1: Core Loop (Week 3-4)
- [ ] LLM client (Anthropic streaming SSE)
- [ ] Agent ReAct loop
- [ ] Intent parser (via LLM)
- [ ] Filesystem adapter + Shell adapter
- [ ] CLI entry point with basic REPL
- [ ] TUI interface (ratatui)

### Phase 2: Auth & Storage (Week 5-6)
- [ ] Auth Vault (ring encryption + keychain)
- [ ] Policy engine
- [ ] 3-layer memory system
- [ ] Headless auth engine (chromiumoxide CDP)
- [ ] Browser adapter
- [ ] Email adapter (IMAP/SMTP)

### Phase 3: Intelligence (Week 7-8)
- [ ] SIMD intent router (aho-corasick)
- [ ] Local ONNX inference (classifier, NER, embedder)
- [ ] Speculative execution
- [ ] Workflow persistence + cron triggers
- [ ] HTTP/REST generic adapter
- [ ] GitHub adapter

### Phase 4: Desktop (Week 9-10)
- [ ] iced desktop UI
- [ ] Global hotkey launcher
- [ ] System tray
- [ ] Account management panel
- [ ] Feishu adapter
- [ ] Calendar adapter

### Phase 5: Ecosystem & Polish (Week 11-12)
- [ ] Wasm plugin sandbox (wasmtime)
- [ ] MCP protocol compatibility
- [ ] Web UI (axum + SSR)
- [ ] Multi-user support
- [ ] Performance profiling & optimization
- [ ] Documentation & binary distribution

---

## 10. MVP Definition (Phase 0 + 1)

Core value loop:
```
User types intent → System understands → Executes with tools → Returns result
```

MVP deliverables:
1. **Kernel**: Scheduler + IPC + service registry
2. **Agent**: ReAct loop with Claude API
3. **Vault**: API key encrypted storage
4. **Adapters**: Filesystem + Shell
5. **Interface**: CLI REPL + TUI (ratatui)
6. **Storage**: SQLite with episodic memory

Single binary, runs anywhere, < 10MB.
