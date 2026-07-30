# 架构设计

phi-agent 与依赖 crate 之间的关系，以及关键设计决策。

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

每个 crate 都是 [hibuka-labs](https://github.com/hibuka-labs) 下的独立仓库。

## 各 Crate 职责

### agent-base
运行时内核：
- `AgentRuntime` — 核心事件循环（LLM 对话 → 工具调用 → 循环）
- `Tool` trait — 所有工具实现的接口
- `LlmClient` trait — LLM 提供商的抽象层
- `RuntimeEvent` — 每轮对话中发出的所有事件
- `AgentBuilder` — 组装 Agent 的构建器模式

### agent-works
基于 agent-base：
- **MCP** — Model Context Protocol 支持
- **Skills** — 插件/技能系统
- **Focus** — 带类型的结构化 LLM 调用

### phi-agent（本 crate）
框架层 — 仅提供基础设施，不含工具：
- `base_agent_builder()` — 预配置的构建器工厂
- `PhiAgent` — `AgentRuntime` 的高级封装
- `EventRenderer` — 终端 / JSON / 静默三种输出格式
- 配置解析、会话管理、系统提示词

### phi-tools
工具实现。`master` 分支：`LocalShellTool`。其他分支有更多工具。

### phi（二进制）
CLI 消费者。串联所有组件：创建 `OpenAiClient`、注册工具、运行 REPL 或单次执行。

## 关键设计决策

### 不内置工具
phi-agent 不了解任何具体工具。工具通过 `AgentBuilder::register_tool()` 外部注册。框架保持精简，消费者完全可控。

### 不内置记忆
没有向量数据库、没有嵌入存储、没有隐藏状态。每一个决策都可以追溯到 prompt 中的内容。

### OpenAI 兼容 CLI
CLI 使用 `OpenAiClient`。如需 Anthropic，替换为 `AnthropicClient` 即可 — 框架本身同时支持两者。

### 会话隔离
每个会话有独立的目录和文件锁，防止多进程并发访问。详见 [高级用法](advanced.md)。
