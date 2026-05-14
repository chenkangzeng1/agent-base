# agent-core

通用 ReAct Agent 运行时框架。零业务假设，提供 AI Agent 的核心编排能力。

## 能力

- **LLM 对话** — `LlmClient` trait，内置 OpenAI / DashScope 实现
- **工具系统** — `Tool` trait + `ToolRegistry`，注册与自动分发
- **审批机制** — `ApprovalHandler` trait，支持 AllowOnce / AllowAlways / Deny
- **事件流** — 执行过程以 `AgentEvent` 结构化事件外抛
- **多轮会话** — `AgentSession` 管理消息历史与状态

## 用法

```rust
use std::sync::Arc;
use agent_core::{
    AgentBuilder, AgentEvent, ApprovalHandler, ApprovalDecision,
    ApprovalRequest, OpenAiClient, Tool, ToolContext, ToolOutput, ToolControlFlow,
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
let events = runtime.run_turn_stream(session_id, "你好").await?;
```

## 运行示例

项目自带一个算术 REPL 示例，在 `.env` 中配置 API Key 后即可运行：

```bash
# 1. 配置 API Key（支持 OpenAI / DashScope）
cp .env.example .env
# 编辑 .env 填入 OPENAI_API_KEY 或 DASHSCOPE_API_KEY

# 2. 运行 REPL
cargo run --example repl
```

进入 REPL 后可直接输入算术题，AI 会调用加减乘除工具计算结果。

## 不做什么

- ❌ 不含 SSH / 文件 / 数据库等业务工具
- ❌ 不含终端 UI、审批弹窗
- ❌ 不含持久化、日志（由上层 `data-core` / `log-core` / `ops-agent` 负责）

## 典型分层

```
ops-agent / db-agent / browser-agent    ← 业务 agent
    └── agent-core                       ← 通用运行时
        ├── data-core                    ← 数据持久化
        └── log-core                     ← 日志
```
