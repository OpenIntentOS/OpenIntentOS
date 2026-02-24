# OpenIntentOS Development Guidelines

## CRITICAL Rules (Highest Priority)

- **No hardcoded multilingual content.** All user-facing strings, failure indicators, prompts, and locale-dependent text MUST be loaded from configuration files (e.g. `config/default.toml`) or resource files — NEVER hardcoded in source code. This applies to ALL languages.
- **Maximum 1000 lines per file.** Any source file exceeding 1000 lines MUST be refactored into smaller modules immediately. No exceptions.

## Language

- All code, comments, commit messages, and documentation MUST be in English.
- Git commits MUST be in English.

## Architecture

- **Full Rust** — no TypeScript, no Node.js, no JavaScript anywhere.
- Hot path performance is critical. Profile before and after changes.
- Every crate is a library crate except `openintent-cli` (binary).

## Crate Dependency Graph

```
openintent-cli (binary)
├── openintent-kernel
├── openintent-agent
│   └── openintent-kernel (for IPC, scheduler)
├── openintent-store
├── openintent-vault
├── openintent-intent
│   └── openintent-agent (for LLM calls)
├── openintent-adapters
│   └── openintent-vault (for auth tokens)
├── openintent-skills (OpenClaw-compatible skill system)
│   └── openintent-adapters (for Adapter trait)
├── openintent-auth-engine
│   └── openintent-vault
├── openintent-ui (iced desktop)
├── openintent-web (axum web)
└── openintent-tui (ratatui terminal)
```

## Coding Standards

### Rust
- Use `thiserror` for library error types, `anyhow` only in binary/CLI.
- Use `tracing` for all logging (never `println!` in library code).
- Prefer `Arc<T>` over `Rc<T>` — everything must be `Send + Sync`.
- Use `#[async_trait]` from `async-trait` crate for async trait methods.
- Follow Rust 2024 edition idioms.
- Run `cargo clippy` and `cargo fmt` before committing.

### Performance Rules
- Zero-copy where possible (`rkyv`, `bytes` crate).
- Prefer stack allocation over heap (use `SmallVec`, `ArrayString` for small data).
- Use `moka` for caching hot data.
- Never block the tokio runtime — use `tokio::task::spawn_blocking` for CPU-heavy work.
- Profile with `cargo flamegraph` for optimization work.

### Error Handling
- Every crate defines its own error type via `thiserror`.
- Use `Result<T, Error>` everywhere, never `unwrap()` or `expect()` in library code.
- `unwrap()` is only acceptable in tests and CLI argument parsing.

### Testing
- Unit tests in the same file (`#[cfg(test)]` module).
- Integration tests in `tests/` directory per crate.
- Use `tokio::test` for async tests.

## Git Workflow

- Branch names: `feat/xxx`, `fix/xxx`, `refactor/xxx`
- Commit messages: imperative mood, < 72 chars first line, English only
- NEVER include `Co-Authored-By`, AI attribution, or any AI-related signatures in commits
- NEVER include any Claude, Anthropic, OpenAI, or other AI tool references in commits, code comments, or documentation
- Do not commit `.claude/` directory or any AI tool configuration

## Key Files

- `PLAN.md` — Full architecture document and roadmap
- `config/default.toml` — Default runtime configuration
- `config/IDENTITY.md` — System persona definition
- `config/SOUL.md` — Behavioral guidelines for the AI agent
