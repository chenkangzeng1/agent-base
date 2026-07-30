# 架构设计

phi-agent 与依赖 crate 之间的关系，以及关键设计决策。

## 仓库

每个 crate 是独立的 git 仓库，发布到 crates.io：

| Crate | 仓库 | crates.io |
|-------|------|-----------|
| `agent-base` | [hibuka-labs/agent-base](https://github.com/hibuka-labs/agent-base) | ✅ |
| `agent-works` | [hibuka-labs/agent-works](https://github.com/hibuka-labs/agent-works) | ✅ |
| `phi-agent` | [hibuka-labs/phi-agent](https://github.com/hibuka-labs/phi-agent)（本仓库） | ✅ |
| `phi-tools` | [hibuka-labs/phi-tools](https://github.com/hibuka-labs/phi-tools) | ✅ |
| `phi-telemetry` | [hibuka-labs/phi-telemetry](https://github.com/hibuka-labs/phi-telemetry) | ✅ |
| `log-core` | [hibuka-labs/log-core](https://github.com/hibuka-labs/log-core) | ✅ |

所有 crate 使用纯版本依赖 `version = "0.1"`，无 path、无 monorepo。
`cargo add phi-agent` 从 crates.io 拉取所需依赖。

## 依赖链

```
agent-base (运行时内核 + Tool trait)
    ↑
agent-works (MCP, Skills, Focus)
    ↑
phi-agent (lib) ← 框架层，不含工具
    ↑
phi (bin) ← CLI，在这里注册工具
```

## 各 Crate 职责

### agent-base
运行时内核 — `cargo add agent-base` 如果只需要引擎：
- `AgentRuntime` — 核心事件循环（LLM 对话 → 工具调用 → 循环）
- `Tool` trait — 所有工具实现的接口
- `LlmClient` trait — LLM 提供商的抽象层
- `RuntimeEvent` — 每轮对话中发出的所有事件
- `AgentBuilder` — 组装 Agent 的构建器模式
- `TurnContext` + `on_turn_end` hook — 可观测性接口

### agent-works
基于 agent-base — `cargo add agent-works` 获取工具箱：
- **MCP** — Model Context Protocol 支持
- **Skills** — 插件/技能系统
- **Focus** — 带类型的结构化 LLM 调用
- **内置工具** — 文件操作（读取、写入、列表等）

### phi-agent
框架层 — `cargo add phi-agent` 获取完整功能：
- `base_agent_builder()` — 预配置的构建器工厂
- `PhiAgent` — `AgentRuntime` 的高级封装
- `EventRenderer` — 终端 / JSON / 静默输出
- 配置解析、会话管理、系统提示词
- `phi` CLI — `cargo install phi-agent`
- `phi init` / `phi init --lib` — 项目脚手架
- `phi metrics` — 会话观测数据查看

## 关键设计决策

### 不内置工具
phi-agent 不了解任何具体工具。工具通过 `AgentBuilder::register_tool()` 外部注册。框架保持精简，消费者完全可控。

### 不内置记忆
没有向量数据库、嵌入存储、隐藏状态。每一个决策都可追溯到 prompt。

### 可观测性默认开启
每个 session 自动写入 `session_metrics.json`。Token 消耗、延迟分布、工具调用统计全部记录。`phi metrics` 查看。详见 [可观测性](observability.md)。

### 会话隔离
每个会话有独立目录和文件锁，防止多进程并发访问。详见 [高级用法](advanced.md)。
