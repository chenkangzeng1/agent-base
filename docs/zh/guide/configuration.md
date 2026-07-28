# 配置详解

所有配置项及其设置方式。

## LLM 配置

环境变量（`.env` 文件或系统环境变量）：

| 变量 | 说明 | 默认值 |
|------|------|--------|
| `LLM_API_KEY` | API Key（必填） | — |
| `LLM_MODEL` | 模型名称 | `copilot` |
| `LLM_BASE_URL` | API 端点地址 | `https://api.openai.com/v1` |

也支持 `OPENAI_API_KEY`、`OPENAI_MODEL`、`OPENAI_BASE_URL` 作为备选变量名。

### 优先级

```
CLI 参数 > 环境变量 > .env 文件 > 默认值
```

## Agent 配置

`PhiAgentConfig` 字段说明：

| 字段 | 类型 | 说明 | 默认值 |
|------|------|------|--------|
| `model` | `String` | 传递给 LLM 提供商的模型名 | — |
| `enable_thinking` | `bool` | 启用思维链推理 | `true` |
| `thinking_budget` | `Option<u64>` | 思维过程 token 预算 | `None`（使用提供商默认） |
| `thinking_effort` | `ReasoningEffort` | Low / Medium / High / XHigh | `Medium` |
| `safety` | `SafetyConfig` | 工具调用限制 | 见下方 |

## Safety 配置

```rust
SafetyConfig {
    max_tool_calls_per_turn: 30,   // 每轮最大工具调用次数
    max_consecutive_failures: 3,   // 连续失败 N 次后停止
}
```

## CLI 参数 (forge)

| 参数 | 说明 |
|------|------|
| `--format <fmt>` | 输出格式：`terminal`、`json`、`quiet` |
| `--model <name>` | 覆盖模型名称 |
| `--base-url <url>` | 覆盖 API 端点地址 |
| `--no-thinking` | 关闭思维链推理 |
| `--thinking-budget <n>` | 思维过程 token 预算 |
| `--thinking-effort <level>` | `low` / `medium` / `high` / `xhigh` |
| `--max-tool-calls <n>` | 每轮最大工具调用次数 |
| `--max-failures <n>` | 最大连续失败次数 |
| `-y` / `--auto-approve` | 自动批准所有工具调用 |
| `--session-id <id>` | 指定会话 ID |
| `--shell-timeout-ms <ms>` | Shell 命令超时时间 |

## 输出格式

| 格式 | CLI 参数 | 适用场景 |
|------|----------|----------|
| Terminal | `--format terminal`（默认） | 人机交互 — 颜色、emoji、流式输出 |
| JSON | `--format json` | 脚本 / IDE 集成 — 每行一个 JSON |
| Quiet | `--format quiet` | Web 后端 — 无标准输出，仅 tracing |

## 会话目录

会话数据存储在 `~/.phi-agent/sessions/<session_id>/`：

```
session_id           # 会话 ID 标记
session.lock         # 文件锁（防止并发访问）
session_meta.json    # 创建时间、最后活跃时间
session.log          # 可读日志（如启用）
turn_001.jsonl       # 每轮事件日志
turn_002.jsonl
...
```

不活跃超过 7 天的会话会在启动时自动清理。
