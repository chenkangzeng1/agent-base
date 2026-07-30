# 架构设计

phi-agent 与依赖 crate 之间的关系，以及关键设计决策。

## 仓库结构

phi-agent 是 **monorepo** — 所有 crate 都在同一个仓库中：

```
github.com/hibuka-labs/phi-agent/
├── Cargo.toml          ← workspace 根
├── src/                ← phi-agent（库 + CLI 二进制）
├── agent-base/         ← 运行时内核
├── agent-works/        ← 工具生态（MCP、Skills）
├── phi-telemetry/      ← 可观测性（内部）
├── phi-tools/          ← Shell 工具（内部）
└── log-core/           ← 日志基础（内部）
```

三个 crate 发布到 crates.io：`agent-base`、`agent-works`、`phi-agent`。
其余为内部 crate — phi-agent 使用但不作为独立包对外暴露。

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
运行时内核 — 只需要引擎用 `cargo add agent-base`：
- `AgentRuntime` — 核心事件循环（LLM 对话 → 工具调用 → 循环）
- `Tool` trait — 所有工具实现的接口
- `LlmClient` trait — LLM 提供商的抽象层
- `RuntimeEvent` — 每轮对话中发出的所有事件
- `AgentBuilder` — 组装 Agent 的构建器模式
- `TurnContext` + `on_turn_end` hook — 可观测性接口（只暴露数据，不包含 metrics 逻辑）

### agent-works
基于 agent-base — 需要工具箱用 `cargo add agent-works`：
- **MCP** — Model Context Protocol 支持
- **Skills** — 插件/技能系统
- **Focus** — 带类型的结构化 LLM 调用
- **内置工具** — 文件操作（读取、写入、列表等）

### phi-agent（本 crate）
框架层 — 完整功能用 `cargo add phi-agent`：
- `base_agent_builder()` — 预配置的构建器工厂
- `PhiAgent` — `AgentRuntime` 的高级封装
- `EventRenderer` — 终端 / JSON / 静默三种输出格式
- 配置解析、会话管理、系统提示词
- `phi` CLI 二进制 — `cargo install phi-agent`

### 可观测性

phi-agent 自动采集结构化指标。每个 session 都会写入 `session_metrics.json`：

- **每轮**：token、延迟拆解（TTFT、LLM、Tool）、工具调用、结果、思考模式
- **每会话**：汇总、P50/P95/P99 延迟、工具分布、错误率、费用估算
- **业务扩展**：通过 `custom` 字段注入业务数据（如 phi-bard 跟踪 prompt 版本、修改轮次）

```bash
# 内置 CLI
phi metrics list               # 列表查看最近会话
phi metrics show <session_id>  # 详细分解
phi metrics last               # 最新会话
```

```json
// session_metrics.json — 示例
{
  "session_id": "20260729_abc12345",
  "model": "claude-sonnet",
  "total_turns": 5,
  "total_input_tokens": 15000,
  "total_output_tokens": 12000,
  "estimated_cost": 0.18,
  "p50_turn_ms": 32000,
  "p95_turn_ms": 52000,
  "tool_breakdown": { "shell": 5, "check_quality": 3 },
  "outcome": "completed",
  "custom": { "product": "phi-bard", "prompt_version": "v3" }
}
```

**架构**：观测代码运行在独立的 tokio task 中，通过 channel 通信。
观测 panic 不会影响 agent。agent-base 不感知"观测"这个事，只通过 `on_turn_end` hook 暴露 `TurnContext` 数据。

**环境变量**：

| 变量 | 默认值 | 说明 |
|------|--------|------|
| `PHI_METRICS_ENABLED` | `true` | 设为 `false` 禁用指标采集 |
| `PHI_NODE_ID` | `""` | 节点标识，用于多节点部署 |
| `PHI_COST_PER_1K_TOKENS` | 内置 | 自定义模型定价（格式：`输入费用,输出费用` 每千 token） |

完整设计文档见 [observability-design.md](https://github.com/hibuka-labs/phi-agent/blob/master/docs/observability-design.md)。
