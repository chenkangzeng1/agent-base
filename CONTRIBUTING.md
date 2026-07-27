# Contributing to phi-agent

Thanks for your interest in contributing! This guide will help you get set up.

## Development environment

### Prerequisites

- Rust toolchain (stable, edition 2024): [rustup.rs](https://rustup.rs)
- An LLM API key (OpenAI-compatible) for testing the agent

### Clone and build

Since phi-agent depends on sibling crates that live in separate repos, clone them alongside:

```bash
git clone https://github.com/hibuka-labs/phi-agent.git
cd phi-agent

# Clone dependencies into sibling directories
cd ..
for repo in agent-base agent-works phi-tools log-core; do
  git clone "https://github.com/hibuka-labs/$repo.git"
done
cd phi-agent

# Build
cargo build

# Run tests
cargo test

# Run the CLI
cargo run -- "Hello, world!"
```

### Before submitting a PR

```bash
cargo fmt --all --check    # Formatting
cargo clippy --all-targets -- -D warnings  # Linting
cargo build                # Compilation
cargo test                 # Tests
cargo doc --no-deps        # Documentation
```

All checks must pass. CI runs these automatically on every PR.

## Project structure

See [design.md](docs/design.md) for the architecture overview.

Key principle: **phi-agent does not bundle any tools.** Tool implementations live in `phi-tools`. The framework provides infrastructure only.

## Commit conventions

- Use present tense, imperative mood: "add feature" not "added feature"
- Keep commits focused — one logical change per commit
- Reference issues with `#123` when applicable

## Pull request process

1. Fork the repo and create a feature branch from `master`
2. Make your changes, with tests if applicable
3. Ensure CI is green (`cargo fmt`, `cargo clippy`, `cargo test`)
4. Update `CHANGELOG.md` under `[Unreleased]`
5. Open a PR with a clear description of what and why

## Questions?

Open a [GitHub Discussion](https://github.com/hibuka-labs/phi-agent/discussions) or an issue.
