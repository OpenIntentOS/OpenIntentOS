# Changelog

## [0.1.5] - 2026-02-26
- feat: auto-generate CHANGELOG.md on every release

## [0.1.4] - 2026-02-26
- feat: auto-release on push and bot self-upgrade command
- refactor: replace hardcoded upgrade keywords with LLM tool
- fix: self-contained auto-release, no PAT required
- fix: replace Python heredoc with perl in auto-release workflow

## [0.1.3] - 2026-02-26
- fix: cfg(unix) guard on unix PermissionsExt import for Windows build

## [0.1.2] - 2026-02-26
- fix: restore Cargo.lock and remove broken lock update from workflow

## [0.1.1] - 2026-02-26
- fix: escape dots in version regex for Cargo.lock update

## [0.1.0] - 2026-02-26
- feat: zero-config install — setup wizard replaces terminal prompts
- feat: add zero-config first-run setup wizard
- feat: support fully silent install via pre-set environment variables
- feat: onboarding, daily briefing, multi-agent orchestrator, self-update
- ux: replace 7-question setup with smart AI provider menu
- fix: improve installer UX — hide API key input, fix PATH, extend timeout
- fix: drop armv7 prebuilt binary — cranelift JIT does not support ARMv7
- fix: replace native-tls with rustls in email adapter
- fix: use cross for ARM Linux builds to resolve OpenSSL cross-compile
- feat: add Telegram group chat configuration tools
- feat: add cascading provider failover and Google Gemini support
- feat: add configuration management system with hot-reloading
- feat: add web_research compound tool for deep search with auto-fetch
- feat: add Readability extraction, Perplexity search, moka cache, SSRF guard
- feat: add evolution engine, Telegram/Discord adapters, skills, and refactor CLI
- feat: add iced desktop UI, Feishu/Calendar adapters, Wasm sandbox, MCP server
- feat: add auth engine, browser/email adapters, macOS keychain, workflow wiring
- feat: add web server, CLI agent REPL, and deployment configs
- feat: implement development task management system
- feat: add workflow persistence, cron scheduler, GitHub adapter, parallel executor
- feat: add session persistence, web adapters, memory, cron, and compaction
- fix: improve agent intelligence with rich system prompt architecture
- fix: improve intent recognition, response quality, and tool selection
