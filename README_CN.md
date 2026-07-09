# agent-base

[![crates.io](https://img.shields.io/crates/v/agent-base.svg)](https://crates.io/crates/agent-base)
[![Documentation](https://docs.rs/agent-base/badge.svg)](https://docs.rs/agent-base)
[![MIT License](https://img.shields.io/badge/license-MIT-blue.svg)](LICENSE)

[English](README.md) | [中文](README_CN.md)

用 Rust 构建 AI Agent 的轻量级 **Agent 运行时内核**。

`agent-base` 提供了构建自定义 AI Agent 所需的最小编排层——LLM 集成、工具调度、多轮对话、审批流程、事件流和错误恢复——零业务假设。

## 安装

```toml
[dependencies]
agent-base = "0.1.2"
```

## 设计原则

- **清晰的语义** — `RunOutcome` 明确区分 `Completed`（成功）和 `Failed`（失败）；事件捕获过程，返回值捕获最终结果。
- **简洁的状态模型** — 运行时内存是活跃会话的唯一数据源；`SessionStore` 是可选的持久化适配器。
- **默认保守** — 工具失败时，运行时默认停止（`StopOnError`），而不是猜测如何恢复。
- **策略注入** — 所有可变行为通过 trait 注入（`ToolErrorRecovery`、`ToolPolicy`、`ApprovalHandler`、`Middleware`），而非硬编码。

## 特性

- **LLM 抽象** — `LlmClient` trait，内置 OpenAI 和 Anthropic 实现
- **工具系统** — `Tool` trait + `ToolRegistry` 注册和调度
- **审批流程** — `ApprovalHandler` trait，支持 `AllowOnce` / `AllowAlways` / `Deny` 决策
- **错误恢复** — `ToolErrorRecovery` trait；默认 `StopOnError`，可选 `RetryOnError`
- **事件流** — 结构化 `RuntimeEvent` 流，用于 UI、日志、审计和调试
- **多轮会话** — `AgentSession` 管理消息历史；`SessionStore` 可选持久化
- **子 Agent** — `SubAgentTool`，支持 `Ephemeral`（默认）或 `Persistent` 会话策略
- **上下文管理** — 可配置的 `ContextWindowManager` 控制 token 预算
- **中间件** — `on_user_message`、`on_pre_llm`、`on_post_llm` 三个钩子用于扩展
- **临时消息（Ephemeral Messages）** — 消息可标记为临时，LLM 本轮可见，turn 结束后自动从内存清理且不持久化
- **Plan 检查清单** — `UpdatePlanTool` 内置工具，支持多步骤任务追踪
- **检查点** — 结构化 `Checkpoint` 事件支持未来的重放、调试和恢复
- **工具执行强制** — `ToolEnforcementMiddleware` 促使 LLM 调用工具而非仅描述操作
- **Turn 工具限制** — `TurnToolLimitMiddleware` 可按 turn 限制工具调用次数

## 快速上手

### 1. 定义工具

Agent 的任何能力都以 `Tool` 的形式表达：

```rust
use agent_base::{Tool, ToolContext, ToolOutput, ToolControlFlow, AgentResult};
use async_trait::async_trait;
use serde_json::{json, Value};

struct WeatherTool;

#[async_trait]
impl Tool for WeatherTool {
    fn name(&self) -> &'static str { "get_weather" }

    fn definition(&self) -> Value {
        json!({
            "type": "function",
            "function": {
                "name": "get_weather",
                "description": "查询指定城市的天气",
                "parameters": {
                    "type": "object",
                    "properties": {
                        "city": { "type": "string", "description": "城市名称" }
                    },
                    "required": ["city"]
                }
            }
        })
    }

    async fn call(&self, args: &Value, _ctx: &ToolContext) -> AgentResult<ToolOutput> {
        let city = args["city"].as_str().unwrap_or("未知");
        Ok(ToolOutput {
            summary: format!("{}天气：22°C，晴", city),
            raw: None,
            control_flow: ToolControlFlow::Continue,
            truncated: false,
        })
    }
}
```

### 2. 构建 Agent

```rust
use std::sync::Arc;
use agent_base::{
    AgentBuilder, RuntimeEvent, AgentResult, RunOutcome,
    OpenAiClient, StopOnError,
};

#[tokio::main]
async fn main() -> AgentResult<()> {
    let llm = Arc::new(OpenAiClient::new(
        std::env::var("OPENAI_API_KEY").unwrap(),
        "gpt-4o".into(),
        None,
    ));

    let mut runtime = AgentBuilder::new(llm)
        .system_prompt("你是一个有用的天气助手。")
        .register_tool(WeatherTool)
        .build();

    let session_id = runtime.create_session();
    let (events, outcome) = runtime.run_turn_collect(
        session_id,
        "东京今天天气怎么样？",
    ).await?;

    for event in &events {
        match event {
            RuntimeEvent::TextDelta { text, .. } => print!("{}", text),
            RuntimeEvent::ToolCallStarted { tool_name, .. } => {
                println!("\n[调用工具: {}]", tool_name);
            }
            RuntimeEvent::ToolCallFinished { summary, .. } => {
                println!("[工具结果: {}]", summary);
            }
            RuntimeEvent::RunFinished { .. } => println!("\n[完成]"),
            _ => {}
        }
    }

    assert_eq!(outcome, RunOutcome::Completed);
    Ok(())
}
```

### 3. 处理工具错误

默认情况下，工具失败会停止运行。对于需要自愈的 Agent（如代码 Agent 需要重试编译），注入 `RetryOnError`：

```rust
use agent_base::RetryOnError;

let mut runtime = AgentBuilder::new(llm)
    .register_tool(MyTool)
    .error_recovery(Arc::new(RetryOnError))  // ← 失败时重试
    .build();
```

### 4. 为敏感工具添加审批

```rust
use agent_base::{
    ApprovalHandler, ApprovalRequest, ApprovalDecision,
    ToolPolicy, RiskLevel,
};

struct MyApprovalHandler;
#[async_trait::async_trait]
impl ApprovalHandler for MyApprovalHandler {
    async fn approve(&self, _req: ApprovalRequest) -> AgentResult<ApprovalDecision> {
        // 通过 UI、CLI 等方式询问用户
        Ok(ApprovalDecision::AllowOnce)
    }
}

struct MyToolPolicy;
impl ToolPolicy for MyToolPolicy {
    fn evaluate_approval(&self, tool_name: &str, _args: &Value, _json: &str)
        -> Option<ApprovalRequest>
    {
        if tool_name == "dangerous_tool" {
            Some(ApprovalRequest {
                title: "确认操作".into(),
                message: format!("是否执行 `{}`？", tool_name),
                risk_level: RiskLevel::Sensitive,
                ..Default::default()
            })
        } else {
            None  // 自动放行
        }
    }
}

let mut runtime = AgentBuilder::new(llm)
    .register_tool(DangerousTool)
    .tool_policy(Arc::new(MyToolPolicy))
    .approval_handler(Arc::new(MyApprovalHandler))
    .build();
```

### 5. 使用子 Agent

```rust
use agent_base::SubAgentTool;

// 构建子 Agent 运行时
let sub_llm = Arc::new(OpenAiClient::new(key, model, None));
let sub_runtime = AgentBuilder::new(sub_llm)
    .system_prompt("你是一个数学专家。")
    .build();

// 包装为工具
let math_tool = SubAgentTool::new(
    "calculate",
    "将数学问题委派给数学专家",
    sub_runtime,
);

// 注册到父 Agent
let mut parent = AgentBuilder::new(parent_llm)
    .register_tool(math_tool)
    .build();
```

每次子 Agent 调用默认创建新会话。使用 `SubAgentTool::with_persistent()` 可跨调用共享上下文。

## 示例

```bash
# 配置 API Key
cp .env.example .env
# 编辑 .env，填入你的 OPENAI_API_KEY 或 ANTHROPIC_API_KEY

# 运行 REPL 示例
cargo run --example repl

# 运行子 Agent 示例
cargo run --example subagent_demo

# 运行中间件示例
cargo run --example middleware_demo

# 运行 Plan 示例
cargo run --example plan_demo
```

## agent-base 不做什么

- 内置 SSH、文件系统或数据库工具
- 工作流 DAG 或多 Agent 编排引擎
- 记忆或 RAG（检索增强生成）框架
- 终端 UI 或内置审批对话框
- 生产级持久化或事务系统

业务特定的工具和策略属于**上层**（如 `ops-agent`、`agent-works`、`db-agent`、`browser-agent`）。

## 典型分层

```
ops-agent / agent-works / ...          ← 业务 Agent / 增强工具集
    └── agent-base                      ← 轻量级运行时内核
```

## v1 语义

| 约定 | 含义 |
|---|---|
| `run_turn_*` → `AgentResult<RunOutcome>` | `Ok(Completed)` = 成功，`Ok(Failed)` = 已完成但有错误 |
| `RuntimeEvent::RunFinished` | 流程结束 — 最终状态见 `RunOutcome` |
| 工具失败 → 默认 `StopOnError` | 注入 `RetryOnError` 获得自愈能力 |
| SubAgent → 默认 `Ephemeral` | 使用 `with_persistent()` 共享上下文 |
| Session → 内存是唯一数据源 | `SessionStore` 是可选的持久化适配器 |

## 致谢

本项目在设计过程中参考了 [OpenAI Codex CLI](https://github.com/openai/codex) 项目，尤其在工具编排和任务规划方面有所借鉴。

## 稳定性

本项目处于早期开发阶段（v0.1.2）。核心抽象已趋于稳定但尚未冻结。生态演进过程中可能会有小幅 API 变更。

## 许可证

MIT
