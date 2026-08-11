# phi-agent

**General-purpose AI Agent Framework in Rust — simple, pure, predictable.**

---

## What is phi-agent?

phi-agent is a Rust framework for building AI agents. It provides the infrastructure — builder factory, renderers, configuration, session management. Kernel tools (file I/O, shell, multi-agent) are available via `phi-kernel-tools` behind feature flags — all off by default. Application tools are injected by consumers.

## Why phi-agent?

- **Simple** — No hidden state, no vector DB, no magic. Everything is explicit.
- **Pure Rust** — Async, type-safe, zero-cost abstractions. Runs from cloud to edge.
- **Your Rules** — The framework doesn't decide what tools your agent has. You do.

## Architecture

```
agent-base (runtime kernel + Tool trait)
    ↑
agent-works (MCP, Skills, Focus)
    ↑
phi-agent (lib) ← framework, no tools
    ↑
Your App (CLI, web, etc.) ← you register tools here
```

## Links

- [GitHub](https://github.com/hibuka-labs/phi-agent)
- [crates.io](https://crates.io/crates/phi-agent)
- [API Reference (docs.rs)](https://docs.rs/phi-agent)
