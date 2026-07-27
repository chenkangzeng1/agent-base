# Changelog

All notable changes to phi-agent will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

### Added
- 21 browser automation tools via CDP (navigate, click, input, screenshot, snapshot, evaluate, etc.)
- `--enable-browser`, `--headed`, `--connect-ws` CLI flags for browser control
- China/GFW-aware system prompt variant (`build_system_prompt_cn`)

## [0.1.0] - 2025-07-23

### Added
- Initial public release
- `base_agent_builder()` factory with sensible defaults
- `PhiAgent` struct wrapping `AgentRuntime`
- Terminal / JSON stream / Null renderers
- CLI entry point (`phi`) with REPL and one-shot modes
- Session management with file locking and auto-cleanup
- LLM config resolution (CLI > env > .env > default)
- `LocalShellTool` (via phi-tools)

[Unreleased]: https://github.com/hibuka-labs/phi-agent/compare/v0.1.0...HEAD
[0.1.0]: https://github.com/hibuka-labs/phi-agent/releases/tag/v0.1.0
