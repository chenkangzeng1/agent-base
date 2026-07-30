# phi-telemetry

[phi-agent](https://github.com/hibuka-labs/phi-agent) 的可观测层 — 采集并持久化结构化 session 指标，不侵入 agent 运行时热路径。

## 原理

```
agent-base (TurnContext + on_turn_end hook)   ← 只暴露原始数据
    ↑
phi-telemetry (collector + storage)           ← 本 crate
    ↑
phi-agent / phi-bard / 消费者                  ← 接入即可用
```

observer 运行在独立 tokio task 中，通过 mpsc channel 与 agent 通信。observer panic **不会**影响 agent。

## 快速开始

```rust
use phi_telemetry::{init_telemetry, save_metrics};

// 接入 telemetry
let mut handle = init_telemetry(
    agent.runtime(),
    session_id,
    node_id,
    model_name,
);

// ... agent 运行，metrics 自动采集 ...

// 关闭并持久化
handle.shutdown().await;
let session = handle.session.read().await;
let mut session = session.clone();
session.finalize(phi_telemetry::SessionOutcome::Completed);
save_metrics(&session, &session_dir)?;
```

## 输出

每个 session 在现有文件旁生成 `session_metrics.json`：

```
~/.phi-agent/sessions/<id>/
├── turn_001.jsonl           ← 事件流
├── session_meta.json         ← session 元信息
├── session.log               ← tracing 日志
└── session_metrics.json      ← 结构化指标（本 crate）
```

完整 JSON schema 见 [observability-design.md](../phi-agent/docs/observability-design.md)。

## CLI

```bash
phi metrics list      # 列出所有 session
phi metrics show <id> # 查看某个 session 详情
phi metrics last      # 最近一个 session
```

## 环境变量

| 变量 | 默认值 | 说明 |
|------|--------|------|
| `PHI_NODE_ID` | `""` | 节点标识，如 `ecs-bard-writer` |
| `PHI_METRICS_ENABLED` | `true` | 设为 `false`/`0`/`no`/`off` 关闭 |
| `PHI_COST_PER_1K_TOKENS` | `""` | 自定义模型定价（Phase 2） |

## 参与贡献

欢迎提 Issue 和 PR。详见父仓库的 [CONTRIBUTING.md](../phi-agent/CONTRIBUTING.md)。

## 安全

发现漏洞请发送邮件至 **phiagent@hibuka.com**，不要公开提 Issue。详见 [SECURITY.md](../phi-agent/SECURITY.md)。

## 许可

Apache 2.0 — 见 [LICENSE](LICENSE)。
