//! Plan 快速入门 —— Mock LLM，零依赖，5 分钟上手
//!
//! 三种调用方式：
//!   1. run_plan              — 手动构建计划，直接执行
//!   2. run_plan_with_generator — 自动生成计划 + 执行（一步完成）
//!   3. generate_plan → 审查 → run_plan — 生成与执行解耦
//!
//! 运行: cargo run --example plan_quickstart_demo

use std::sync::Arc;

use agent_base::{
    AgentBuilder, AgentResult, ExecutionPlan, LlmCapabilities, LlmClient,
    PlanConfig, PlanGenerator, PlanStep, Recovery, ResponseFormat, RuntimeEvent,
    StepExecutor, StepResult, StreamChunk, Tool, ToolContext, ToolControlFlow, ToolOutput,
    ChatMessage,
};
use async_trait::async_trait;
use serde_json::{json, Value};

// ============================================================================
// 工具
// ============================================================================

struct GreetTool;

#[async_trait]
impl Tool for GreetTool {
    fn name(&self) -> &'static str { "greet" }

    fn definition(&self) -> Value {
        json!({
            "type": "function",
            "function": {
                "name": "greet",
                "description": "生成问候语",
                "parameters": {
                    "type": "object",
                    "properties": {
                        "name": { "type": "string", "description": "要问候的名字" }
                    },
                    "required": ["name"]
                }
            }
        })
    }

    async fn call(&self, args: &Value, _ctx: &ToolContext) -> AgentResult<ToolOutput> {
        let name = args["name"].as_str().unwrap_or("World");
        Ok(ToolOutput {
            summary: format!("Hello, {}!", name),
            raw: Some(json!({ "greeting": format!("Hello, {}!", name) })),
            control_flow: ToolControlFlow::Continue,
            truncation: None,
        })
    }
}

// ============================================================================
// 确定性步骤执行器 —— 根据 step.payload 中的 tool_name 调用工具
// ============================================================================

struct DemoStepExecutor {
    tools: Vec<Arc<dyn Tool + Send + Sync>>,
}

impl DemoStepExecutor {
    fn new(tools: Vec<Arc<dyn Tool + Send + Sync>>) -> Self {
        Self { tools }
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
        let tool_name = step.payload.get("tool_name").and_then(|v| v.as_str()).unwrap_or("unknown");
        let args = step.payload.get("args").cloned().unwrap_or(json!({}));

        let tool = self.tools.iter().find(|t| t.name() == tool_name)
            .ok_or_else(|| agent_base::AgentError::internal(format!("Tool not found: {tool_name}")))?;

        let output = tool.call(&args, ctx).await?;
        Ok(StepResult::success(output.summary, 200))
    }
}

// ============================================================================
// Mock LLM —— 按顺序返回预设的流式响应
// ============================================================================

struct MockLlmClient;

#[async_trait]
impl LlmClient for MockLlmClient {
    async fn chat(&self, _: &[ChatMessage], _: &[Value], _: Option<&agent_base::ReasoningConfig>, _: Option<&ResponseFormat>) -> AgentResult<Value> {
        unimplemented!()
    }

    async fn chat_stream(&self, _: &[ChatMessage], _: &[Value], _: Option<&agent_base::ReasoningConfig>, _: Option<&ResponseFormat>)
        -> AgentResult<std::pin::Pin<Box<dyn futures_core::Stream<Item = AgentResult<StreamChunk>> + Send>>>
    {
        // 返回一个简单的计划 JSON
        let plan = json!({
            "steps": [
                {"id": "s1", "description": "问候 Alice"},
                {"id": "s2", "description": "问候 Bob"}
            ]
        });
        Ok(Box::pin(futures_util::stream::iter(vec![
            Ok(StreamChunk::Text(plan.to_string())),
            Ok(StreamChunk::Stop),
        ])))
    }

    fn capabilities(&self) -> LlmCapabilities {
        LlmCapabilities { supports_streaming: true, supports_tools: true, ..Default::default() }
    }
}

// ============================================================================
// 自定义 PlanGenerator —— 规则生成，不依赖 LLM
// ============================================================================

struct RulePlanGenerator;

#[async_trait]
impl PlanGenerator for RulePlanGenerator {
    async fn generate_plan(&self, objective: &str, _: &str, _: &[Value], _: Option<tokio::sync::mpsc::UnboundedSender<RuntimeEvent>>) -> AgentResult<ExecutionPlan> {
        let steps = if objective.contains("团队") {
            vec![
                PlanStep::tool_call("s1", "问候用户", "greet", json!({"name": "User"})),
                PlanStep::tool_call("s2", "问候团队", "greet", json!({"name": "Team"})),
                PlanStep::tool_call("s3", "问候领导", "greet", json!({"name": "Boss"})),
            ]
        } else {
            vec![PlanStep::tool_call("s1", "问候用户", "greet", json!({"name": "User"}))]
        };
        Ok(ExecutionPlan::of_steps("rule-plan", objective, steps))
    }
}

// ============================================================================
// 事件打印
// ============================================================================

fn print_event(event: &RuntimeEvent) {
    match event {
        RuntimeEvent::PlanGenerated { plan, .. } => {
            println!("  📋 计划生成: {} ({} 步)", plan.id, plan.total_steps());
            for step in plan.all_steps() {
                println!("     - {}: {}", step.id, step.description);
            }
        }
        RuntimeEvent::PlanStepStarted { step_id, step_description, .. } => {
            println!("  ▶️  [{step_id}] {step_description}");
        }
        RuntimeEvent::PlanStepCompleted { step_id, success, result, .. } => {
            let icon = if *success { "✅" } else { "❌" };
            println!("  {icon}  [{step_id}] {}", result.as_deref().unwrap_or("-"));
        }
        RuntimeEvent::PlanCompleted { success, .. } => {
            println!("  {} 计划{}\n", if *success { "🎉" } else { "⚠️" }, if *success { "完成" } else { "失败" });
        }
        _ => {}
    }
}

// ============================================================================
// 主流程
// ============================================================================

#[tokio::main]
async fn main() -> AgentResult<()> {
    println!("╔════════════════════════════════════════════════════════════╗");
    println!("║  Plan 快速入门 (Mock LLM, 无需 API Key)                   ║");
    println!("╚════════════════════════════════════════════════════════════╝\n");

    let llm = Arc::new(MockLlmClient);
    let runtime = AgentBuilder::new(llm).register_tool(GreetTool).build()?;
    let executor = Arc::new(DemoStepExecutor::new(vec![Arc::new(GreetTool)]));

    // ── 方式一: run_plan ── 手动构建计划，直接执行
    println!("┌────────────────────────────────────────────────────────────┐");
    println!("│ 方式一: run_plan — 手动构建计划                            │");
    println!("│ 适用: 固定流程、SOP、确定性要求高的任务                    │");
    println!("└────────────────────────────────────────────────────────────\n");

    let plan = ExecutionPlan::of_steps("manual", "问候 Alice 和 Bob", vec![
        PlanStep::tool_call("s1", "问候 Alice", "greet", json!({"name": "Alice"})),
        PlanStep::tool_call("s2", "问候 Bob",   "greet", json!({"name": "Bob"})),
    ]);

    runtime.run_plan(
        runtime.create_session().await,
        plan,
        PlanConfig::new().with_executor(executor.clone()).recovery(Recovery::skip()),
        |e| { print_event(&e); Ok(()) },
    ).await?;

    // ── 方式二: run_plan_with_generator ── 自动生成 + 执行（一步完成）
    println!("┌────────────────────────────────────────────────────────────┐");
    println!("│ 方式二: run_plan_with_generator — 自动生成并执行          │");
    println!("│ 适用: LLM 根据目标自动规划，快速执行                      │");
    println!("└────────────────────────────────────────────────────────────\n");

    runtime.run_plan_with_generator(
        runtime.create_session().await,
        "问候整个团队",
        Arc::new(RulePlanGenerator),
        PlanConfig::new().with_executor(executor.clone()).recovery(Recovery::abort()),
        |e| { print_event(&e); Ok(()) },
    ).await?;

    // ── 方式三: generate_plan → 审查 → run_plan ── 生成与执行解耦
    println!("┌────────────────────────────────────────────────────────────┐");
    println!("│ 方式三: generate_plan → 审查 → run_plan                   │");
    println!("│ 适用: 需要人工审查、修改计划后再执行                      │");
    println!("└────────────────────────────────────────────────────────────\n");

    let mut plan = RulePlanGenerator.generate_plan("问候用户", "", &[], None).await?;
    println!("  📋 原始计划 ({} 步):", plan.total_steps());
    for step in plan.all_steps() {
        println!("     - {}: {}", step.id, step.description);
    }

    // 追加一个步骤
    if let Some(phase) = plan.phases.first_mut() {
        phase.steps.push(PlanStep::tool_call("s-extra", "额外问候", "greet", json!({"name": "Extra"})));
    }
    println!("  → 审查: 追加 1 步\n");

    runtime.run_plan(
        runtime.create_session().await,
        plan,
        PlanConfig::new().with_executor(executor).recovery(Recovery::abort()),
        |e| { print_event(&e); Ok(()) },
    ).await?;

    // ── Recovery 策略速览 ──
    println!("┌────────────────────────────────────────────────────────────┐");
    println!("│ Recovery 策略速览                                          │");
    println!("└────────────────────────────────────────────────────────────\n");
    println!("  Recovery::abort()       — 步骤失败则终止整个计划");
    println!("  Recovery::skip()        — 步骤失败则跳过，继续后续");
    println!("  Recovery::retry(N)      — 步骤失败重试最多 N 次");
    println!("  Recovery::custom(...)   — 自定义逻辑\n");
    println!("  更多恢复策略 → plan_recovery_demo");
    println!("  LLM 工具流程 → plan_exec_tool_demo");
    println!("  生产环境特性 → plan_advanced_demo");

    Ok(())
}
