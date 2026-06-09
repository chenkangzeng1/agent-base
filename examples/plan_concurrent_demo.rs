//! 并发计划执行示例 —— 展示 AgentRuntime 的 Clone 能力
//!
//! 本次重构将 AgentRuntime 内部改为 Arc<PlanRunner>，使其可 Clone。
//! 本示例展示一个实用模式：克隆 runtime 后，在多个 tokio task 中并发执行独立计划。
//!
//! 场景：批量运维任务
//!   - 3 个独立的服务检查计划并发执行
//!   - 每个计划有自己的 session、步骤、恢复策略
//!   - 共享同一个 AgentRuntime（无需创建多个实例）
//!
//! 使用 MockLlmClient，无需 API Key：
//!   cargo run --example plan_concurrent_demo

use std::collections::HashMap;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;

use agent_base::{
    AgentBuilder, AgentError, AgentResult, ExecutionPlan, PlanConfig, PlanPhase, PlanStep,
    PlanStatus, Recovery, RunOutcome, RuntimeEvent, StepExecutor, StepResult, Tool, ToolContext,
    ToolControlFlow, ToolOutput,
};
use async_trait::async_trait;
use serde_json::{json, Value};

// ============================================================================
// Mock 工具 —— 模拟服务健康检查
// ============================================================================

/// 通用服务检查工具，按名称区分不同服务
struct ServiceCheckTool {
    /// 每个服务的调用计数器
    counters: Arc<std::sync::Mutex<HashMap<String, AtomicUsize>>>,
    /// 每个服务需要失败几次后才成功
    fail_thresholds: HashMap<String, usize>,
}

impl ServiceCheckTool {
    fn new(fail_thresholds: HashMap<String, usize>) -> Self {
        let counters = fail_thresholds
            .keys()
            .map(|k| (k.clone(), AtomicUsize::new(0)))
            .collect();
        Self {
            counters: Arc::new(std::sync::Mutex::new(counters)),
            fail_thresholds,
        }
    }
}

#[async_trait]
impl Tool for ServiceCheckTool {
    fn name(&self) -> &'static str {
        "check_service"
    }

    fn definition(&self) -> Value {
        json!({
            "type": "function",
            "function": {
                "name": "check_service",
                "description": "检查指定服务的运行状态",
                "parameters": {
                    "type": "object",
                    "properties": {
                        "service": { "type": "string", "description": "服务名称，如 nginx / redis / postgres" }
                    },
                    "required": ["service"]
                }
            }
        })
    }

    async fn call(&self, args: &Value, _ctx: &ToolContext) -> AgentResult<ToolOutput> {
        let service = args["service"].as_str().unwrap_or("unknown");
        let guard = self.counters.lock().unwrap();
        let counter = guard.get(service).unwrap();
        let call_num = counter.fetch_add(1, Ordering::SeqCst) + 1;
        let threshold = self.fail_thresholds.get(service).copied().unwrap_or(0);
        drop(guard);

        if call_num <= threshold {
            Ok(ToolOutput {
                summary: format!("ERROR: {} 连接超时 (call #{call_num})", service),
                raw: Some(json!({"service": service, "status": "error", "call_num": call_num})),
                control_flow: ToolControlFlow::Continue,
                truncation: None,
            })
        } else {
            Ok(ToolOutput {
                summary: format!("{} 运行正常 (call #{call_num})", service),
                raw: Some(json!({"service": service, "status": "healthy", "call_num": call_num})),
                control_flow: ToolControlFlow::Continue,
                truncation: None,
            })
        }
    }
}

// ============================================================================
// DemoStepExecutor —— 确定性步骤执行器
// ============================================================================

struct DemoStepExecutor {
    tools: HashMap<String, Arc<dyn Tool + Send + Sync>>,
}

impl DemoStepExecutor {
    fn new(tools: Vec<Arc<dyn Tool + Send + Sync>>) -> Self {
        let map = tools.into_iter().map(|t| (t.name().to_string(), t)).collect();
        Self { tools: map }
    }
}

#[async_trait]
impl StepExecutor for DemoStepExecutor {
    async fn execute_step(
        &self,
        step: &PlanStep,
        _step_outputs: &Value,
        ctx: &ToolContext,
    ) -> AgentResult<StepResult> {
        let tool_name = step
            .payload
            .get("tool_name")
            .and_then(|v| v.as_str())
            .unwrap_or("unknown");
        let args = step.payload.get("args").cloned().unwrap_or(json!({}));

        let tool = self
            .tools
            .get(tool_name)
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
// 事件处理
// ============================================================================

fn make_event_handler(label: String) -> impl Fn(RuntimeEvent) -> AgentResult<()> {
    move |event| {
        match &event {
            RuntimeEvent::PlanStepStarted {
                step_id,
                step_description,
                ..
            } => {
                println!("  [{label}] ▶  [{step_id}] {step_description}");
            }
            RuntimeEvent::PlanStepCompleted {
                step_id, success, result, ..
            } => {
                let icon = if *success { "✅" } else { "❌" };
                println!(
                    "  [{label}] {icon}  [{step_id}] {}",
                    result.as_deref().unwrap_or("-")
                );
            }
            RuntimeEvent::StepRetry {
                step_id,
                retry_count,
                backoff_ms,
                ..
            } => {
                println!(
                    "  [{label}] 🔁 [{step_id}] 重试 #{retry_count} (退避 {backoff_ms}ms)"
                );
            }
            RuntimeEvent::StepAlternativeTrying {
                original_step_id,
                alternative_step_id,
                alternative_count,
                ..
            } => {
                println!(
                    "  [{label}] 🔄 [{original_step_id}] 替代 #{alternative_count}: {alternative_step_id}"
                );
            }
            RuntimeEvent::PlanReplanned {
                plan_id, new_steps, ..
            } => {
                println!("  [{label}] 📝 [{plan_id}] 重规划完成: {new_steps} 个新步骤");
            }
            RuntimeEvent::PlanRecoveryExhausted {
                step_id,
                retries,
                alternatives,
                replans,
                ..
            } => {
                println!(
                    "  [{label}] 🛑 [{step_id}] 恢复耗尽 (retry={retries}, alt={alternatives}, replan={replans})"
                );
            }
            RuntimeEvent::PlanCompleted {
                plan_id, success, ..
            } => {
                let icon = if *success { "🎉" } else { "💥" };
                println!(
                    "  [{label}] {icon} [{plan_id}] {}",
                    if *success { "完成" } else { "失败" }
                );
            }
            _ => {}
        }
        Ok(())
    }
}

// ============================================================================
// 自定义恢复策略 —— 用 view_logs 替代失败的 check_service
// ============================================================================

struct ServiceRecoveryStrategy;

#[async_trait]
impl agent_base::AdaptiveRecoveryStrategy for ServiceRecoveryStrategy {
    async fn recover(
        &self,
        ctx: &agent_base::RecoveryContext,
    ) -> AgentResult<agent_base::RecoveryAction> {
        let tool = ctx
            .failed_step
            .payload
            .get("tool_name")
            .and_then(|v| v.as_str())
            .unwrap_or("unknown");

        // Level 1: 尝试替代步骤
        if ctx.alternative_count < ctx.max_alternatives {
            let alt_step = match tool {
                "check_service" => {
                    let service = ctx
                        .failed_step
                        .payload
                        .get("args")
                        .and_then(|a| a.get("service"))
                        .and_then(|s| s.as_str())
                        .unwrap_or("unknown");
                    PlanStep::tool_call(
                        format!("{}-alt-{}", ctx.failed_step.id, ctx.alternative_count + 1),
                        "通过查看日志间接确认服务状态",
                        "view_logs",
                        json!({"service": service, "lines": 20}),
                    )
                }
                _ => PlanStep::tool_call(
                    format!("{}-alt-{}", ctx.failed_step.id, ctx.alternative_count + 1),
                    "记录问题并通知管理员",
                    "notify_admin",
                    json!({"message": format!("Step {} failed: {}", ctx.failed_step.id, ctx.error)}),
                ),
            };
            return Ok(agent_base::RecoveryAction::Alternative {
                step: alt_step,
                root_step_id: ctx.root_step_id.clone(),
            });
        }

        // Level 2: 重规划
        if ctx.replan_count < ctx.max_replans {
            let new_steps = vec![PlanStep::tool_call(
                "replan-notify",
                "通知运维团队处理",
                "notify_admin",
                json!({"message": format!("服务异常，自动恢复失败: {}", ctx.error)}),
            )];
            return Ok(agent_base::RecoveryAction::Replan {
                steps: new_steps,
                clear_future_phases: true,
            });
        }

        Ok(agent_base::RecoveryAction::Abort)
    }
}

// ============================================================================
// 辅助工具 —— view_logs 和 notify_admin
// ============================================================================

struct ViewLogsTool;

#[async_trait]
impl Tool for ViewLogsTool {
    fn name(&self) -> &'static str {
        "view_logs"
    }

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
            raw: Some(json!({"service": service, "status": "ok"})),
            control_flow: ToolControlFlow::Continue,
            truncation: None,
        })
    }
}

struct NotifyAdminTool;

#[async_trait]
impl Tool for NotifyAdminTool {
    fn name(&self) -> &'static str {
        "notify_admin"
    }

    fn definition(&self) -> Value {
        json!({
            "type": "function",
            "function": {
                "name": "notify_admin",
                "description": "通知管理员",
                "parameters": {
                    "type": "object",
                    "properties": {
                        "message": { "type": "string" }
                    },
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
// 主流程
// ============================================================================

#[tokio::main]
async fn main() -> AgentResult<()> {
    println!("╔══════════════════════════════════════════════════════════════╗");
    println!("║  并发计划执行 —— AgentRuntime Clone 能力演示                 ║");
    println!("╚══════════════════════════════════════════════════════════════╝");
    println!();
    println!("核心能力: AgentRuntime 现在可以 Clone，支持多任务并发执行。");
    println!();
    println!("场景: 3 个独立服务的健康检查并发执行");
    println!("  - nginx:  前 1 次失败 → 重试后成功");
    println!("  - redis:  前 2 次失败 → 替代步骤(view_logs) 成功");
    println!("  - postgres: 始终成功");
    println!();

    // ── 构建共享的 Tool 实例 ──
    // 注意: 多个 executor 共享同一组 Tool（通过 Arc）
    let tool_check = Arc::new(ServiceCheckTool::new(
        vec![
            ("nginx".to_string(), 1),
            ("redis".to_string(), 2),
            ("postgres".to_string(), 0),
        ]
        .into_iter()
        .collect(),
    )) as Arc<dyn Tool + Send + Sync>;
    let tool_logs: Arc<dyn Tool + Send + Sync> = Arc::new(ViewLogsTool);
    let tool_notify: Arc<dyn Tool + Send + Sync> = Arc::new(NotifyAdminTool);
    let all_tools = vec![
        tool_check.clone(),
        tool_logs.clone(),
        tool_notify.clone(),
    ];

    // ── 构建 Runtime ──
    // 使用一个不会被调用的 Mock LLM（因为 executor 是确定性的）
    let llm = Arc::new(NoOpLlm);
    let runtime = AgentBuilder::new(llm)
        .register_tool(ServiceCheckTool::new(
            vec![
                ("nginx".to_string(), 1),
                ("redis".to_string(), 2),
                ("postgres".to_string(), 0),
            ]
            .into_iter()
            .collect(),
        ))
        .register_tool(ViewLogsTool)
        .register_tool(NotifyAdminTool)
        .build()?;

    // ── Clone runtime 用于并发任务 ──
    // 这是本次重构的关键能力：AgentRuntime 实现了 Clone
    let runtime_a = runtime.clone();
    let runtime_b = runtime.clone();
    let runtime_c = runtime;

    // ── 定义 3 个独立的计划 ──
    let plan_nginx = ExecutionPlan {
        id: "check-nginx".to_string(),
        objective: "检查 nginx 服务健康".to_string(),
        phases: vec![PlanPhase::new(
            "p1",
            "nginx 检查",
            vec![PlanStep::tool_call(
                "s1",
                "检查 nginx 状态",
                "check_service",
                json!({"service": "nginx"}),
            )],
        )],
        status: PlanStatus::Created,
        context: json!({}),
    };

    let plan_redis = ExecutionPlan {
        id: "check-redis".to_string(),
        objective: "检查 redis 服务健康".to_string(),
        phases: vec![PlanPhase::new(
            "p1",
            "redis 检查",
            vec![PlanStep::tool_call(
                "s1",
                "检查 redis 状态",
                "check_service",
                json!({"service": "redis"}),
            )],
        )],
        status: PlanStatus::Created,
        context: json!({}),
    };

    let plan_postgres = ExecutionPlan {
        id: "check-postgres".to_string(),
        objective: "检查 postgres 服务健康".to_string(),
        phases: vec![PlanPhase::new(
            "p1",
            "postgres 检查",
            vec![PlanStep::tool_call(
                "s1",
                "检查 postgres 状态",
                "check_service",
                json!({"service": "postgres"}),
            )],
        )],
        status: PlanStatus::Created,
        context: json!({}),
    };

    // ── 并发执行 ──
    println!("════════════════════════════════════════════════════════════");
    println!("  并发启动 3 个计划...");
    println!("═══════════════════════════════════════════════════════════\n");

    let tools_for_a = all_tools.clone();
    let tools_for_b = all_tools.clone();
    let tools_for_c = all_tools.clone();

    let handle_a = tokio::spawn(async move {
        let session = runtime_a.create_session().await;
        let executor = Arc::new(DemoStepExecutor::new(tools_for_a));
        let strategy = Arc::new(ServiceRecoveryStrategy);

        runtime_a
            .run_plan(
                session,
                plan_nginx,
                PlanConfig::new()
                    .with_executor(executor)
                    .max_retries(2)
                    .max_alternatives(1)
                    .adaptive_recovery(strategy),
                make_event_handler("nginx".to_string()),
            )
            .await
    });

    let handle_b = tokio::spawn(async move {
        let session = runtime_b.create_session().await;
        let executor = Arc::new(DemoStepExecutor::new(tools_for_b));
        let strategy = Arc::new(ServiceRecoveryStrategy);

        runtime_b
            .run_plan(
                session,
                plan_redis,
                PlanConfig::new()
                    .with_executor(executor)
                    .max_retries(1)
                    .max_alternatives(2)
                    .adaptive_recovery(strategy),
                make_event_handler("redis".to_string()),
            )
            .await
    });

    let handle_c = tokio::spawn(async move {
        let session = runtime_c.create_session().await;
        let executor = Arc::new(DemoStepExecutor::new(tools_for_c));

        runtime_c
            .run_plan(
                session,
                plan_postgres,
                PlanConfig::new()
                    .with_executor(executor)
                    .max_retries(2)
                    .recovery(Recovery::skip()),
                make_event_handler("postgres".to_string()),
            )
            .await
    });

    // ── 等待所有任务完成 ──
    let (result_a, result_b, result_c) = tokio::join!(handle_a, handle_b, handle_c);

    println!();
    println!("════════════════════════════════════════════════════════════");
    println!("  结果汇总");
    println!("═══════════════════════════════════════════════════════════\n");

    for (name, result) in [("nginx", result_a), ("redis", result_b), ("postgres", result_c)] {
        match result {
            Ok(Ok(RunOutcome::Completed)) => println!("  ✅ {name}: 计划完成"),
            Ok(Ok(RunOutcome::Failed { error })) => println!("  ❌ {name}: {error}"),
            Ok(Err(e)) => println!("  💥 {name}: 运行时错误: {e}"),
            Err(e) => println!("  💥 {name}: task panic: {e}"),
            _ => println!("  ⚠️  {name}: 其他结果"),
        }
    }

    println!();
    println!("════════════════════════════════════════════════════════════");
    println!("  架构说明");
    println!("═══════════════════════════════════════════════════════════\n");
    println!("  AgentRuntime 内部结构:");
    println!("    AgentRuntime {{ runner: Arc<PlanRunner> }}");
    println!("    PlanRunner {{ config, llm_engine, tool_engine, session_manager, event_bus, ... }}");
    println!();
    println!("  Clone 语义:");
    println!("    runtime.clone() → 只 clone Arc 指针，不复制内部状态");
    println!("    多个 clone 共享同一个 PlanRunner 实例");
    println!("    session_manager 内部用 DashMap，天然线程安全");
    println!();
    println!("  适用场景:");
    println!("    - Web 服务中每个请求 clone runtime 处理独立任务");
    println!("    - 批量运维: 多台服务器并发巡检");
    println!("    - Pipeline: 多阶段任务并发执行");

    Ok(())
}

/// 不会真正被调用的 LLM（因为使用了确定性 executor）
struct NoOpLlm;

#[async_trait]
impl agent_base::LlmClient for NoOpLlm {
    async fn chat(
        &self,
        _messages: &[agent_base::ChatMessage],
        _tools: &[Value],
        _reasoning: Option<&agent_base::ReasoningConfig>,
        _response_format: Option<&agent_base::ResponseFormat>,
    ) -> AgentResult<Value> {
        Err(AgentError::internal("NoOpLlm should not be called"))
    }

    async fn chat_stream(
        &self,
        _messages: &[agent_base::ChatMessage],
        _tools: &[Value],
        _reasoning: Option<&agent_base::ReasoningConfig>,
        _response_format: Option<&agent_base::ResponseFormat>,
    ) -> AgentResult<
        std::pin::Pin<
            Box<dyn futures_core::Stream<Item = AgentResult<agent_base::StreamChunk>> + Send>,
        >,
    > {
        Err(AgentError::internal("NoOpLlm should not be called"))
    }

    fn capabilities(&self) -> agent_base::LlmCapabilities {
        agent_base::LlmCapabilities {
            supports_streaming: true,
            supports_tools: true,
            supports_vision: false,
            supports_thinking: false,
            max_context_tokens: None,
            max_output_tokens: None,
        }
    }
}
