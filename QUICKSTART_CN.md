# 快速上手：5 分钟构建你自己的 Agent

> 从零开始，搭建一个带工具调用、审批流程、事件流的完整 Agent。

本教程带你构建一个 **服务器健康检查 Agent**——通过一个真实场景，展示 `agent-base` 的所有核心概念。没有废话，直接上能用的代码。

---

## 你将构建什么

一个 CLI Agent，具备以下能力：
- 检查远程服务器磁盘使用情况
- 查看内存状态
- 重启服务（需要人工审批）
- 实时流式输出事件

读完本教程，你将理解：**Tool → Runtime → Event → Approval → Middleware** 的完整链路。

---

## Step 0：创建项目

```bash
mkdir my-agent && cd my-agent
cargo init
```

在 `Cargo.toml` 中添加依赖：

```toml
[dependencies]
agent-base = "0.1.2"
async-trait = "0.1"
serde_json = "1"
tokio = { version = "1", features = ["full"] }
dotenvy = "0.15"
```

---

## Step 1：定义你的第一个 Tool

**Tool** 是 LLM 可以调用的任何能力。每个 Tool 有四个部分：
- `name()` — 唯一标识符，LLM 通过它来调用工具
- `description()` — 人类可读的工具用途描述
- `schema()` — JSON Schema，告诉 LLM 该传什么参数
- `call()` — 异步执行逻辑

```rust
// src/tools.rs
use agent_base::{Tool, ToolContext, Content, AgentResult};
use async_trait::async_trait;
use serde_json::{json, Value};

pub struct DiskCheckTool;

#[async_trait]
impl Tool for DiskCheckTool {
    fn name(&self) -> &'static str {
        "check_disk"
    }

    fn description(&self) -> &'static str {
        "检查服务器磁盘使用情况。返回已用/总空间和使用百分比。"
    }

    fn schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "path": {
                    "type": "string",
                    "description": "要检查的文件系统路径（如 '/'、'/home'、'/var'）"
                }
            },
            "required": ["path"]
        })
    }

    async fn call(&self, args: &Value, _ctx: &ToolContext) -> AgentResult<Vec<Content>> {
        let path = args["path"].as_str().unwrap_or("/");
        // 实际项目中，这里会通过 SSH 或 sysinfo 获取真实数据
        // 本教程中我们模拟输出
        let output = format!(
            "文件系统: {}\n总计: 50G  已用: 32G  可用: 18G  使用率: 64%",
            path
        );
        Ok(vec![Content::text(output)])
    }
}
```

> **关键点：** 工具返回结构化的 `Content`（通常用 `Content::text(...)`）。工具执行完后，Runtime 会把结果回传给 LLM，LLM 继续推理直到给出最终答案。

---

## Step 2：组装 Runtime

**AgentBuilder** 是你的入口。它负责配置 LLM、注册工具、构建运行时。

```rust
// src/main.rs
mod tools;

use std::sync::Arc;
use agent_base::{AgentBuilder, RuntimeEvent, AgentResult, OpenAiClient, RunOutcome, RetryOnError};
use tools::DiskCheckTool;

const SYSTEM_PROMPT: &str = r#"你是一个服务器健康检查助手。
你可以检查磁盘使用情况和内存状态。
当用户询问服务器健康状况时，调用相应的工具获取数据。
回答要简洁，用要点列表汇报结果。"#;

#[tokio::main]
async fn main() -> AgentResult<()> {
    dotenvy::dotenv().ok();

    // 1. 创建 LLM 客户端（兼容 OpenAI 接口）
    let llm = Arc::new(OpenAiClient::new(
        std::env::var("OPENAI_API_KEY").expect("请设置 OPENAI_API_KEY"),
        "gpt-4o-mini",           // 可换成任何兼容 OpenAI 接口的模型
        None,                    // 或传入代理地址，如 Some("https://your-proxy.com/v1")
    ));

    // 2. 构建运行时
    let runtime = AgentBuilder::new(llm)
        .system_prompt(SYSTEM_PROMPT)
        .register_tool(DiskCheckTool)
        // .error_recovery(Arc::new(RetryOnError))  // 取消注释可开启自动重试
        .build()?;

    // 3. 创建会话并运行
    let session_id = runtime.create_session().await;

    let (events, outcome) = runtime
        .run_turn_collect(session_id, "检查 / 目录的磁盘使用情况")
        .await?;

    // 4. 打印结果
    for event in &events {
        match event {
            RuntimeEvent::TextDelta { text, .. } => print!("{}", text),
            RuntimeEvent::ToolCallStarted { tool_name, .. } => {
                println!("\n🔧 调用工具: {}", tool_name);
            }
            RuntimeEvent::ToolCallFinished { summary, .. } => {
                println!("✅ 结果: {}", summary);
            }
            RuntimeEvent::RunFinished { .. } => println!("\n[完成]"),
            _ => {}
        }
    }

    assert_eq!(outcome, RunOutcome::Completed);
    Ok(())
}
```

运行：

```bash
OPENAI_API_KEY=sk-xxx cargo run
```

**就这样。** LLM 会看到工具定义，决定调用 `check_disk`，拿到结果后格式化输出。Runtime 处理完整的循环：LLM → 工具调用 → 执行 → 回传结果 → LLM 回复。

---

## Step 3：为敏感操作添加审批流程

有些工具需要人工审批（重启服务、删除数据等）。`agent-base` 把审批拆成两个 trait：

| Trait | 职责 | 何时执行 |
|---|---|---|
| `ToolPolicy` | 判断工具*是否*需要审批 | 每次工具调用前（同步、无状态） |
| `ApprovalHandler` | 执行审批*交互* | 仅当 Policy 判定需要审批时触发（异步） |

```rust
// src/approval.rs
use agent_base::{
    ApprovalHandler, ApprovalRequest, ApprovalDecision,
    ToolPolicy, ToolContext, AgentResult, RiskLevel,
};
use async_trait::async_trait;
use serde_json::Value;

/// Policy：restart_service 需要审批，其他工具自动放行
pub struct HealthCheckPolicy;

#[async_trait]
impl ToolPolicy for HealthCheckPolicy {
    async fn evaluate_approval(
        &self,
        tool_name: &str,
        _args: &Value,
    ) -> Option<ApprovalRequest> {
        match tool_name {
            "restart_service" => Some(ApprovalRequest {
                title: "重启服务".into(),
                message: "是否允许重启该服务？".into(),
                risk_level: RiskLevel::Sensitive,
                action_key: Some(format!("restart:{}", _args.get("service").unwrap_or(&Value::Null))),
                raw: None,
            }),
            _ => None, // 其他工具自动放行
        }
    }

    fn before_call(&self, _tool_name: &str, _args: &Value, _ctx: &ToolContext) -> AgentResult<()> {
        Ok(())
    }

    fn after_call(
        &self, _tool_name: &str, _args: &Value,
        _result: &[agent_base::Content], _ctx: &ToolContext,
    ) -> AgentResult<()> {
        Ok(())
    }
}

/// Handler：基于 CLI 的审批（在终端询问用户）
pub struct CliApproval;

#[async_trait]
impl ApprovalHandler for CliApproval {
    async fn approve(&self, request: ApprovalRequest) -> AgentResult<ApprovalDecision> {
        println!("\n⚠️  需要审批: {}", request.title);
        println!("   风险等级: {:?}", request.risk_level);
        println!("   {}", request.message);
        print!("   是否允许？[y/n]: ");
        // 生产环境中从 stdin 读取用户输入，这里为了演示直接放行
        Ok(ApprovalDecision::AllowOnce)
    }
}
```

在 `main.rs` 中注册：

```rust
use approval::{HealthCheckPolicy, CliApproval};

let runtime = AgentBuilder::new(llm)
    .system_prompt(SYSTEM_PROMPT)
    .register_tool(DiskCheckTool)
    .register_tool(RestartServiceTool)  // 你需要自己定义这个工具
    .tool_policy(Arc::new(HealthCheckPolicy))
    .approval_handler(Arc::new(CliApproval))
    .build()?;
```

**审批流程：**
```
LLM 决定调用 "restart_service"
    → ToolPolicy::evaluate_approval() 返回 Some(ApprovalRequest)
    → ApprovalHandler::approve() 询问人工
    → 批准？→ 工具执行
    → 拒绝？→ LLM 收到"被拒绝"的消息，自行调整策略
```

---

## Step 4：添加 Middleware

**Middleware** 可以在 Agent 循环的三个节点插入自定义逻辑：

| 钩子 | 触发时机 | 典型用途 |
|---|---|---|
| `on_user_message` | 用户消息进入会话之前 | 输入清洗、命令改写 |
| `on_pre_llm` | 发送给 LLM 之前 | 注入额外上下文、过滤消息 |
| `on_post_llm` | LLM 返回响应之后 | 防幻觉、屏蔽敏感内容 |

下面是一个实际的例子——**反幻觉 Middleware**，当 LLM 有工具可用却不调用、只是在"描述它会做什么"时，强制它去调用工具：

```rust
// src/middleware.rs
use std::sync::atomic::{AtomicUsize, Ordering};
use agent_base::{Middleware, PostLlmCtx, AgentResult};
use async_trait::async_trait;

pub struct ToolEnforcement {
    max_nudges: usize,
    nudge_count: AtomicUsize,
}

impl ToolEnforcement {
    pub fn new(max_nudges: usize) -> Self {
        Self {
            max_nudges,
            nudge_count: AtomicUsize::new(0),
        }
    }
}

#[async_trait]
impl Middleware for ToolEnforcement {
    async fn on_post_llm(&self, ctx: &mut PostLlmCtx) -> AgentResult<()> {
        // 仅在以下条件同时满足时触发：有可用工具、LLM 没调用任何工具、还没超过最大推动次数
        if ctx.available_tools.is_empty()
            || ctx.is_tool_call
            || ctx.full_text.is_empty()
            || ctx.total_tool_calls > 0
        {
            return Ok(());
        }

        let count = self.nudge_count.fetch_add(1, Ordering::SeqCst);
        if count >= self.max_nudges {
            return Ok(()); // 超过最大推动次数，放弃
        }

        // 丢弃当前文本响应，注入一条跟进指令
        ctx.skip_push = true;
        ctx.follow_up_message = Some(
            "你有可用的工具。请直接调用工具，不要只描述你会做什么。".into()
        );
        Ok(())
    }
}
```

注册到运行时：

```rust
use middleware::ToolEnforcement;

let runtime = AgentBuilder::new(llm)
    .system_prompt(SYSTEM_PROMPT)
    .register_tool(DiskCheckTool)
    .middleware(ToolEnforcement::new(3))  // 最多推动 3 次
    .build()?;
```

---

## Step 5：实时事件流

用 `run_turn` 可以实时接收每个事件，适合 CLI 和 WebSocket 场景：

```rust
use std::io::{self, Write};
use agent_base::{RuntimeEvent, AgentResult};

// 这是一个同步回调——每个事件触发时立即调用
fn on_event(event: RuntimeEvent) -> AgentResult<()> {
    match event {
        RuntimeEvent::TextDelta { text, .. } => {
            print!("{}", text);
            io::stdout().flush().unwrap();
        }
        RuntimeEvent::ToolCallStarted { tool_name, args_json, .. } => {
            println!("\n🔧 {}({})", tool_name, args_json);
        }
        RuntimeEvent::ToolCallFinished { summary, .. } => {
            // 长输出截断显示
            let display = if summary.len() > 200 {
                format!("{}...", &summary[..200])
            } else {
                summary.clone()
            };
            println!("  → {}", display);
        }
        RuntimeEvent::RunFinished { .. } => {
            println!("\n✅ 完成");
        }
        _ => {}
    }
    Ok(())
}

// 使用方式：
let outcome = runtime
    .run_turn(session_id, "检查磁盘和内存", on_event)
    .await?;
```

> **提示：** `run_turn` 适合实时流式场景（CLI、WebSocket）。`run_turn_collect` 把所有事件收集到 Vec 里——适合测试或批量处理。

---

## Step 6：多轮对话

Session 自动保存消息历史。只要用同一个 `session_id` 持续调用 `run_turn_*` 即可：

```rust
let session_id = runtime.create_session().await;

// 第 1 轮：询问磁盘情况
runtime.run_turn(session_id.clone(), "检查 / 的磁盘使用", on_event).await?;

// 第 2 轮：追问——LLM 记得上一轮的上下文
runtime.run_turn(session_id.clone(), "/var 目录呢？", on_event).await?;

// 第 3 轮：决策——LLM 拥有前两轮的完整上下文
runtime.run_turn(session_id.clone(), "哪个更需要关注？", on_event).await?;
```

Runtime 自动管理 Session 中的消息历史（用户消息、助手回复、工具调用、工具结果）。

---

## Step 7：错误恢复

默认情况下，工具执行失败会**直接停止**（`StopOnError`）。对于需要自动恢复的 Agent（如失败后重试命令），注入 `RetryOnError`：

```rust
use agent_base::RetryOnError;

let runtime = AgentBuilder::new(llm)
    .register_tool(DiskCheckTool)
    .error_recovery(Arc::new(RetryOnError))  // 把错误喂回 LLM，让它重试
    .build()?;
```

使用 `RetryOnError` 后，工具失败时：
1. 错误消息作为用户消息注入到对话中
2. LLM 看到错误后可以调整策略
3. 循环继续（直到达到 `max_turns` 上限）

你也可以实现自定义的恢复逻辑：

```rust
use agent_base::{ToolErrorRecovery, ToolErrorAction, AgentResult, SessionId, AgentError};

struct SmartRecovery;

impl ToolErrorRecovery for SmartRecovery {
    fn on_error(
        &self,
        _session_id: &SessionId,
        tool_names: &[String],
        error: &AgentError,
    ) -> AgentResult<ToolErrorAction> {
        // SSH 超时自动重试，认证失败则停止
        if error.to_string().contains("timeout") {
            Ok(ToolErrorAction::Retry)
        } else {
            Ok(ToolErrorAction::Stop)
        }
    }
}
```

---

## 完整最小可运行 Agent（直接复制粘贴）

```rust
// src/main.rs
use std::sync::Arc;
use std::io::{self, Write};

use agent_base::{
    AgentBuilder, RuntimeEvent, AgentResult, OpenAiClient, RunOutcome,
    Tool, ToolContext, Content,
};
use async_trait::async_trait;
use serde_json::{json, Value};

struct CheckDiskTool;

#[async_trait]
impl Tool for CheckDiskTool {
    fn name(&self) -> &'static str { "check_disk" }

    fn description(&self) -> &'static str {
        "检查指定路径的磁盘使用情况"
    }

    fn schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "path": { "type": "string", "description": "要检查的路径" }
            },
            "required": ["path"]
        })
    }

    async fn call(&self, args: &Value, _ctx: &ToolContext) -> AgentResult<Vec<Content>> {
        let path = args["path"].as_str().unwrap_or("/");
        Ok(vec![Content::text(format!(
            "{}: 总计 50G，已用 32G（64%）",
            path
        ))])
    }
}

fn on_event(event: RuntimeEvent) -> AgentResult<()> {
    match event {
        RuntimeEvent::TextDelta { text, .. } => { print!("{}", text); io::stdout().flush().unwrap(); }
        RuntimeEvent::ToolCallStarted { tool_name, .. } => println!("\n🔧 {}", tool_name),
        RuntimeEvent::ToolCallFinished { summary, .. } => println!("  → {}", summary),
        RuntimeEvent::RunFinished { .. } => println!("\n✅"),
        _ => {}
    }
    Ok(())
}

#[tokio::main]
async fn main() -> AgentResult<()> {
    dotenvy::dotenv().ok();

    let llm = Arc::new(OpenAiClient::new(
        std::env::var("OPENAI_API_KEY").expect("请设置 OPENAI_API_KEY"),
        "gpt-4o-mini", None,
    ));

    let runtime = AgentBuilder::new(llm)
        .system_prompt("你是一个服务器健康检查助手。使用工具来检查状态。回答要简洁。")
        .register_tool(CheckDiskTool)
        .build()?;

    let session_id = runtime.create_session().await;

    // REPL 循环
    loop {
        print!("> ");
        io::stdout().flush().unwrap();
        let mut input = String::new();
        io::stdin().read_line(&mut input).unwrap();
        let input = input.trim();
        if input == "exit" { break; }

        match runtime.run_turn(session_id.clone(), input, on_event).await {
            Ok(_) => {}
            Err(e) => eprintln!("错误: {}", e),
        }
    }
    Ok(())
}
```

```bash
cargo run
# > 检查 / 的磁盘
# 🔧 check_disk
#   → /: 总计 50G，已用 32G（64%）
# 根据磁盘使用报告，根目录文件系统总容量 50G...
# ✅
```

---

## 核心概念速查表

| 概念 | 是什么 | 什么时候用 |
|---|---|---|
| `Tool` trait | 定义 LLM 可调用的能力 | 每个 Agent 至少需要一个 Tool |
| `description()` + `schema()` | 告诉 LLM 工具的用途和参数 | 每个 Tool 都要实现 |
| `Content` | 返回给 LLM 的结构化工具结果（文本/图片） | 每个 `call()` 返回 `Vec<Content>` |
| `ToolPolicy` | 判断工具是否需要人工审批 | 敏感操作 |
| `ApprovalHandler` | 执行审批交互 | 和 ToolPolicy 配合使用 |
| `Middleware` | 在 Agent 循环中插入自定义逻辑 | 输入输出过滤、防幻觉 |
| `ToolErrorRecovery` | 工具失败后的行为策略 | `StopOnError`（默认）或 `RetryOnError` |
| `RuntimeEvent` 流 | 运行时实时事件 | UI 更新、日志、调试 |
| `SessionId` | 多轮对话句柄 | REPL 或聊天 UI |

---

## 接下来做什么

- **子 Agent**：构建专家 Agent 并组合为工具 → `examples/subagent_demo.rs`
- **Plan 编排器**：多步任务规划与执行 → `examples/plan_demo.rs`
- **Middleware 模式**：防幻觉、内容过滤 → `examples/middleware_demo.rs`
- **自定义 LLM 提供商**：实现 `LlmClient` trait 对接你自己的 LLM 服务

---

## 典型分层架构

```
你的垂直 Agent（如 ops-agent、db-agent）
    ├── 领域专属工具（SSH、SQL、API 调用）
    ├── 领域专属 Middleware（防幻觉、安全策略）
    └── agent-base（运行时内核）
         ├── LLM 抽象
         ├── 工具调度
         ├── 审批流程
         └── 事件流
```

`agent-base` 负责编排。**你**带来领域知识。