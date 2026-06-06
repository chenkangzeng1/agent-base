//! Plan 基础入门示例 —— 零外部依赖，Mock LLM 演示核心 API
//!
//! 本示例展示 Plan 的三种调用方式：
//!   1. run_plan           — 执行预定义的手动计划
//!   2. run_plan_with_generator — LLM 自动生成计划并执行（一步完成）
//!   3. generate_plan + run_plan — 先生成计划，审查后再执行（解耦）
//!
//! 使用 MockLlmClient，无需配置 API Key，直接运行：
//!   cargo run --example plan_basic_demo

use std::sync::Arc;
use std::sync::Mutex;

use agent_base::{
    AgentBuilder, AgentResult, ExecutionPlan, InMemoryPlanStore,
    LlmCapabilities, LlmClient, PlanConfig, PlanGenerator, PlanStep,
    PlanStore, Recovery, ResponseFormat, RuntimeEvent, StepExecutor, StepResult,
    StreamChunk, Tool, ToolContext, ToolControlFlow, ToolOutput, ChatMessage,
};
use async_trait::async_trait;
use serde_json::{json, Value};

// ============================================================================
// 1. 定义一个简单工具
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
// 2. Mock LLM —— 无需网络，按顺序返回预设的流式响应
// ============================================================================

struct MockLlmClient {
    responses: Mutex<Vec<Vec<StreamChunk>>>,
    call_count: Mutex<usize>,
}

impl MockLlmClient {
    fn new(responses: Vec<Vec<StreamChunk>>) -> Self {
        Self {
            responses: Mutex::new(responses),
            call_count: Mutex::new(0),
        }
    }
}

#[async_trait]
impl LlmClient for MockLlmClient {
    async fn chat(
        &self,
        _messages: &[ChatMessage],
        _tools: &[Value],
        _reasoning: Option<&agent_base::ReasoningConfig>,
        _response_format: Option<&ResponseFormat>,
    ) -> AgentResult<Value> {
        unimplemented!("本示例只使用流式接口")
    }

    async fn chat_stream(
        &self,
        _messages: &[ChatMessage],
        _tools: &[Value],
        _reasoning: Option<&agent_base::ReasoningConfig>,
        _response_format: Option<&ResponseFormat>,
    ) -> AgentResult<std::pin::Pin<Box<dyn futures_core::Stream<Item = AgentResult<StreamChunk>> + Send>>> {
        let mut count = self.call_count.lock().unwrap();
        *count += 1;
        let mut responses = self.responses.lock().unwrap();
        let chunks = responses.remove(0);
        let stream = futures_util::stream::iter(chunks.into_iter().map(Ok));
        Ok(Box::pin(stream))
    }

    fn capabilities(&self) -> LlmCapabilities {
        LlmCapabilities {
            supports_streaming: true,
            supports_tools: true,
            supports_vision: false,
            supports_thinking: false,
            max_context_tokens: None,
            max_output_tokens: None,
        }
    }
}

// ============================================================================
// 3. 自定义 PlanGenerator —— 用规则生成计划（不依赖 LLM）
// ============================================================================

struct RuleBasedPlanGenerator;

#[async_trait]
impl PlanGenerator for RuleBasedPlanGenerator {
    async fn generate_plan(
        &self,
        objective: &str,
        _context: &str,
        _tools: &[Value],
        _on_event: Option<tokio::sync::mpsc::UnboundedSender<RuntimeEvent>>,
    ) -> AgentResult<ExecutionPlan> {
        // 根据目标关键词匹配不同的固定计划
        let steps = if objective.contains("团队") {
            vec![
                PlanStep::tool_call("s1", "问候用户", "greet", json!({"name": "User"})),
                PlanStep::tool_call("s2", "问候团队", "greet", json!({"name": "Team"})),
                PlanStep::tool_call("s3", "问候领导", "greet", json!({"name": "Boss"})),
            ]
        } else {
            vec![
                PlanStep::tool_call("s1", "问候用户", "greet", json!({"name": "User"})),
            ]
        };

        Ok(ExecutionPlan::of_steps("rule-plan", objective, steps))
    }
}

// ============================================================================
// 4. 自定义 StepExecutor —— 模拟步骤执行
// ============================================================================

struct SimpleStepExecutor;

#[async_trait]
impl StepExecutor for SimpleStepExecutor {
    async fn execute_step(
        &self,
        step: &PlanStep,
        _step_outputs: &Value,
        _ctx: &ToolContext,
    ) -> AgentResult<StepResult> {
        println!("    [执行] {} - {}", step.id, step.description);
        Ok(StepResult::success(format!("步骤 {} 完成", step.id), 100))
    }
}

// ============================================================================
// 5. 打印事件的辅助函数
// ============================================================================

fn print_event(event: &RuntimeEvent) {
    match event {
        RuntimeEvent::PlanGenerated { plan, .. } => {
            println!("  📋 计划已生成: {} ({} 步)", plan.id, plan.total_steps());
            for step in plan.all_steps() {
                println!("     - {}: {}", step.id, step.description);
            }
        }
        RuntimeEvent::PlanStepStarted { step_id, step_description, .. } => {
            println!("  ▶️  开始: {} - {}", step_id, step_description);
        }
        RuntimeEvent::PlanStepCompleted { step_id, success, result, .. } => {
            let icon = if *success { "✅" } else { "❌" };
            println!("  {} 完成: {} → {}", icon, step_id, result.as_deref().unwrap_or("-"));
        }
        RuntimeEvent::PlanCompleted { success, .. } => {
            if *success {
                println!("  🎉 计划执行成功\n");
            } else {
                println!("  ⚠️  计划执行失败\n");
            }
        }
        _ => {}
    }
}

// ============================================================================
// 6. 主流程：三种调用方式逐一演示
// ============================================================================

#[tokio::main]
async fn main() -> AgentResult<()> {
    println!("╔════════════════════════════════════════════════════════════╗");
    println!("║  Plan 基础入门示例                                         ║");
    println!("║  使用 Mock LLM，无需 API Key                               ║");
    println!("╚════════════════════════════════════════════════════════════\n");

    // 创建 Runtime（Mock LLM 只需要一个占位实例）
    let llm = Arc::new(MockLlmClient::new(vec![]));
    let runtime = AgentBuilder::new(llm)
        .register_tool(GreetTool)
        .build()?;

    let session_id = runtime.create_session().await;
    let executor = Arc::new(SimpleStepExecutor);
    let plan_store = Arc::new(InMemoryPlanStore::new());

    // ======================================================================
    // 方式一：run_plan —— 手动创建计划，直接执行
    // ======================================================================
    println!("┌────────────────────────────────────────────────────────────┐");
    println!("│ 方式一: run_plan —— 手动预定义计划                         │");
    println!("│                                                            │");
    println!("│ 适用场景：固定流程、SOP、对确定性要求高的任务              │");
    println!("└────────────────────────────────────────────────────────────\n");

    let manual_plan = ExecutionPlan::of_steps(
        "manual-plan",
        "手动问候 Alice 和 Bob",
        vec![
            PlanStep::tool_call("s1", "问候 Alice", "greet", json!({"name": "Alice"})),
            PlanStep::tool_call("s2", "问候 Bob",   "greet", json!({"name": "Bob"})),
        ],
    );

    println!("目标: {}\n", manual_plan.objective);

    runtime
        .run_plan(
            session_id.clone(),
            manual_plan,
            PlanConfig::new()
                .executor(executor.clone())
                .recovery(Recovery::skip())
                .store(plan_store.clone()),
            |event| { print_event(&event); Ok(()) },
        )
        .await?;

    // ======================================================================
    // 方式二：run_plan_with_generator —— 自动生成+执行（一步完成）
    // ======================================================================
    println!("┌────────────────────────────────────────────────────────────┐");
    println!("│ 方式二: run_plan_with_generator —— 自动生成并执行          │");
    println!("│                                                            │");
    println!("│ 适用场景：需要 LLM 根据目标自动规划，快速执行              │");
    println!("└────────────────────────────────────────────────────────────\n");

    let generator = Arc::new(RuleBasedPlanGenerator);

    println!("目标: 问候整个团队\n");

    runtime
        .run_plan_with_generator(
            session_id.clone(),
            "问候整个团队",
            generator,
            PlanConfig::new()
                .executor(executor.clone())
                .recovery(Recovery::abort())
                .store(plan_store.clone()),
            |event| { print_event(&event); Ok(()) },
        )
        .await?;

    // ======================================================================
    // 方式三：generate_plan + run_plan —— 生成与执行解耦
    // ======================================================================
    println!("┌────────────────────────────────────────────────────────────┐");
    println!("│ 方式三: generate_plan → 审查 → run_plan                   │");
    println!("│                                                            │");
    println!("│ 适用场景：需要人工审查、修改计划后再执行                   │");
    println!("└────────────────────────────────────────────────────────────\n");

    let generator = RuleBasedPlanGenerator;

    println!("目标: 问候用户\n");

    // 步骤 1：生成计划
    let mut plan = generator
        .generate_plan("问候用户", "", &[], None)
        .await?;

    println!("📋 原始计划 ({} 步):", plan.total_steps());
    for step in plan.all_steps() {
        println!("   - {}: {}", step.id, step.description);
    }

    // 步骤 2：审查并修改（示例：追加一个步骤）
    println!("\n→ 审查：追加问候步骤\n");
    if let Some(phase) = plan.phases.first_mut() {
        phase.steps.push(PlanStep::tool_call(
            "s-extra",
            "额外问候",
            "greet",
            json!({"name": "Extra"}),
        ));
    }
    println!("📋 修改后计划 ({} 步):", plan.total_steps());
    for step in plan.all_steps() {
        println!("   - {}: {}", step.id, step.description);
    }
    println!();

    // 步骤 3：执行修改后的计划
    runtime
        .run_plan(
            session_id.clone(),
            plan,
            PlanConfig::new()
                .executor(executor.clone())
                .recovery(Recovery::abort())
                .store(plan_store.clone()),
            |event| { print_event(&event); Ok(()) },
        )
        .await?;

    // ======================================================================
    // Recovery 策略说明
    // ======================================================================
    println!("┌────────────────────────────────────────────────────────────┐");
    println!("│ Recovery 策略说明                                          │");
    println!("└────────────────────────────────────────────────────────────\n");

    println!("  Recovery::abort()       — 任意步骤失败则终止整个计划");
    println!("  Recovery::skip()        — 步骤失败则跳过，继续后续步骤");
    println!("  Recovery::retry(3)      — 步骤失败则重试最多 3 次");
    println!("  Recovery::custom(...)   — 自定义重试/跳过/终止逻辑\n");

    // ======================================================================
    // 查询 PlanStore
    // ======================================================================
    println!("┌────────────────────────────────────────────────────────────┐");
    println!("│ PlanStore 查询结果                                         │");
    println!("└────────────────────────────────────────────────────────────\n");

    for plan_id in ["manual-plan", "rule-plan"] {
        match plan_store.load_plan(plan_id).await? {
            Some(data) => println!("  📦 {}: status={:?}", plan_id, data.plan.status),
            None => println!("  📦 {}: 未找到", plan_id),
        }
    }

    println!("\n=== Demo 结束 ===");
    Ok(())
}
