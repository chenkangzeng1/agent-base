# agent-core

轻量 Agent Runtime Kernel。提供最小必要的会话执行编排能力，不承载复杂 workflow、memory 平台和强业务恢复策略。

## 设计原则

- **语义清晰** — 运行结果通过 `RunOutcome` 显式表达，事件与返回值不冲突
- **状态简单** — runtime 内存态是 session 的 source of truth，store 是 persistence adapter
- **默认保守** — 内核默认行为保守，工具失败后默认停止，不做业务恢复猜测
- **策略外置** — 可变行为通过 trait / policy / middleware 注入，不固化在 runtime 中

## 能力

- **LLM 对话** — `LlmClient` trait，内置 OpenAI / Anthropic 实现
- **工具系统** — `Tool` trait + `ToolRegistry`，注册与自动分发
- **审批机制** — `ApprovalHandler` trait，支持 AllowOnce / AllowAlways / Deny
- **错误恢复** — `ToolErrorRecovery` trait，默认 `StopOnError`，可选 `RetryOnError`
- **事件流** — 执行过程以 `AgentEvent` 结构化事件外抛
- **多轮会话** — `AgentSession` 管理消息历史，`SessionStore` 负责可选持久化
- **子 Agent** — `SubAgentTool` 支持委托子 Agent 执行，默认 ephemeral session
- **运行结果** — `RunOutcome` 区分 `Completed` / `Failed`，消除语义歧义

## 用法

```rust
use std::sync::Arc;
use agent_core::{
    AgentBuilder, AgentEvent, AgentResult, RunOutcome,
    OpenAiClient, Tool, ToolContext, ToolOutput, ToolControlFlow,
};
use serde_json::{json, Value};

// 1. 定义工具
struct GreetTool;
// impl Tool for GreetTool { ... }

// 2. 创建 runtime
let client = Arc::new(OpenAiClient::new("sk-xxx".into(), "gpt-4o".into(), None));
let mut runtime = AgentBuilder::new(client)
    .system_prompt("你是一个助手")
    .register_tool(GreetTool)
    .build();

// 3. 执行一轮对话
let session_id = runtime.create_session();
let (events, outcome) = runtime.run_turn_stream(session_id, "你好").await?;
assert_eq!(outcome, RunOutcome::Completed);
```

## 运行示例

```bash
# 1. 配置 API Key
cp .env.example .env
# 编辑 .env 填入 OPENAI_API_KEY 或 ANTHROPIC_API_KEY

# 2. 运行 REPL
cargo run --example repl
```

## 不做什么

- ❌ 不含 SSH / 文件 / 数据库等业务工具
- ❌ 不含复杂 workflow DAG / multi-agent 调度
- ❌ 不含 memory framework
- ❌ 不含终端 UI、审批弹窗
- ❌ 不含重型持久化一致性系统

## 典型分层

```
ops-agent / db-agent / browser-agent    ← 业务 agent
    └── agent-core                       ← 轻量运行时 Kernel
```

## v1 语义约定

- `run_turn_*` 返回 `AgentResult<RunOutcome>` — `Ok(Completed)` 表示成功完成，`Ok(Failed)` 表示运行结束但未成功
- `AgentEvent::RunFinished` 只表示过程结束 — 最终状态见 `RunOutcome`
- 工具失败默认停止运行 — 需要自动恢复时注入 `RetryOnError`
- 子 Agent 默认每次调用创建新 session — 需复用上下文时使用 `with_persistent`
- Session live state 在 runtime 内存 — `SessionStore` 是可选持久化适配层
