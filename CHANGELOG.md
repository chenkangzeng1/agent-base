# Changelog

All notable changes to `agent-base` are documented here.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

## [0.5.0] - 2026-09-06

### Added
- Truncation circuit breaker: when a tool-call loop hits the token-truncation
  guard repeatedly, the run is redirected with guidance instead of being killed,
  preventing the re-issue death spiral.
- Partial execution of valid tool_calls when truncation strikes: tool calls
  whose arguments fit are still executed; only the oversized ones are rejected.
- `Content::Detail` — structured tool-result metadata (e.g. multi-agent
  tool-result details) alongside the plain text payload.
- `thinking_bytes` / total thinking byte tracking on `TurnContext` for
  reasoning-token accounting.
- `GuardCtx` now exposes the rejected-tool-call state so downstream guards can
  react to rejections instead of re-judging completion.

### Fixed
- Truncation guard now catches empty (`{}`) arguments for tools whose schema
  requires fields (`args_len = 0` no longer bypasses the check).
- Streaming no longer suppresses text/thought events that arrive after a
  `tool_call` chunk.
- Deduplicated event delivery in parallel orchestration (child-agent events
  could previously be emitted twice).
