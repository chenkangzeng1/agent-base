//! Plan 恢复策略全览 —— Mock LLM，零依赖
//!
//! 一站式展示所有恢复策略：
//!   Level 0: 框架自动重试（max_retries，线性退避）
//!   Level 1: 替代步骤（自定义 AdaptiveRecoveryStrategy）
//!   Level 2: 重规划（replan，重新编排剩余步骤）
//!   Level 3: 兜底中止（Recovery::abort）
//!
//! 额外展示：LlmAdaptiveRecovery（LLM 驱动的开箱即用恢复）
//!
//! 运行: cargo run --example plan_recovery_demo

use std::collections::HashMap;
use std::pin::Pin;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};

use agent_base::{
    AdaptiveRecoveryStrategy, AgentBuilder, AgentError, AgentResult, ChatMessage,
    ExecutionPlan, LlmAdaptiveRecovery, LlmCapabilities, LlmClient, LlmPlanGenerator,
    PlanConfig, PlanPhase, PlanStatus, PlanStep, Recovery, RecoveryAction, RecoveryContext,
    RunOutcome, RuntimeEvent, StepExecutor, StepResult, StreamChunk, Tool, ToolContext,
    ToolControlFlow, ToolOutput,
};
use async_trait::async_trait;
use serde_json::{json, Value};

// ============================================================================
// Mock 工具 —— 用计数器控制故障模式
// ============================================================================

/// 检查服务 —— 前 N 次返回 ERROR，之后成功
struct CheckServiceTool {
    fail_count: Arc<AtomicUsize>,
    max_fails: usize,
}

#[async_trait]
impl Tool for CheckServiceTool {
    fn name(&self) -> &'static str { "check_service" }

    fn definition(&self) -> Value {
        json!({
            "type": "function",
            "function": {
                "name": "check_service",
                "description": "检查服务状态",
                "parameters": {
                    "type": "object",
                    "properties": { "service": { "type": "string" } },
                    "required": ["service"]
                }
            }
        })
    }

    async fn call(&self, args: &Value, _ctx: &ToolContext) -> AgentResult<ToolOutput> {
        let service = args["service"].as_str().unwrap_or("unknown");
        let n = self.fail_count.fetch_add(1, Ordering::SeqCst) + 1;
        if n <= self.max_fails {
            Ok(ToolOutput {
                summary: format!("ERROR: {} 连接超时 (call #{})", service, n),
                raw: Some(json!({"status": "error", "call_num": n})),
                control_flow: ToolControlFlow::Continue,
                truncation: None,
            })
        } else {
            Ok(ToolOutput {
                summary: format!("{} 运行正常 (call #{})", service, n),
                raw: Some(json!({"status": "healthy", "call_num": n})),
                control_flow: ToolControlFlow::Continue,
                truncation: None,
            })
        }
    }
}

/// 查看日志 —— 始终成功（替代步骤）
struct ViewLogsTool;

#[async_trait]
impl Tool for ViewLogsTool {
    fn name(&self) -> &'static str { "view_logs" }

    fn definition(&self) -> Value {
        json!({
            "type": "function",
            "function": {
                "name": "view_logs",
                "description": "查看服务日志",
                "parameters": {
                    "type": "object",
                    "properties": {
                        "service": { "type": "string" },
                        "lines": { "type": "integer" }
                    },
                    "required": ["service"]
                }
            }
        })
    }

    async fn call(&self, args: &Value, _ctx: &ToolContext) -> AgentResult<ToolOutput> {
        let service = args["service"].as_str().unwrap_or("unknown");
        Ok(ToolOutput {
            summary: format!("[{}] 日志正常，无异常", service),
            raw: Some(json!({"status": "ok"})),
            control_flow: ToolControlFlow::Continue,
            truncation: None,
        })
    }
}

/// 重启服务 —— 始终成功
struct RestartServiceTool;

#[async_trait]
impl Tool for RestartServiceTool {
    fn name(&self) -> &'static str { "restart_service" }

    fn definition(&self) -> Value {
        json!({
            "type": "function",
            "function": {
                "name": "restart_service",
                "description": "重启服务",
                "parameters": {
                    "type": "object",
                    "properties": { "service": { "type": "string" } },
                    "required": ["service"]
                }
            }
        })
    }

    async fn call(&self, args: &Value, _ctx: &ToolContext) -> AgentResult<ToolOutput> {
        let service = args["service"].as_str().unwrap_or("unknown");
        Ok(ToolOutput {
            summary: format!("{} 重启成功", service),
            raw: Some(json!({"status": "restarted"})),
            control_flow: ToolControlFlow::Continue,
            truncation: None,
        })
    }
}

/// 通知管理员 —— 始终成功（重规划兜底）
struct NotifyAdminTool;

#[async_trait]
impl Tool for NotifyAdminTool {
    fn name(&self) -> &'static str { "notify_admin" }

    fn definition(&self) -> Value {
        json!({
            "type": "function",
            "function": {
                "name": "notify_admin",
                "description": "通知管理员",
                "parameters": {
                    "type": "object",
                    "properties": { "message": { "type": "string" } },
                    "required": ["message"]
                }
            }
        })
    }

    async fn call(&self, args: &Value, _ctx: &ToolContext) -> AgentResult<ToolOutput> {
        let msg = args["message"].as_str().unwrap_or("");
        Ok(ToolOutput {
            summary: format!("已通知: {}", msg),
            raw: Some(json!({"status": "sent"})),
            control_flow: ToolControlFlow::Continue,
            truncation: None,
        })
    }
}

// ============================================================================
// 确定性步骤执行器
// ============================================================================

struct DemoStepExecutor {
    tools: HashMap<String, Arc<dyn Tool + Send + Sync>>,
}

impl DemoStepExecutor {
    fn new(tools: Vec<Arc<dyn Tool + Send + Sync>>) -> Self {
        Self { tools: tools.into_iter().map(|t| (t.name().to_string(), t)).collect() }
    }
}

#[async_trait]
impl StepExecutor for DemoStepExecutor {
    async fn execute_step(&self, step: &PlanStep, _: &Value, ctx: &ToolContext) -> AgentResult<StepResult> {
        let tool_name = step.payload.get("tool_name").and_then(|v| v.as_str()).unwrap_or("unknown");
        let args = step.payload.get("args").cloned().unwrap_or(json!({}));
        let tool = self.tools.get(tool_name)
            .ok_or_else(|| AgentError::internal(format!("Tool not found: {tool_name}")))?;
        let output = tool.call(&args, ctx).await?;
        if output.summary.starts_with("ERROR") {
            Ok(StepResult::failure(&output.summary, 500))
        } else {
            Ok(StepResult::success(&output.summary, 200))
        }
    }
}

// ============================================================================
// 自定义恢复策略 —— 替代步骤 + 重规划
// ============================================================================

struct OpsRecoveryStrategy;

#[async_trait]
impl AdaptiveRecoveryStrategy for OpsRecoveryStrategy {
    async fn recover(&self, ctx: &RecoveryContext) -> AgentResult<RecoveryAction> {
        let tool = ctx.failed_step.payload.get("tool_name").and_then(|v| v.as_str()).unwrap_or("unknown");
        let service = ctx.failed_step.payload.get("args").and_then(|a| a.get("service")).and_then(|s| s.as_str()).unwrap_or("unknown");

        // Level 1: 替代步骤 —— 用 view_logs 替代 check_service
        if ctx.alternative_count < ctx.max_alternatives {
            let alt = match tool {
                "check_service" => PlanStep::tool_call(
                    format!("{}-alt-{}", ctx.failed_step.id, ctx.alternative_count + 1),
                    "通过查看日志间接确认服务状态",
                    "view_logs",
                    json!({"service": service, "lines": 20}),
                ),
                _ => PlanStep::tool_call(
                    format!("{}-alt-{}", ctx.failed_step.id, ctx.alternative_count + 1),
                    "通知管理员处理",
                    "notify_admin",
                    json!({"message": format!("Step {} failed: {}", ctx.failed_step.id, ctx.error)}),
                ),
            };
            return Ok(RecoveryAction::Alternative { step: alt, root_step_id: ctx.root_step_id.clone() });
        }

        // Level 2: 重规划 —— 简化为 "通知团队"
        if ctx.replan_count < ctx.max_replans {
            return Ok(RecoveryAction::Replan {
                steps: vec![PlanStep::tool_call(
                    "replan-notify", "通知运维团队",
                    "notify_admin",
                    json!({"message": format!("服务 {} 异常，自动恢复失败: {}", service, ctx.error)}),
                )],
                clear_future_phases: true,
            });
        }

        Ok(RecoveryAction::Abort)
    }
}

// ============================================================================
// Mock LLM —— 用于 LlmAdaptiveRecovery 场景
// ============================================================================

struct RecoveryMockLlm {
    call_count: Mutex<usize>,
}

impl RecoveryMockLlm {
    fn new() -> Self { Self { call_count: Mutex::new(0) } }
}

#[async_trait]
impl LlmClient for RecoveryMockLlm {
    async fn chat(&self, messages: &[ChatMessage], _: &[Value], _: Option<&agent_base::ReasoningConfig>, _: Option<&agent_base::ResponseFormat>) -> AgentResult<Value> {
        let system = messages.iter().find_map(|m| match m { ChatMessage::System { content } => Some(content.as_str()), _ => None }).unwrap_or("");
        let mut count = self.call_count.lock().unwrap();
        *count += 1;

        // 计划生成
        if system.contains("task planner") {
            return Ok(json!({
                "steps": [
                    {"id": "s1", "description": "检查 nginx 状态", "tool_name": "check_service", "args": {"service": "nginx"}},
                    {"id": "s2", "description": "重启 nginx", "tool_name": "restart_service", "args": {"service": "nginx"}}
                ]
            }).to_string().into());
        }

        // LlmAdaptiveRecovery 请求替代方案
        if system.contains("ALTERNATIVE") || system.contains("step recovery") {
            return Ok(json!({
                "id": "s1-alt",
                "description": "通过查看日志间接确认",
                "tool_name": "view_logs",
                "args": {"service": "nginx", "lines": 20}
            }).to_string().into());
        }

        // LlmAdaptiveRecovery 请求重规划
        if system.contains("replanning") || system.contains("replan") {
            return Ok(json!([{
                "id": "replan-notify",
                "description": "通知运维团队",
                "tool_name": "notify_admin",
                "args": {"message": "nginx 异常，自动恢复失败"}
            }]).to_string().into());
        }

        Ok(json!("Task completed.").to_string().into())
    }

    async fn chat_stream(&self, messages: &[ChatMessage], tools: &[Value], reasoning: Option<&agent_base::ReasoningConfig>, response_format: Option<&agent_base::ResponseFormat>)
        -> AgentResult<Pin<Box<dyn futures_core::Stream<Item = AgentResult<StreamChunk>> + Send>>>
    {
        let response = self.chat(messages, tools, reasoning, response_format).await?;
        let text = response.as_str().unwrap_or("").to_string();
        Ok(Box::pin(futures_util::stream::iter(vec![
            Ok(StreamChunk::Text(text)),
            Ok(StreamChunk::Stop),
        ])))
    }

    fn capabilities(&self) -> LlmCapabilities {
        LlmCapabilities { supports_streaming: true, supports_tools: true, ..Default::default() }
    }
}

// ============================================================================
// 事件处理
// ============================================================================

fn handle_event(label: &str, event: &RuntimeEvent) {
    match event {
        RuntimeEvent::PlanStepStarted { step_id, step_description, .. } => {
            println!("  [{label}] ▶  [{step_id}] {step_description}");
        }
        RuntimeEvent::PlanStepCompleted { step_id, success, result, .. } => {
            let icon = if *success { "✅" } else { "❌" };
            println!("  [{label}] {icon}  [{step_id}] {}", result.as_deref().unwrap_or("-"));
        }
        RuntimeEvent::StepRetry { step_id, retry_count, backoff_ms, .. } => {
            println!("  [{label}] 🔁 [{step_id}] 重试 #{retry_count} (退避 {backoff_ms}ms)");
        }
        RuntimeEvent::StepAlternativeTrying { original_step_id, alternative_step_id, alternative_count, .. } => {
            println!("  [{label}] 🔄 [{original_step_id}] 替代 #{alternative_count}: {alternative_step_id}");
        }
        RuntimeEvent::PlanReplanned { plan_id, new_steps, .. } => {
            println!("  [{label}] 📝 [{plan_id}] 重规划: {new_steps} 个新步骤");
        }
        RuntimeEvent::PlanRecoveryExhausted { step_id, retries, alternatives, replans, .. } => {
            println!("  [{label}] 🛑 [{step_id}] 恢复耗尽 (retry={retries}, alt={alternatives}, replan={replans})");
        }
        RuntimeEvent::PlanCompleted { plan_id, success, .. } => {
            let icon = if *success { "🎉" } else { "💥" };
            println!("  [{label}] {icon} [{plan_id}] {}", if *success { "完成" } else { "失败" });
        }
        _ => {}
    }
}

// ============================================================================
// 辅助：构建 executor 和工具集
// ============================================================================

fn make_tools(max_fails: usize) -> (Arc<AtomicUsize>, Vec<Arc<dyn Tool + Send + Sync>>) {
    let counter = Arc::new(AtomicUsize::new(0));
    let tools: Vec<Arc<dyn Tool + Send + Sync>> = vec![
        Arc::new(CheckServiceTool { fail_count: counter.clone(), max_fails }),
        Arc::new(ViewLogsTool),
        Arc::new(RestartServiceTool),
        Arc::new(NotifyAdminTool),
    ];
    (counter, tools)
}

// ============================================================================
// 主流程
// ============================================================================

#[tokio::main]
async fn main() -> AgentResult<()> {
    println!("╔══════════════════════════════════════════════════════════════╗");
    println!("║  Plan 恢复策略全览 (Mock LLM, 无需 API Key)                  ║");
    println!("╚══════════════════════════════════════════════════════════════╝\n");

    let llm = Arc::new(RecoveryMockLlm::new());

    // ── 场景 1: Level 0 — 框架自动重试 ──
    println!("═══════════════════════════════════════════════════════════");
    println!("  场景 1: Level 0 — 框架自动重试");
    println!("  check_service 前 2 次失败 → max_retries=3 → 第 3 次成功");
    println!("═══════════════════════════════════════════════════════════\n");

    let (counter, tools) = make_tools(2);
    let runtime = AgentBuilder::new(llm.clone())
        .register_tool(CheckServiceTool { fail_count: counter.clone(), max_fails: 2 })
        .register_tool(ViewLogsTool)
        .register_tool(RestartServiceTool)
        .register_tool(NotifyAdminTool)
        .build()?;

    let plan = ExecutionPlan {
        id: "retry-demo".into(), objective: "检查 nginx".into(),
        phases: vec![PlanPhase::new("p1", "检查", vec![
            PlanStep::tool_call("s1", "检查 nginx 状态", "check_service", json!({"service": "nginx"})),
        ])],
        status: PlanStatus::Created, context: json!({}),
    };

    let outcome = runtime.run_plan(
        runtime.create_session().await, plan,
        PlanConfig::new().with_executor(Arc::new(DemoStepExecutor::new(tools))).recovery(Recovery::retry(3)),
        |e| { handle_event("重试", &e); Ok(()) },
    ).await?;

    println!("  结果: {}\n", match &outcome { RunOutcome::Completed => "✅ 成功", _ => "❌ 失败" });

    // ── 场景 2: Level 1 — 替代步骤 ──
    println!("═══════════════════════════════════════════════════════════");
    println!("  场景 2: Level 1 — 替代步骤");
    println!("  check_service 始终失败 → 重试耗尽 → 替代 view_logs → 成功");
    println!("═══════════════════════════════════════════════════════════\n");

    let (counter2, tools2) = make_tools(99);
    let runtime2 = AgentBuilder::new(llm.clone())
        .register_tool(CheckServiceTool { fail_count: counter2.clone(), max_fails: 99 })
        .register_tool(ViewLogsTool)
        .register_tool(RestartServiceTool)
        .register_tool(NotifyAdminTool)
        .build()?;

    let plan2 = ExecutionPlan {
        id: "alt-demo".into(), objective: "诊断 nginx".into(),
        phases: vec![PlanPhase::new("p1", "诊断", vec![
            PlanStep::tool_call("s1", "检查 nginx 状态", "check_service", json!({"service": "nginx"})),
        ])],
        status: PlanStatus::Created, context: json!({}),
    };

    let outcome2 = runtime2.run_plan(
        runtime2.create_session().await, plan2,
        PlanConfig::new()
            .with_executor(Arc::new(DemoStepExecutor::new(tools2)))
            .max_retries(1).max_alternatives(2)
            .adaptive_recovery(Arc::new(OpsRecoveryStrategy)),
        |e| { handle_event("替代", &e); Ok(()) },
    ).await?;

    println!("  结果: {}\n", match &outcome2 { RunOutcome::Completed => "✅ 成功", _ => "❌ 失败" });

    // ── 场景 3: Level 2 — 重规划 ──
    println!("═══════════════════════════════════════════════════════════");
    println!("  场景 3: Level 2 — 重规划");
    println!("  check_service 始终失败 → 重试耗尽 → 替代耗尽 → 重规划 → 成功");
    println!("═══════════════════════════════════════════════════════════\n");

    let (counter3, tools3) = make_tools(99);
    let runtime3 = AgentBuilder::new(llm.clone())
        .register_tool(CheckServiceTool { fail_count: counter3.clone(), max_fails: 99 })
        .register_tool(ViewLogsTool)
        .register_tool(RestartServiceTool)
        .register_tool(NotifyAdminTool)
        .build()?;

    let plan3 = ExecutionPlan {
        id: "replan-demo".into(), objective: "诊断 nginx".into(),
        phases: vec![PlanPhase::new("p1", "诊断", vec![
            PlanStep::tool_call("s1", "检查 nginx 状态", "check_service", json!({"service": "nginx"})),
        ])],
        status: PlanStatus::Created, context: json!({}),
    };

    let outcome3 = runtime3.run_plan(
        runtime3.create_session().await, plan3,
        PlanConfig::new()
            .with_executor(Arc::new(DemoStepExecutor::new(tools3)))
            .max_retries(1).max_alternatives(1).max_replans(1)
            .adaptive_recovery(Arc::new(OpsRecoveryStrategy)),
        |e| { handle_event("重规划", &e); Ok(()) },
    ).await?;

    println!("  结果: {}\n", match &outcome3 { RunOutcome::Completed => "✅ 成功", _ => "❌ 失败" });

    // ── 场景 4: LlmAdaptiveRecovery — LLM 驱动恢复 ──
    println!("═══════════════════════════════════════════════════════════");
    println!("  场景 4: LlmAdaptiveRecovery — LLM 驱动恢复");
    println!("  LLM 全程参与：生成计划 + 决定恢复策略");
    println!("═══════════════════════════════════════════════════════════\n");

    let counter4 = Arc::new(AtomicUsize::new(0));
    let runtime4 = AgentBuilder::new(llm.clone())
        .register_tool(CheckServiceTool { fail_count: counter4.clone(), max_fails: 2 })
        .register_tool(ViewLogsTool)
        .register_tool(RestartServiceTool)
        .register_tool(NotifyAdminTool)
        .build()?;

    let outcome4 = runtime4.run_plan_with_generator(
        runtime4.create_session().await,
        "检查 nginx 服务状态，如果不正常则重启",
        Arc::new(LlmPlanGenerator::new(llm.clone()).with_max_steps(5)),
        PlanConfig::new()
            .max_retries(1).max_alternatives(2).max_replans(1)
            .adaptive_recovery(Arc::new(LlmAdaptiveRecovery::new(llm.clone()))),
        |e| { handle_event("LLM恢复", &e); Ok(()) },
    ).await?;

    println!("  结果: {}\n", match &outcome4 { RunOutcome::Completed => "✅ 成功", _ => "❌ 失败" });

    // ── 恢复管线速览 ──
    println!("═══════════════════════════════════════════════════════════");
    println!("  恢复管线速览");
    println!("═══════════════════════════════════════════════════════════\n");
    println!("  Level 0: max_retries(N)        — 框架自动重试，线性退避");
    println!("  Level 1: max_alternatives(N)   — 替代步骤（换工具/换参数）");
    println!("  Level 2: max_replans(N)        — 重规划（重新编排剩余步骤）");
    println!("  Level 3: Recovery::abort/skip  — 兜底策略\n");
    println!("  两种恢复策略实现:");
    println!("    自定义: impl AdaptiveRecoveryStrategy  — 注入领域知识");
    println!("    LLM:   LlmAdaptiveRecovery::new(llm)  — 开箱即用\n");
    println!("  配置示例:");
    println!("    PlanConfig::new()");
    println!("        .max_retries(2).max_alternatives(2).max_replans(1)");
    println!("        .adaptive_recovery(Arc::new(LlmAdaptiveRecovery::new(llm)))");

    Ok(())
}
