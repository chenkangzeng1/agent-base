# phi-agent

**General-purpose AI Agent Framework in Rust — simple, pure, predictable.**

<div style="text-align: center; margin: 3em 0;">

[Get Started](./en/guide/getting-started.md) · [快速开始](./zh/guide/getting-started.md)

</div>

---

## What is phi-agent?

phi-agent is a Rust framework for building AI agents. It provides the infrastructure — builder factory, renderers, configuration, session management — but **does not bundle any tools**. You bring your own tools, and keep full control.

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

## Documentation

| | EN | 中文 |
|------|----|------|
| Quick Start | [EN](./en/guide/getting-started.md) | [中文](./zh/guide/getting-started.md) |
| Custom Tools | [EN](./en/guide/custom-tool.md) | [中文](./zh/guide/custom-tool.md) |
| Configuration | [EN](./en/guide/configuration.md) | [中文](./zh/guide/configuration.md) |
| Focus | [EN](./en/guide/focus.md) | [中文](./zh/guide/focus.md) |
| Advanced | [EN](./en/guide/advanced.md) | [中文](./zh/guide/advanced.md) |

## Links

- [GitHub](https://github.com/hibuka-labs/phi-agent)
- [crates.io](https://crates.io/crates/phi-agent)
- [API Reference (docs.rs)](https://docs.rs/phi-agent)
