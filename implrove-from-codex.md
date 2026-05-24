# Codex 借鉴分析 & 改进清单

> **创建日期**: 2026-05-24
>
> 通过阅读 OpenAI Codex (`codex-rs/core`) 的 ReAct 运行时核心代码，
> 提取可借鉴的工程实践和具体改进项，指导 agent-base 的演进。

---

## 1. 背景

OpenAI Codex (`codex-rs`) 是一个已经在生产环境大规模运行的 AI 编程助手。
其核心 `codex-core` crate 包含完整的 Agent 运行时（ReAct 循环、工具路由、审批、
沙箱、compaction、skills 注入等）。虽然 codex-core 深度耦合 OpenAI Responses API
和产品基础设施，不适合直接依赖，但其架构设计中有多个值得 agent-base 借鉴的工程实践。

codex-core 架构概要：

```
TUI/CLI/App-Server
    │
    ├── tx_sub (Sender<Submission>)    ← 命令通道
    └── rx_event (Receiver<Event>)     ← 事件流

submission_loop (tokio task):
  Op::UserTurn → run_turn():
    → pre_sampling_compact （上下文压缩）
    → record_context_updates （注入系统指令）
    → Skills/Plugins/MCP 依赖解析
    → skill/plugin inject （注入技能文档到消息历史）
    → main loop:
        → drain pending_input
        → LLM 采样 (streaming SSE)
        → if tool_calls: parallel_execution → ToolOrchestrator → 回写结果 → continue
        → if no tool_calls: post-turn hooks → break
```

---

## 2. 可借鉴特性

### 2.1 Auto-Compaction（自动上下文压缩）⭐ 最重要

**现状**：agent-base 和 skilllite 都没有真正的上下文压缩。
skilllite 在上下文溢出时只做简单截断 tool 消息，
agent-base 完全没有处理。当对话变长时，LLM token 越界是必然的。

**Codex 做法**：

```
上下文快满时：
  1. 收集历史用户消息和 assistant 回复
  2. 调另一个 LLM 调用将这些对话压缩成结构化摘要
  3. 用摘要替换完整历史（保留最近 N 条消息）
  4. 继续当前 turn（mid-turn compaction）
```

两种触发模式：
- **Pre-turn compaction**：turn 开始前压缩
- **Mid-turn compaction**：turn 执行中上下文不够了，中间插入压缩

**agent-base 改进方向**：

```rust
// agent-base 可能新增

#[async_trait]
pub trait CompactionStrategy: Send + Sync {
    /// 压缩历史消息为摘要
    async fn compact(
        &self,
        messages: &[ChatMessage],
        llm: &dyn LlmClient,
        model: &str,
    ) -> AgentResult<CompactionResult>;
}

pub struct CompactionResult {
    pub summary: String,
    pub recent_messages_to_keep: Vec<ChatMessage>,
    pub tokens_before: usize,
    pub tokens_after: usize,
}
```

可通过 `ContextWindowManager` 或 Middleware 集成：

```rust
// 方案 A: ContextWindowManager 增强
pub struct ContextWindowManager {
    max_tokens: usize,
    compaction_strategy: Option<Arc<dyn CompactionStrategy>>,
    compaction_threshold: f32, // 0.8 = 当使用超 80% 时触发
}

// 方案 B: Middleware
pub struct AutoCompactMiddleware {
    strategy: Arc<dyn CompactionStrategy>,
    threshold: f32,
}
```

---

### 2.2 Parallel Tool Execution（并行工具执行）⭐

**现状**：agent-base 的 `handle_tool_calls` 是串行的 for 循环：

```rust
// react_loop.rs - 当前：顺序执行
for (id, name, args_str, args) in parsed_calls {
    let tool_result = self.tool_engine.execute_tool(...).await;
    // ...
}
```

**问题**：LLM 经常在一次响应中返回多个无依赖的 tool call（如同时 `read_file A` 和 `read_file B`），串行执行浪费大量时间。

**Codex 做法**：`ToolCallRuntime` 用 `FuturesUnordered` 并发调度所有无依赖的 tool call。

**agent-base 改进方向**：

```rust
// react_loop.rs - 改进后
use futures_util::stream::FuturesUnordered;
use futures_util::StreamExt;

let mut futures: FuturesUnordered<_> = parsed_calls
    .into_iter()
    .map(|(id, name, args_str, args)| {
        self.tool_engine.execute_tool(
            session_id, &id, &name, &args, &args_str, ...)
    })
    .collect();

let mut should_break = false;
while let Some(result) = futures.next().await {
    match result {
        Ok(output) => {
            if matches!(output.control_flow, ToolControlFlow::Break) {
                should_break = true;
            }
            // 回写结果到 session
        }
        Err(e) => {
            // 错误处理
        }
    }
}

if should_break { return ToolCallResult::Break; }
```

**注意**：需要处理 Break 信号。如果任何一个 tool 返回 Break，
其他已经在飞的 tool 可以等它们完成（不回写结果），或者取消。

---

### 2.3 Steer Input（中轮用户输入注入）

**现状**：agent-base 的 `run_turn_with_handler` 一旦开始就无法中途注入新的用户消息。
用户必须等当前 turn 完成才能发送新消息。

**Codex 做法**：

```rust
// 用户可以在 turn 执行中通过 steer_input 注入新消息
pub async fn steer_input(&self, input: Vec<UserInput>, expected_turn_id: Option<&str>) {
    let mut turn_state = active_turn.turn_state.lock().await;
    turn_state.push_pending_input(input.into());
    turn_state.accept_mailbox_delivery_for_current_turn();
}
// 下一次 LLM 采样前会 drain pending_input，将新用户消息写入 history
```

**agent-base 改进方向**：
agent-base 的 EventBus + `tokio::select!` 架构天然支持这个。
在 `process_stream` 中已经用 `select!` 监听事件，
可以扩展为也检查是否有 pending user input：

```rust
// react_loop.rs 或 llm_engine.rs 中
tokio::select! {
    recv_result = event_rx.recv() => {
        match recv_result {
            Ok(AgentEvent::UserSteerInput { text, .. }) => {
                // 发现中轮注入，中断当前 LLM 调用
                // 将用户输入 push 到 session
                // 重新调用 LLM
                break; // 跳出当前 stream processing
            }
            // ...
        }
    }
    maybe_chunk = stream.next() => { ... }
}
```

这需要：
1. 新增 `AgentEvent::UserSteerInput` 事件类型
2. AgentRuntime 提供 `steer_input(&self, session_id, input)` API
3. react_loop 在 LLM 采样完成后检查是否有 pending input，如果有则注入后继续

---

### 2.4 Turn 前置处理（Pre-turn Dependency Resolution）

**现状**：agent-base 在 `run_turn_with_handler` 中，用户消息 push 进 session 后直接开始 LLM 调用。
没有"前置处理"阶段。

**Codex 做法**：每轮 turn 开始前执行大量前置工作：

1. **Skill 依赖解析**：收集 skill 需要的环境变量和 MCP 依赖
2. **MCP 依赖安装**：`maybe_prompt_and_install_mcp_dependencies`
3. **Skill 文档注入**：`record_conversation_items(skill_items)`
4. **Plugin 注入**：`record_conversation_items(plugin_items)`
5. **Hooks 执行**：session start hooks、user prompt submit hooks
6. **显式 mention 收集**：检测用户消息中提到的 connectors/apps/skills
7. **Ghost commit 启动**：后台创建 diff snapshot（代码助手特性）

**agent-base 改进方向**：
这些可以通过已有的 Middleware 管道实现，不需要改核心架构：

```rust
// on_user_message middleware 中：
impl Middleware for SkillDependencyMiddleware {
    async fn on_user_message(&self, ctx: &mut UserMessageCtx) -> AgentResult<()> {
        // 1. 检测用户消息中显式提到的 skill
        // 2. 收集 env var 依赖
        // 3. 安装 MCP 依赖
        // 4. 将处理结果注入到 session
        Ok(())
    }
}

// on_pre_llm middleware 中：
impl Middleware for SkillInjectionMiddleware {
    async fn on_pre_llm(&self, ctx: &mut PreLlmCtx) -> AgentResult<()> {
        // 将完整的 skill 文档注入到 messages 中
        // （渐进式披露：只注入被提及的 skill 的完整文档）
        Ok(())
    }
}
```

---

### 2.5 分层工具执行（审批 → 沙箱 → 执行 → 重试）

**现状**：agent-base 的工具执行是单层审批 + 直接执行。
Codex 的 `ToolOrchestrator` 做三层：

```
approval → select sandbox → attempt → retry on denial (escalated sandbox)
```

**agent-base 改进方向**：
沙箱选择不是 agent-base 的职责（是平台的事），但分层执行模式可以抽象成 trait：

```rust
#[async_trait]
pub trait ToolExecutionStrategy: Send + Sync {
    /// 在工具执行前后做额外处理
    async fn before_execute(&self, ctx: &ToolContext, tool_name: &str, args: &Value)
        -> AgentResult<()> { Ok(()) }

    async fn after_execute(&self, ctx: &ToolContext, output: &ToolOutput)
        -> AgentResult<ToolOutput> { Ok(output.clone()) }

    /// 执行失败后的重试策略
    async fn on_error(&self, ctx: &ToolContext, error: &AgentError, retry_count: usize)
        -> AgentResult<ToolErrorAction> {
        Ok(ToolErrorAction::Stop)
    }
}
```

agent-base 已有的 `ToolPolicy` 和 `ToolErrorRecovery` 可以保持不变，
新增 `ToolExecutionStrategy` 作为可选装配。

---

### 2.6 Contextual Fragment 注入

**现状**：agent-base 只有 system prompt + user message 两种消息注入方式。
所有上下文信息都堆在 system prompt 里。

**Codex 做法**：引入了 `developer` role 消息和 `contextual_user` 消息：

```
ResponseItem::Message { role: "developer", content: [...] }  ← 系统指令、权限说明
ResponseItem::Message { role: "user", content: [ContextualUserFragment] }  ← 环境上下文
```

这些消息在 history 中以独立 item 存在，可以被 rollback/compact。

**agent-base 改进方向**：
agent-base 的 `ChatMessage` 已经支持 `MessageRole::Developer`（OpenAI 的新 role）。
可以基于此做上下文分段注入：

```rust
pub enum ChatMessage {
    System { content: String, ... },
    User { content: String, ... },
    Assistant { content: String, ... },
    Developer { content: String, ... },  // ← 已有
    Tool { tool_call_id: String, content: String, ... },
}
```

Middleware 可以在 `on_pre_llm` 中插入 Developer 消息来注入系统上下文，
而不是全部塞进 system prompt。

---

### 2.7 Multi-Agent 通信（InterAgentCommunication + Mailbox）

**现状**：agent-base 有 `SubAgentTool`，但子 agent 之间不能相互通信。
每个 SubAgentTool 是孤立的。

**Codex 做法**：

```rust
// Agent 间通信
InterAgentCommunication::new(
    child_agent_path,
    parent_agent_path,
    message,
    /*trigger_turn*/ false,
);

// Mailbox 系统
mailbox.send(communication);
// 下一轮 turn 会 drain mailbox 中的消息到 pending_input
```

**agent-base 改进方向**（长期）：

```rust
// 增强 AgentRuntime
impl AgentRuntime {
    /// 向另一个 session 发送消息
    pub async fn send_to_session(&self, target: &SessionId, message: InterAgentMessage)
        -> AgentResult<()>;
}

pub struct InterAgentMessage {
    pub from: SessionId,
    pub content: String,
    pub trigger_turn: bool,  // 是否立即触发接收方的 turn
}
```

这是高级特性，短期优先级低。

---

### 2.8 TurnDiffTracker / Undo 支持

**场景**：agent 在 turn 中修改了多个文件，用户想 undo 整个 turn 的改动。

**Codex 做法**：`TurnDiffTracker` 在整个 turn 中追踪所有文件变更，支持：
- Diff 展示（turn 结束后显示改动）
- Undo（回滚整个 turn 的文件改动）

**agent-base 改进方向**：
这是代码助手（harness）的领域需求，不适合放进 agent-base 核心。
在 harness 层实现即可。

---

## 3. 优先级排序

| 优先级 | 特性 | 改动量 | 收益 | 备注 |
|--------|------|--------|------|------|
| **P0** | Auto-Compaction | 中 | 高 | 解决长对话必然出现的 token 溢出 |
| **P0** | Parallel Tools | 小 | 高 | 现有架构几乎不用改，换个并发容器 |
| **P1** | Steer Input | 中 | 中 | 利用已有 EventBus+select!，架构上已支持 |
| **P1** | Turn 前置处理 | 小 | 中 | 通过 Middleware，不涉及核心改动 |
| **P2** | Contextual Fragment 注入 | 小 | 中 | 已有 Developer role，加 Middleware |
| **P2** | 分层工具执行 | 中 | 中 | 抽象现有 ToolPolicy/ToolErrorRecovery |
| **P3** | Multi-Agent 通信 | 大 | 中 | 需要增加 agent registry + session 间通信 |
| **P3** | TurnDiffTracker | 大 | 低 | 领域相关，放 harness 层 |

---

## 4. 实施建议

### Phase A（短期，1-2 周）

1. **Parallel Tools**：改 `react_loop.rs` 的 `handle_tool_calls` 为 `FuturesUnordered`
2. **Turn 前置处理 Middleware**：新增示例 Middleware 演示 skill 依赖解析/注入
3. **Contextual Fragment Middleware**：用 Developer role 注入上下文

### Phase B（中期，2-4 周）

4. **Auto-Compaction**：实现 `CompactionStrategy` trait + `AutoCompactMiddleware`
5. **Steer Input**：新增 `UserSteerInput` event + `steer_input` API

### Phase C（长期，后续迭代）

6. **分层工具执行策略**：`ToolExecutionStrategy` trait
7. **Multi-Agent 通信**：`InterAgentMessage` + session 间通信

---

## 5. 不做的事

- ❌ 不引入 codex-core 作为依赖（34 个 crate，OpenAI Responses API 专用）
- ❌ 不照搬 Codex 的事件模型（200+ 事件类型，过度设计）
- ❌ 不实现沙箱（Landlock/Seatbelt）——平台层面的事
- ❌ 不实现 Codex 的产品特性（Guardian review、realtime audio 等）
