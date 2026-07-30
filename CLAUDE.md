# CLAUDE.md — agent-base

Lightweight Agent Runtime Kernel for building AI agents in Rust.

## Rules

### Dependencies
- `Cargo.toml` uses **pure version deps** (no `path`). The committed state is clean.
- To debug against a local dependency: temporarily add `path`, **DO NOT commit** it.

### Publishing
After making changes to this crate:

1. Bump version in `Cargo.toml`
2. Commit and push to GitHub
3. `cargo publish --registry crates-io`

If you don't bump the version, crates.io rejects the publish (can't overwrite).

### Version bump checklist
When publishing a new version of agent-base, update the dep in:
- [ ] agent-works
- [ ] phi-tools
- [ ] phi-telemetry
- [ ] phi-agent
- [ ] phi-bard (if using agent-base directly)
- [ ] ops/*

### Pre-push
```bash
cargo fmt --check && cargo clippy --all-targets -- -D warnings
```
