# phi-tools Design

## Purpose

`phi-tools` is a standalone tool crate providing a set of general-purpose Agent tool implementations. Each tool implements the `agent_base::Tool` trait and can be registered on demand by any consumer (CLI, Web, CI).

## Principles

- **One file per tool** — each `.rs` exports one struct + Tool impl
- **Zero consumer assumptions** — no dependency on CLI/Web/any specific environment
- **Configurable with defaults** — pass config at construction time; sensible defaults otherwise
- **No interdependency** — tools are independent; registration order doesn't matter
- **Minimal dependencies** — only `agent-base` + `tokio`

## Directory Structure

```
phi-tools/
├── Cargo.toml
└── src/
    ├── lib.rs              # Unified export of all tools
    ├── local_shell.rs      # Local shell commands
    ├── file_read.rs        # Read file (future)
    ├── file_write.rs       # Write file (future)
    ├── file_list.rs        # List directory (future)
    ├── git.rs              # Git operations (future)
    └── http.rs             # HTTP requests (future)
```

## Tool List

### Phase 1 (current)

| Tool | name | Description |
|------|------|-------------|
| `LocalShellTool` | `execute_command` | Execute commands via `sh -c`, with timeout/working directory |

### Phase 2 (future, as needed)

| Tool | name | Description |
|------|------|-------------|
| `FileReadTool` | `read_file` | Read file, size-limited, optional line range |
| `FileWriteTool` | `write_file` | Write file, optional overwrite/append |
| `FileListTool` | `list_directory` | List directory, optional recursion/filtering |
| `GitTool` | `git` | git status/log/diff/branch; read-only by default |

## Cargo.toml

```toml
[package]
name = "phi-tools"
version = "0.1.0"
edition = "2024"
description = "General-purpose Agent toolset — local shell, file operations, Git, etc."

[dependencies]
agent-base = { path = "../agent-base" }
async-trait = "0.1"
serde_json = "1"
tokio = { version = "1", features = ["process", "time", "fs"] }
tracing = "0.1"
```

## lib.rs

```rust
//! phi-tools: General-purpose Agent toolset
//!
//! Each tool independently implements the agent_base::Tool trait.
//! Consumers register tools with AgentBuilder on demand.

pub mod local_shell;

pub use local_shell::LocalShellTool;
// pub use file_read::FileReadTool;     // Phase 2
// pub use file_write::FileWriteTool;   // Phase 2
```

## Consumer Usage

```rust
// CLI
use phi_tools::LocalShellTool;
builder.register_tool(LocalShellTool::new(30_000));

// Web
builder.register_tool(LocalShellTool::new(10_000));
```

## Relationship with phi-agent

```
agent-base        ← Tool trait definition
    ↑
phi-tools         ← Tool implementations (only depends on agent-base)
    ↑
phi-agent (lib)   ← Framework (does not depend on phi-tools)
    ↑
forge-cli (bin)   ← Consumer: depends on both phi-agent + phi-tools
web-server        ← Consumer: depends on both phi-agent + phi-tools
```

`phi-agent` itself does not depend on `phi-tools`, remaining tool-agnostic. Consumers pull in both and assemble as needed.
