# Contributing to agent-base

Thank you for your interest in contributing!

## What We Welcome

- **Bug fixes** — typos, edge cases, error handling improvements
- **New LLM providers** — Google, Mistral, Cohere, Groq, etc.
- **Documentation** — doc comments, examples, README improvements
- **Tests** — more edge cases, stress tests, mock scenarios
- **Performance** — obvious optimizations with benchmarks

## What We Usually Decline

- **Business-specific tools** — SSH, database, browser tools belong in upper-layer crates (e.g., `ops-agent`)
- **Workflow engines** — DAG, planner/executor frameworks are out of scope for this kernel
- **Memory/RAG frameworks** — agent-base is a runtime kernel, not a memory platform
- **Major API overhauls without discussion** — please open an issue first

## Getting Started

```bash
# Clone and build
git clone https://github.com/chenkangzeng1/agent-base.git
cd agent-base
cargo build

# Run tests
cargo test

# Check formatting
cargo fmt --check

# Run clippy
cargo clippy --all-targets
```

## Pull Request Guidelines

1. Keep changes focused. One PR = one concern.
2. Add tests for new functionality.
3. Ensure `cargo test` passes.
4. Run `cargo fmt` before committing.
5. Keep the `Cargo.toml` dependency list minimal.

## Code of Conduct

Be respectful. That's it.
