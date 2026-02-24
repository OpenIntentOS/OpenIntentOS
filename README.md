# OpenIntentOS

**An AI-native micro operating system built in Rust.**

OpenIntentOS runs on any device, connects to large language models, manages account authorizations, and autonomously operates daily workflows — all from a single ~10MB binary.

## What is this?

OpenIntentOS is an **intent-driven AI operating system**. Instead of clicking through apps and filling forms, you describe what you want in natural language, and the system handles the rest.

```
> Summarize my unread emails and post the summary to the team Feishu group

✓ Connected to email (IMAP)
✓ Found 12 unread emails
✓ Generated summary via Claude
✓ Posted to "Product Team" group on Feishu
```

## Why Rust?

Every microsecond matters. OpenIntentOS is built entirely in Rust for extreme performance:

| Metric | OpenIntentOS | Typical agent frameworks |
|--------|-------------|-------------------------|
| Cold start | < 200ms | 3-5s |
| Simple command | < 5ms | 50-200ms |
| Memory | < 15MB | 200-500MB |
| Binary size | ~10MB | 200MB+ (with runtime) |
| IPC latency | < 1μs | 0.1-1ms |

## Architecture

```
┌─────────────────────────────────────────────┐
│  Intent Interface (NL / GUI / API / Cron)   │
├─────────────────────────────────────────────┤
│  Agent Runtime (ReAct loop, LLM routing)    │
├─────────────────────────────────────────────┤
│  Micro-Kernel (scheduler, IPC, auth vault)  │
├─────────────────────────────────────────────┤
│  Adapters (filesystem, shell, browser, ...) │
├─────────────────────────────────────────────┤
│  Storage (SQLite WAL+mmap, vector search)   │
└─────────────────────────────────────────────┘
```

## Quick Start

### Prerequisites

- Rust 1.84+ (`curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh`)
- An LLM API key (Anthropic Claude recommended)

### Build & Run

```bash
# Build
cargo build --release

# Run (CLI mode)
./target/release/openintent-cli

# Or run directly
cargo run --release --bin openintent-cli
```

### Configuration

```bash
# Set your API key
export ANTHROPIC_API_KEY=sk-ant-...

# Or use the built-in setup
./target/release/openintent-cli --setup
```

## Features

- **Intent Engine**: Natural language → structured actions
- **AI Agent**: ReAct loop with tool calling and reflection
- **Auth Vault**: AES-256-GCM encrypted credential storage with OS keychain
- **Headless Auth**: AI-driven OAuth flows — no browser popups
- **3-Layer Memory**: Working → Episodic → Semantic memory
- **SIMD Router**: aho-corasick pattern matching at ~2GB/s
- **Adapters**: Filesystem, Shell, Browser (CDP), Email, GitHub, Feishu, ...
- **Triple UI**: Native GPU desktop (iced) + Web (axum) + Terminal (ratatui)
- **Plugin Sandbox**: Wasm (wasmtime) for third-party extensions
- **Single Binary**: ~10MB, runs on macOS/Linux/Windows/ARM

## Project Structure

See [PLAN.md](PLAN.md) for the full architecture document.

## License

MIT

## Contributing

OpenIntentOS is in active development. See [CLAUDE.md](CLAUDE.md) for development guidelines.
