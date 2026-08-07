# Contributing to phi-agent

Thanks for your interest in contributing! This guide gets you from clone to PR in minutes.

## Quick Start (5 min)

```bash
git clone https://github.com/hibuka-labs/phi-agent.git
cd phi-agent
cargo build        # pulls dependencies from crates.io automatically
cargo test         # make sure everything passes
```

**That's it.** phi-agent uses pure crates.io dependencies — `cargo build` downloads everything you need. You don't need to clone any other repository for most contributions.

## Finding Something to Work On

- **[Good First Issues](https://github.com/hibuka-labs/phi-agent/labels/good%20first%20issue)** — small, well-scoped tasks ideal for new contributors. Each one explains what to change and how to verify it.
- **[Help Wanted](https://github.com/hibuka-labs/phi-agent/labels/help%20wanted)** — larger features or improvements the maintainers would love help with.
- **Got your own idea?** Open an issue first to discuss it before writing code — saves you time if it doesn't fit the roadmap.

## Features We Won't Accept

To keep phi-agent focused and maintainable, these features are **explicitly out of scope** and PRs for them will not be merged:

| Feature | Reason |
|---------|--------|
| **Built-in business tools** (web search, file I/O, code exec, database query) | phi-agent is tool-agnostic by design. You register what you need via the `Tool` trait. Framework capabilities (memory, multi-agent, skills, MCP) are provided by agent-works as standard tools — these are opt-out, not built-in. |
| **Built-in memory / vector DB** | No embedded vector store. File-based memory is available via agent-works (`memory` feature gate), opt-out with `.without_memory()`. |
| **Prompt templates or chains** | No langchain-style abstractions. Users control their system prompts directly. |
| **Workflow / DAG engine** | Agent behavior is LLM-driven via tool-choice, not graph-compiled. Use LangGraph for orchestration. |
| **Pre-built agent types** (coder, researcher, etc.) | There is no one-size-fits-all agent. Users compose their own from tools + prompts. |
| **HTTP server or REST API** | phi-agent is a library + CLI. The server layer (Axum/Actix/Warp) is left to the user. |
| **Plugin system** | Tools are registered at runtime via the `Tool` trait. No dynamic loading or plugin discovery. |
| **Non-Rust SDKs in this repo** | Python/Node/Go SDKs live in their own repositories. This repo is Rust only. |

If your idea isn't on this list, open a discussion first — we're happy to talk. But if it's listed above, please don't spend time on a PR; it won't be accepted.

## Before Submitting a PR

```bash
cargo fmt --check    # Formatting
cargo clippy --all-targets -- -D warnings   # Linting
cargo test           # Tests
```

All three must pass. CI runs them automatically on every PR, so save yourself a round-trip.

## Pull Request Checklist

1. Create a feature branch from `master`
2. Make your changes, with tests if applicable
3. Run the checks above (`fmt`, `clippy`, `test`)
4. Add an entry to `CHANGELOG.md` under `[Unreleased]`
5. Open a PR with a clear description of **what** and **why**

## Review Timeline

We review PRs within **48 hours**. Small, focused PRs (one logical change) get merged faster. If your PR is large, consider breaking it up.

## Commit Style

- Present tense, imperative mood: "add feature" not "added feature"
- One logical change per commit
- Reference issues with `#123` when applicable

## Advanced: Working Across Crates

If your change requires modifying a sibling crate (agent-base, agent-works, etc.) simultaneously:

```bash
# Clone the dependency alongside phi-agent
cd ..
git clone https://github.com/hibuka-labs/agent-base.git
cd phi-agent

# Temporarily add a path override in Cargo.toml (DO NOT COMMIT):
# agent-base = { version = "0.1.11", path = "../agent-base" }
```

Remove the `path` override before committing. See [CLAUDE.md](CLAUDE.md) for details on the multi-repo workflow.

## Questions?

Open a [GitHub Discussion](https://github.com/hibuka-labs/phi-agent/discussions) or an issue.
