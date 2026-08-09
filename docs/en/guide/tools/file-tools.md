# File Tools

phi-agent gives the agent read/write/list access to the filesystem through kernel tools. These are enabled by default (behind the `file` feature flag).

## Available tools

| Tool | Description |
|------|-------------|
| `read_file` | Read a file, with optional offset/limit for paging large files |
| `write_file` | Create or overwrite a file |
| `list_files` | List directory contents, supports glob patterns |

## Design principles

**Path safety**. All paths are resolved relative to the working directory. Parent directory traversal (`..`) is rejected.

**Size limits**. `read_file` defaults to 2000 lines per call (use `offset`/`limit` for large files). `write_file` defaults to 1MB max per write.

**Explicit truncation**. When `write_file` truncates output, the result carries a `... (truncated)` marker so the agent knows the content is incomplete.

## Usage

The file tools are registered automatically by `base_agent_builder()`. No additional setup needed:

```rust
let builder = base_agent_builder(llm_client)
    .system_prompt(build_system_prompt());
// read_file, write_file, list_files are already registered
```

To disable file tools:

```toml
# Cargo.toml
[dependencies]
phi-agent = { version = "0.9", default-features = false, features = ["shell"] }
```

Or at runtime:

```rust
use phi_agent::base_agent_builder;
// Build with --no-default-features equivalent:
// file tools won't be registered if the `file` feature is off
```

## Why file tools matter

File tools are the architectural foundation for Skills and Memory:

```
Without file tools                    With file tools
┌──────────────────┐                 ┌──────────────────┐
│ Agent wants SKILL.md │             │ Agent wants SKILL.md │
│     ↓            │                 │     ↓            │
│ calls get_skill_detail │  ──becomes──▶ │ calls read_file     │
│     ↓            │                 │     ↓            │
│ framework feeds it │               │ framework IS the OS   │
│ (3 dedicated tools) │              │ (1 set of generic     │
└──────────────────┘                 │  tools)              │
                                     └──────────────────┘
```

One set of generic file tools replaces multiple dedicated tools. This is the same pattern used by Claude Code and Codex.
