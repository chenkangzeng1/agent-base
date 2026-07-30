# Changelog

All notable changes to phi-telemetry.

## [0.1.0] — 2026-07-30

Initial release.

### Added

- `TurnMetrics` / `SessionMetrics` / `SessionSummary` types with serde support
- `init_telemetry()` — channel-isolated observer via `on_turn_end` hook
- `ObserverHandle` with async `shutdown()` for graceful finalization
- `save_metrics()` / `load_metrics()` / `try_load_metrics()` / `list_all_metrics()` — JSON persistence
- `PHI_NODE_ID` / `PHI_METRICS_ENABLED` / `PHI_COST_PER_1K_TOKENS` env var support
- `phi metrics list` / `show` / `last` CLI commands
- Apache 2.0 license
