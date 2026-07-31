# Changelog

All notable changes to phi-agent will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

## [0.2.0] - 2025-07-31

### Added
- `phi init` subcommand to scaffold new phi-agent projects
- `phi metrics` subcommand (list/show/last) for session observability
- 8 user guides (EN + ZH): Quick Start, Custom Tools, CLI Usage, Configuration, Focus, Architecture, Observability, Advanced
- Full i18n documentation site at [docs.phi-agent.dev](https://docs.phi-agent.dev)
- Observability card on homepage — turn logging, metrics, tracing

### Changed
- Refreshed feature cards on homepage and README — simpler, more abstract, more compelling
- Added contact info to homepage and README
- Updated docs links to use docs.phi-agent.dev domain
- Copyright updated to "hibuka labs Contributors"

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

[Unreleased]: https://github.com/hibuka-labs/phi-agent/compare/v0.2.0...HEAD
[0.2.0]: https://github.com/hibuka-labs/phi-agent/compare/v0.1.0...v0.2.0
[0.1.0]: https://github.com/hibuka-labs/phi-agent/releases/tag/v0.1.0
