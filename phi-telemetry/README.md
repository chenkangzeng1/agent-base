# phi-telemetry

Observability layer for [phi-agent](https://github.com/hibuka-labs/phi-agent) — collects and persists structured session metrics without touching the agent runtime hot path.

## How it works

```
agent-base (TurnContext + on_turn_end hook)   ← raw data only
    ↑
phi-telemetry (collector + storage)           ← this crate
    ↑
phi-agent / phi-bard / consumers              ← just wire it in
```

The observer runs in an independent tokio task, connected to the agent via an mpsc channel. A panic in the observer **never** affects the agent.

## Quick start

```rust
use phi_telemetry::{init_telemetry, save_metrics};

// Wire telemetry into the agent
let mut handle = init_telemetry(
    agent.runtime(),
    session_id,
    node_id,
    model_name,
);

// ... agent runs, metrics accumulate automatically ...

// Shut down and persist
handle.shutdown().await;
let session = handle.session.read().await;
let mut session = session.clone();
session.finalize(phi_telemetry::SessionOutcome::Completed);
save_metrics(&session, &session_dir)?;
```

## Output

Each session produces a `session_metrics.json` alongside existing session files:

```
~/.phi-agent/sessions/<id>/
├── turn_001.jsonl           ← event stream
├── session_meta.json         ← session metadata
├── session.log               ← tracing log
└── session_metrics.json      ← structured metrics (this crate)
```

See [observability-design.md](../phi-agent/docs/observability-design.md) for the full JSON schema.

## CLI

```bash
phi metrics list      # table of all sessions
phi metrics show <id> # detail for one session
phi metrics last      # most recent session
```

## Env vars

| Variable | Default | Description |
|----------|---------|-------------|
| `PHI_NODE_ID` | `""` | Node label (e.g. `ecs-bard-writer`) |
| `PHI_METRICS_ENABLED` | `true` | Set to `false`/`0`/`no`/`off` to disable |
| `PHI_COST_PER_1K_TOKENS` | `""` | Custom model pricing (Phase 2) |

## Contributing

Issues and PRs welcome. See [CONTRIBUTING.md](../phi-agent/CONTRIBUTING.md) in the parent repo.

## Security

Report vulnerabilities to **phiagent@hibuka.com**. Do NOT open a public issue. See [SECURITY.md](../phi-agent/SECURITY.md) for details.

## License

Apache 2.0 — see [LICENSE](LICENSE).
