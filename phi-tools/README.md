[![Crates.io](https://img.shields.io/crates/v/phi-tools.svg)](https://crates.io/crates/phi-tools)
[![License: MIT](https://img.shields.io/badge/License-MIT-yellow.svg)](https://opensource.org/licenses/MIT)

General-purpose Agent toolset for the [agent-base](https://crates.io/crates/agent-base) runtime.

Each tool independently implements the `agent_base::Tool` trait. Consumers register tools with `AgentBuilder` on demand.

## Tools

| Tool | Name | Description |
|------|------|-------------|
| `LocalShellTool` | `execute_command` | Execute shell commands via `sh -c`, with timeout and cancellation |

## Usage

```rust
use phi_tools::LocalShellTool;

builder.register_tool(LocalShellTool::new(30_000));
```

## License

MIT

[中文文档](README_CN.md)
