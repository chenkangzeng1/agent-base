# phi-tools

[![Crates.io](https://img.shields.io/crates/v/phi-tools.svg)](https://crates.io/crates/phi-tools)
[![License: MIT](https://img.shields.io/badge/License-MIT-yellow.svg)](https://opensource.org/licenses/MIT)

通用 Agent 工具集，基于 [agent-base](https://crates.io/crates/agent-base) 运行时。

每个工具独立实现 `agent_base::Tool` trait，消费方按需注册到 `AgentBuilder`。

## 工具列表

| 工具 | 名称 | 说明 |
|------|------|------|
| `LocalShellTool` | `execute_command` | 通过 `sh -c` 执行 Shell 命令，支持超时和取消 |

## 用法

```rust
use phi_tools::LocalShellTool;

builder.register_tool(LocalShellTool::new(30_000));
```

## License

MIT
