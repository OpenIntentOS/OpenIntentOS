# OpenIntentOS

An AI-native micro operating system built entirely in Rust.

OpenIntentOS is an **intent-driven AI operating system**. Users express what they want in natural language, and the system understands, orchestrates tools, manages credentials, and executes autonomously — all from a single ~10MB binary.

```
> Summarize my unread emails and post the summary to the team Feishu group

✓ Connected to email (IMAP)
✓ Found 12 unread emails
✓ Generated summary via Claude
✓ Posted to "Product Team" group on Feishu
```

## Features

- **Intent-Driven**: Express what you want in natural language — the system handles the rest
- **Full Rust**: Zero-cost abstractions, no GC, < 10MB binary, < 200ms cold start
- **10 Adapters**: filesystem, shell, web, email, GitHub, browser, memory, cron, Feishu, calendar
- **Multi-Provider LLM**: Anthropic Claude + OpenAI GPT + any OpenAI-compatible API
- **4 Interfaces**: CLI REPL, TUI (ratatui), Web UI (axum), Desktop (iced)
- **Secure**: AES-256-GCM vault, macOS Keychain, OAuth 2.0 + PKCE, PBKDF2 password hashing
- **Multi-User**: Role-based access control (admin/user/viewer) with secure authentication
- **Extensible**: Wasm plugin sandbox, MCP protocol, workflow engine with cron triggers

## Quick Start

### Prerequisites

- Rust nightly (`curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh`)
- An LLM API key (Anthropic Claude recommended)

### Build & Run

```bash
# Build
cargo build --release

# Setup — stores your API key in the encrypted vault
export ANTHROPIC_API_KEY=sk-ant-...
./target/release/openintent setup

# Run (CLI REPL)
./target/release/openintent run

# Run (TUI — terminal dashboard)
./target/release/openintent tui

# Run (Web UI)
./target/release/openintent serve --port 3000
```

## Architecture

```
┌──────────────────────────────────────────────────────────────────┐
│  Layer 5: Intent Interface                                       │
│  Natural language / Voice / GUI (iced) / API / Cron triggers     │
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
│  └───────┘└───────┘└───────┘└────────┘└─────────┘└──────────┘ │
│  ┌───────┐┌───────┐┌───────┐┌────────┐                        │
│  │Memory ││ Web   ││ Cron  ││ Shell  │  + Wasm plugin adapters │
│  └───────┘└───────┘└───────┘└────────┘                        │
├──────────────────────────────────────────────────────────────────┤
│  Layer 1: Runtime & Storage                                      │
│  tokio │ rusqlite WAL+mmap │ moka cache │ io_uring/kqueue       │
└──────────────────────────────────────────────────────────────────┘
```

## Project Structure

```
crates/
├── openintent-kernel/       Micro-kernel: scheduler, IPC bus, intent router, service registry
├── openintent-agent/        Agent runtime: ReAct loop, planner, executor, multi-provider LLM client
├── openintent-store/        Storage engine: SQLite WAL+mmap, 3-layer memory, user accounts, caching
├── openintent-vault/        Encrypted credential vault: AES-256-GCM, OS keychain, policies
├── openintent-intent/       Intent parsing and persistent workflow engine with triggers
├── openintent-adapters/     Built-in adapters: filesystem, shell, browser, email, GitHub, web, etc.
├── openintent-auth-engine/  Headless OAuth automation: CDP browser, device code flow
├── openintent-ui/           Native desktop UI (iced, GPU-rendered)
├── openintent-web/          Web UI and REST API (axum + SSR templates)
├── openintent-tui/          Terminal UI (ratatui)
└── openintent-cli/          CLI entry point — the single binary
```

## Configuration

| Variable | Description | Required |
|----------|-------------|----------|
| `ANTHROPIC_API_KEY` | Anthropic Claude API key | Yes (or use OpenAI) |
| `OPENAI_API_KEY` | OpenAI GPT API key | Optional |
| `OPENINTENT_MODEL` | Default LLM model (e.g. `claude-sonnet-4-20250514`) | Optional |
| `OPENINTENT_DATA_DIR` | Data directory (default: `./data`) | Optional |
| `OPENINTENT_LOG_LEVEL` | Log level: trace, debug, info, warn, error | Optional |
| `OPENINTENT_WEB_PORT` | Web server port (default: 3000) | Optional |

Additional configuration is available in `config/default.toml`.

## Performance

| Scenario | Target | Technique |
|----------|--------|-----------|
| Cold start | < 200ms | Single binary, connection warmup |
| Simple command | < 5ms | SIMD intent router, no LLM |
| Complex task | < 5s | Streaming LLM + parallel tool calls |
| Memory footprint | < 15MB | Rust zero-overhead + mmap |
| Binary size | < 10MB | LTO, strip, panic=abort |
| SQLite read (hot) | < 0.05us | moka lock-free cache |
| SQLite read (warm) | < 5us | WAL + mmap |
| IPC message | < 1us | rkyv zero-copy deserialization |

## Security

- All credentials encrypted at rest with AES-256-GCM (ring, hardware AES-NI)
- Master encryption key stored in OS keychain (macOS Keychain / Linux Secret Service)
- User passwords hashed with PBKDF2-HMAC-SHA256 (600,000 iterations)
- Per-action permission policies: allow / confirm / deny
- Full audit log of every external action
- Wasm sandbox for third-party plugins
- No telemetry — fully self-hosted

## License

MIT
