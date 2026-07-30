# 高级用法

Middleware、会话管理、事件日志等进阶功能。

## Middleware（中间件）

Middleware 在 LLM 调用前后介入 Agent 循环：

```rust
use agent_base::{TurnFactMiddleware, TurnToolLimitMiddleware};

let builder = base_agent_builder(llm_client)
    .system_prompt(system_prompt)
    .middleware(TurnFactMiddleware::new())
    .middleware(TurnToolLimitMiddleware::from_config(&safety));
```

内置中间件：
- `TurnFactMiddleware` — 在每轮开始时注入事实/上下文
- `TurnToolLimitMiddleware` — 强制执行 `max_tool_calls_per_turn` 限制

## 审批处理器

控制哪些工具调用需要人工确认：

```rust
// 全部自动批准（CI / 自动化场景）
use phi_agent::{AutoApprovalHandler, ApprovalMode};
builder = builder.approval_handler(Arc::new(
    AutoApprovalHandler::new(ApprovalMode::Auto)
));

// 全部拒绝（只读 / 预览模式）
builder = builder.approval_handler(Arc::new(
    AutoApprovalHandler::new(ApprovalMode::DenyAll)
));
```

交互式 CLI 审批参见 phi 二进制中的 `CliApprovalHandler`。

## 会话管理

会话用于持久化对话历史和工具调用结果：

```rust
use phi_agent::session::{resolve_session, cleanup_expired_sessions};

// 创建或复用会话
let ctx = resolve_session(Some("my-session"), &base_dir)?;
println!("Session: {} (new: {})", ctx.session_id, ctx.is_new_session);

// 清理过期会话（> 7 天）
let cleaned = cleanup_expired_sessions(&base_dir, 7)?;
println!("Cleaned {} expired sessions", cleaned);
```

会话目录结构：
```
~/.phi-agent/sessions/<id>/
├── session_id           # 会话 ID 标记
├── session.lock         # 独占文件锁
├── session_meta.json    # 创建时间、最后活跃时间
└── turn_001.jsonl       # 每轮事件日志（JSONL）
```

## 事件日志

每轮对话都以 JSONL 格式保存，方便回放和分析：

```rust
use phi_agent::{save_turn_log, event_to_jsonl};

// 保存本轮事件
save_turn_log(&session_ctx, 1, &events, "用户查询内容")?;

// 将单个事件转为 JSONL 行
let line = event_to_jsonl(&event);
```

日志中的事件类型：
- `thought_delta` — LLM 思维过程内容
- `text_delta` — 助手文本输出
- `tool_call_started` / `tool_call_finished` — 工具调用
- `approval_request` — 需要审批的工具调用
- `plan_updated` — 任务计划更新
- `turn_finished` — 轮次汇总（包含耗时和统计信息）

## 系统提示词

phi-agent 提供两种系统提示词变体：

```rust
use phi_agent::{build_system_prompt, build_system_prompt_cn};

// 默认（国际版）
let prompt = build_system_prompt();

// 中国网络环境适配版（优先国内服务，处理 GFW）
let prompt_cn = build_system_prompt_cn();
```

你也可以通过 `builder.system_prompt(...)` 传入完全自定义的提示词。

## 推理 / 思考

控制 LLM 的思维链行为：

```rust
use agent_base::{ReasoningConfig, ReasoningEffort};

// Builder 级别的默认值
builder = builder.reasoning(ReasoningConfig {
    effort: Some(ReasoningEffort::High),
    ..Default::default()
});

// 单轮覆盖
agent.set_reasoning_effort(ReasoningEffort::XHigh).await;
```

推理强度级别及适用场景：
- `Low` — 简单任务，快速响应
- `Medium` — 默认，平衡
- `High` — 复杂的多步骤任务
- `XHigh` — 最困难的问题，最长思考时间

## 编程式使用 Renderer

在 CLI 之外使用渲染器：

```rust
use phi_agent::{
    TerminalRenderer, JsonStreamRenderer, NullRenderer, EventRenderer,
};
use std::io;

// 终端渲染
let mut renderer = TerminalRenderer::new(true, true, true, Box::new(io::stdout()));

// JSON 流渲染（适用于 IDE 集成）
let mut renderer = JsonStreamRenderer::stdout();

// 静默渲染（适用于 Web 后端）
let mut renderer = NullRenderer;
```

## 错误恢复

phi-agent 默认配置了连续失败恢复机制：

```rust
use agent_base::ConsecutiveFailureRecovery;

// 连续 3 次失败 → 停止并说明原因
builder = builder.error_recovery(Arc::new(
    ConsecutiveFailureRecovery::new(3)
));
```
