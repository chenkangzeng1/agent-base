# Contributing to agent-works

Thank you for your interest in contributing!

## What We Welcome

- **New built-in tools** — useful general-purpose tools (network, process, etc.)
- **New MCP transports** — additional transport backends beyond HTTP + stdio
- **New SkillPrompters** — alternative prompt injection strategies
- **Documentation** — README improvements, examples, doc comments
- **Tests** — edge cases, integration tests
- **Performance** — obvious optimizations with benchmarks

## What We Usually Decline

- **Business-specific tools** — SSH, database, browser tools belong in upper-layer crates (e.g., `ops-agent`)
- **Heavy dependencies** — keep the dependency tree minimal; each feature should add at most 1–2 crates
- **Breaking changes to Skill/MCP traits without discussion** — please open an issue first

## Getting Started

```bash
git clone https://github.com/chenkangzeng1/agent-works.git
cd agent-works
cargo build --features full
cargo test --features full
cargo fmt --check
cargo clippy --all-targets --features full
```

## Pull Request Guidelines

1. Keep changes focused. One PR = one concern.
2. Gate new modules behind feature flags when appropriate.
3. Add tests for new functionality.
4. Ensure `cargo test --features full` passes.
5. Add an example if the feature is user-facing.
6. Run `cargo fmt` before committing.

## Code of Conduct

Be respectful. That's it.
