//! Plan 高级示例 —— 真实 LLM，完整工作流演示
//!
//! 本示例展示基于真实 LLM 的三种 Plan 调用场景：
//!   1. run_plan_with_generator — 一步完成：LLM 生成计划并立即执行
//!   2. generate_plan + 审查修改 + run_plan — 解耦生成与执行
//!   3. run_plan — 手动创建计划并执行（作为基准对比）
//!
//! 额外展示：审批策略、计时统计、PlanStore 持久化查询
//!
//! 运行方式：
//!   cp .env.example .env   # 填写 OPENAI_API_KEY
//!   cargo run --example plan_advanced_demo

use std::sync::Arc;
use std::time::Instant;

use agent_base::{
    AgentBuilder, AgentError, AgentResult, ExecutionPlan, InMemoryPlanStore,
    LlmPlanGenerator, OpenAiClient, PlanConfig, PlanGenerator, PlanOptions, PlanStep, PlanStore,
    Recovery, RuntimeEvent, Tool, ToolContext, ToolControlFlow, ToolOutput,
    ApprovalRequest, ToolPolicy, RiskLevel, ApprovalHandler, ApprovalDecision,
};
use async_trait::async_trait;
use dotenvy::dotenv;
use serde_json::{json, Value};

// ============================================================================
// 计时工具
// ============================================================================

struct Timer {
    start: Instant,
    phases: Vec<(&'static str, u128)>,
    phase_start: Instant,
}

impl Timer {
    fn new() -> Self {
        let now = Instant::now();
        Self { start: now, phases: Vec::new(), phase_start: now }
    }

    fn mark(&mut self, label: &'static str) {
        self.phases.push((label, self.phase_start.elapsed().as_millis()));
        self.phase_start = Instant::now();
    }

    fn summary(&self, label: &str) {
        println!("\n  ┌─ 耗时统计: {}", label);
        for (name, ms) in &self.phases {
            println!("  │  {:<12} {:>6}ms", name, ms);
        }
        println!("  │  ─────────────────────");
        println!("  │  合计:        {:>6}ms", self.start.elapsed().as_millis());
        println!("  └─");
    }
}

// ============================================================================
// 业务工具定义
// ============================================================================

struct DiskCheckTool;

#[async_trait]
impl Tool for DiskCheckTool {
    fn name(&self) -> &'static str { "check_disk" }

    fn definition(&self) -> Value {
        json!({
            "type": "function",
            "function": {
                "name": "check_disk",
                "description": "检查服务器磁盘使用情况",
                "parameters": {
                    "type": "object",
                    "properties": {
                        "path": { "type": "string", "description": "路径如 '/'、'/home'" }
                    },
                    "required": ["path"]
                }
            }
        })
    }

    async fn call(&self, args: &Value, _ctx: &ToolContext) -> AgentResult<ToolOutput> {
        let path = args["path"].as_str().unwrap_or("/");
        Ok(ToolOutput {
            summary: format!("{} 磁盘: 总计 100G, 已用 68G, 可用 32G", path),
            raw: Some(json!({"path": path, "total_gb": 100, "used_gb": 68})),
            control_flow: ToolControlFlow::Continue,
            truncation: None,
        })
    }
}

struct MemCheckTool;

#[async_trait]
impl Tool for MemCheckTool {
    fn name(&self) -> &'static str { "check_memory" }

    fn definition(&self) -> Value {
        json!({
            "type": "function",
            "function": {
                "name": "check_memory",
                "description": "检查服务器内存使用情况",
                "parameters": { "type": "object", "properties": {} }
            }
        })
    }

    async fn call(&self, _args: &Value, _ctx: &ToolContext) -> AgentResult<ToolOutput> {
        Ok(ToolOutput {
            summary: "内存: 总计 32G, 已用 18G, 可用 14G".into(),
            raw: Some(json!({"total_gb": 32, "used_gb": 18})),
            control_flow: ToolControlFlow::Continue,
            truncation: None,
        })
    }
}

struct CheckProcessTool;

#[async_trait]
impl Tool for CheckProcessTool {
    fn name(&self) -> &'static str { "check_process" }

    fn definition(&self) -> Value {
        json!({
            "type": "function",
            "function": {
                "name": "check_process",
                "description": "检查指定进程是否正在运行",
                "parameters": {
                    "type": "object",
                    "properties": {
                        "name": { "type": "string", "description": "进程名称如 'nginx'" }
                    },
                    "required": ["name"]
                }
            }
        })
    }

    async fn call(&self, args: &Value, _ctx: &ToolContext) -> AgentResult<ToolOutput> {
        let name = args["name"].as_str().unwrap_or("unknown");
        let running = name != "nginx"; // 模拟 nginx 未运行
        Ok(ToolOutput {
            summary: format!("进程 '{}' {}", name, if running { "正在运行" } else { "未运行" }),
            raw: Some(json!({"process": name, "running": running})),
            control_flow: ToolControlFlow::Continue,
            truncation: None,
        })
    }
}

struct RestartServiceTool;

#[async_trait]
impl Tool for RestartServiceTool {
    fn name(&self) -> &'static str { "restart_service" }

    fn definition(&self) -> Value {
        json!({
            "type": "function",
            "function": {
                "name": "restart_service",
                "description": "重启指定服务（敏感操作，需审批）",
                "parameters": {
                    "type": "object",
                    "properties": {
                        "service": { "type": "string", "description": "服务名称如 'nginx'" }
                    },
                    "required": ["service"]
                }
            }
        })
    }

    async fn call(&self, args: &Value, _ctx: &ToolContext) -> AgentResult<ToolOutput> {
        let service = args["service"].as_str().unwrap_or("unknown");
        Ok(ToolOutput {
            summary: format!("服务 '{}' 已成功重启", service),
            raw: Some(json!({"service": service, "status": "restarted"})),
            control_flow: ToolControlFlow::Continue,
            truncation: None,
        })
    }
}

// ============================================================================
// 审批策略：只有 restart_service 需要审批
// ============================================================================

struct ServerCheckPolicy;

#[async_trait]
impl ToolPolicy for ServerCheckPolicy {
    async fn evaluate_approval(&self, tool_name: &str, args: &Value) -> Option<ApprovalRequest> {
        if tool_name == "restart_service" {
            let service = args.get("service").and_then(|v| v.as_str()).unwrap_or("unknown");
            Some(ApprovalRequest {
                title: "重启服务".into(),
                message: format!("确认重启服务 '{}'? 这将导致短暂服务中断", service),
                risk_level: RiskLevel::Sensitive,
                action_key: Some(format!("restart:{}", service)),
                raw: None,
            })
        } else {
            None
        }
    }

    fn before_call(&self, _tool_name: &str, _args: &Value, _ctx: &ToolContext) -> AgentResult<()> {
        Ok(())
    }

    fn after_call(&self, _tool_name: &str, _args: &Value, _result: &ToolOutput, _ctx: &ToolContext) -> AgentResult<()> {
        Ok(())
    }
}

struct AutoApprove;

#[async_trait]
impl ApprovalHandler for AutoApprove {
    async fn approve(&self, request: ApprovalRequest) -> AgentResult<ApprovalDecision> {
        println!("\n  ⚠️  审批请求: {} (risk: {:?})", request.title, request.risk_level);
        println!("     {}", request.message);
        println!("     → 自动放行 (开发模式)");
        Ok(ApprovalDecision::AllowOnce)
    }
}

// ============================================================================
// 事件打印辅助
// ============================================================================

fn print_event(event: &RuntimeEvent) {
    match event {
        RuntimeEvent::PlanGenerating { .. } => println!("  🤖 LLM 正在生成计划..."),
        RuntimeEvent::PlanGenerated { plan, .. } => {
            println!("  📋 计划已生成: {} ({} 步)", plan.id, plan.total_steps());
            for step in plan.all_steps() {
                let tool = step.payload.get("tool_name").and_then(|v| v.as_str()).unwrap_or("-");
                println!("     - {}: {} [tool={}]", step.id, step.description, tool);
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
            if *success { println!("  🎉 计划执行成功\n"); }
            else { println!("  ⚠️  计划执行失败\n"); }
        }
        RuntimeEvent::ToolCallStarted { tool_name, args_json, .. } => {
            println!("    🔧 [tool] {} args={}", tool_name, args_json);
        }
        RuntimeEvent::ToolCallFinished { tool_name, summary, .. } => {
            let short = if summary.len() > 100 { format!("{}...", &summary[..100]) } else { summary.clone() };
            println!("    📤 [tool] {} → {}", tool_name, short);
        }
        _ => {}
    }
}

// ============================================================================
// 主流程
// ============================================================================

#[tokio::main]
async fn main() -> AgentResult<()> {
    dotenv().ok();

    let api_key = std::env::var("OPENAI_API_KEY")
        .map_err(|_| AgentError::internal("请设置 OPENAI_API_KEY 环境变量"))?;
    let model = std::env::var("OPENAI_MODEL").unwrap_or_else(|_| "gpt-4o-mini".to_string());
    let base_url = std::env::var("OPENAI_BASE_URL")
        .unwrap_or_else(|_| "https://api.openai.com/v1".to_string());

    println!("╔════════════════════════════════════════════════════════════╗");
    println!("║  Plan 高级示例 —— 真实 LLM                                 ║");
    println!("╚════════════════════════════════════════════════════════════\n");
    println!("Model: {model}");
    println!("Base : {base_url}\n");

    let llm_client = Arc::new(OpenAiClient::new(api_key, model, Some(base_url)));

    let mut runtime = AgentBuilder::new(llm_client.clone())
        .register_tool(DiskCheckTool)
        .register_tool(MemCheckTool)
        .register_tool(CheckProcessTool)
        .register_tool(RestartServiceTool)
        .tool_policy(Arc::new(ServerCheckPolicy))
        .approval_handler(Arc::new(AutoApprove))
        .build()?;

    let session_id = runtime.create_session().await;
    let plan_store = Arc::new(InMemoryPlanStore::new());
    let step_executor = Arc::new(runtime.create_step_executor());
    let tool_defs: Vec<Value> = runtime.tools_mut().blocking_read().definitions();

    let objective = "检查服务器健康状况: 依次检查磁盘、内存、nginx 进程状态, 如果 nginx 不在运行则重启 nginx";

    // ======================================================================
    // 场景 A（推荐）: Agentic 模式 —— 计划只含描述，执行时 LLM 决定工具
    // ======================================================================
    println!("┌────────────────────────────────────────────────────────────┐");
    println!("│ 场景 A（推荐）: Agentic 模式 —— 计划只管「做什么」       │");
    println!("│                                                            │");
    println!("│ 核心: LLM 只生成步骤描述，不指定工具。执行时每步调 LLM，  │");
    println!("│      由 LLM 根据当前上下文动态选择最合适的工具和参数。     │");
    println!("│ 优势: 简单灵活、容错性强，工具变化不影响计划质量           │");
    println!("│ 配置: 自定义 system prompt + PlanConfig::new()（无 executor）│");
    println!("└────────────────────────────────────────────────────────────\n");

    // 注意：新的 API 设计中，框架会自动在 prompt 末尾追加步数约束
    // 用户只需关注业务逻辑，不需要写 {max_steps} 占位符
    // 框架会自动检测语言并追加相应语言的约束
    const PLAN_GENERATOR_PROMPT: &str = r#"You are a task planner. Given an objective, break it down into sequential steps."#;

    let mut timer_a = Timer::new();

    // 示例：使用自定义 prompt（不包含 {max_steps} 占位符）
    // 框架会自动在末尾追加："Note: Generate at most 5 steps."
    runtime
        .run_plan_with_generator(
            session_id.clone(),
            objective,
            Arc::new(LlmPlanGenerator::new(llm_client.clone())
                .with_system_prompt(PLAN_GENERATOR_PROMPT)
                .with_max_steps(5)),
            PlanConfig::new()
                // 注意: 不设置 executor，框架自动使用 Agentic 模式
                .recovery(Recovery::abort())
                .store(plan_store.clone()),
            |event| {
                print_event(&event);
                Ok(())
            },
        )
        .await?;

    timer_a.mark("Agentic 模式完成");
    timer_a.summary("场景 A: Agentic（推荐）");

    // ======================================================================
    // 场景 B（高级）: Deterministic 模式 —— 计划指定工具，执行直接调用
    // ======================================================================
    println!("\n┌────────────────────────────────────────────────────────────┐");
    println!("│ 场景 B（高级）: Deterministic 模式 —— 计划指定具体工具   │");
    println!("│                                                            │");
    println!("│ 核心: LLM 生成计划时指定每步的 tool_name 和 args，执行时  │");
    println!("│      直接调工具，零 LLM 开销。                             │");
    println!("│ 优势: 速度快、成本低，适合高频任务和固定 SOP              │");
    println!("│ 配置: 默认 system prompt + PlanConfig::new().with_executor(...) │");
    println!("└────────────────────────────────────────────────────────────\n");

    let mut timer_b = Timer::new();

    // 示例：使用默认 prompt（不包含 {max_steps} 占位符）
    // 框架会自动在 prompt 末尾追加步数约束
    // 实际发送给 LLM 的 prompt 会是：
    // "You are a task planner...Keep steps atomic and ordered.
    //
    // Note: Generate at most 5 steps."
    runtime
        .run_plan_with_generator(
            session_id.clone(),
            objective,
            Arc::new(LlmPlanGenerator::new(llm_client.clone()).with_max_steps(5)),
            PlanConfig::new()
                .with_executor(step_executor.clone())
                .recovery(Recovery::abort())
                .store(plan_store.clone()),
            |event| {
                print_event(&event);
                Ok(())
            },
        )
        .await?;

    timer_b.mark("Deterministic 模式完成");
    timer_b.summary("场景 B: Deterministic（高级）");

    // ======================================================================
    // 场景 C: generate_plan → 审查修改 → run_plan —— 解耦生成与执行
    // ======================================================================
    println!("\n┌────────────────────────────────────────────────────────────┐");
    println!("│ 场景 C: generate_plan → 审查修改 → run_plan —— 解耦     │");
    println!("│                                                            │");
    println!("│ 特点: 生成后可人工审查、修改、保存，再决定是否执行         │");
    println!("└────────────────────────────────────────────────────────────\n");

    let mut timer_c = Timer::new();

    // 步骤 1: 生成计划
    let generator = LlmPlanGenerator::new(llm_client.clone()).with_max_steps(5);
    let mut plan = generator
        .generate_plan(objective, "", &tool_defs, None)
        .await
        .map_err(|e| AgentError::plan_generation(e.to_string()))?;

    timer_c.mark("LLM 生成计划");

    println!("📋 原始计划 ({} 步):", plan.total_steps());
    for step in plan.all_steps() {
        let tool = step.payload.get("tool_name").and_then(|v| v.as_str()).unwrap_or("-");
        println!("   - {}: {} [tool={}]", step.id, step.description, tool);
    }

    // 步骤 2: 审查并追加验证步骤
    println!("\n→ 审查: 追加 nginx 状态验证步骤\n");
    if let Some(phase) = plan.phases.first_mut() {
        phase.steps.push(PlanStep::tool_call(
            "verify-nginx",
            "验证 nginx 重启后状态",
            "check_process",
            json!({"name": "nginx"}),
        ));
    }

    // 保存到 PlanStore
    plan_store.save_plan(&plan, json!({"source": "llm_generated", "modified": true})).await?;
    println!("📋 修改后计划 ({} 步)，已保存到 plan_store\n", plan.total_steps());

    timer_c.mark("审查修改");

    // 步骤 3: 执行
    runtime
        .run_plan(
            session_id.clone(),
            plan,
            PlanConfig::new()
                .with_executor(step_executor.clone())
                .recovery(Recovery::abort())
                .store(plan_store.clone()),
            |event| { print_event(&event); Ok(()) },
        )
        .await?;

    timer_c.mark("执行计划");
    timer_c.summary("场景 C: 解耦生成+执行");

    // ======================================================================
    // 场景 D: generate_plan_with_options —— 运行时覆盖 max_steps
    // ======================================================================
    println!("\n┌────────────────────────────────────────────────────────────┐");
    println!("│ 场景 D: generate_plan_with_options —— 运行时覆盖配置     │");
    println!("│                                                            │");
    println!("│ 特点: 同一个 generator，运行时可临时覆盖 max_steps 等参数 │");
    println!("│ 优势: 适合多 plan 场景，每个 plan 可以有不同的步数限制     │");
    println!("└────────────────────────────────────────────────────────────\n");

    let mut timer_d = Timer::new();

    // 创建一个默认 max_steps=10 的 generator
    let generator = LlmPlanGenerator::new(llm_client.clone())
        .with_max_steps(10);

    // 使用 generate_plan_with_options 临时覆盖为 3 步
    let options = PlanOptions { max_steps: Some(3) };
    let plan = generator
        .generate_plan_with_options(objective, "", &tool_defs, options, None)
        .await
        .map_err(|e| AgentError::plan_generation(e.to_string()))?;

    timer_d.mark("生成计划 (max_steps=3)");

    println!("📋 计划 ({} 步，运行时指定 max_steps=3):", plan.total_steps());
    for step in plan.all_steps() {
        println!("   - {}: {}", step.id, step.description);
    }

    // 执行这个精简计划
    runtime
        .run_plan(
            session_id.clone(),
            plan,
            PlanConfig::new()
                .recovery(Recovery::abort())
                .store(plan_store.clone()),
            |event| { print_event(&event); Ok(()) },
        )
        .await?;

    timer_d.mark("执行计划");
    timer_d.summary("场景 D: 运行时覆盖");

    // ======================================================================
    // 场景 E: 手动创建计划 → run_plan —— 基准对比
    // ======================================================================
    println!("\n┌────────────────────────────────────────────────────────────┐");
    println!("│ 场景 E: 手动创建计划 → run_plan —— 零 LLM 开销          │");
    println!("│                                                            │");
    println!("│ 特点: 不用 LLM 生成，完全手动指定步骤，适合固定 SOP        │");
    println!("└────────────────────────────────────────────────────────────\n");

    let mut timer_e = Timer::new();

    let manual_plan = ExecutionPlan::of_steps(
        "manual-check",
        "手动检查磁盘和内存",
        vec![
            PlanStep::tool_call("s1", "检查磁盘 /",      "check_disk",    json!({"path": "/"})),
            PlanStep::tool_call("s2", "检查磁盘 /home",  "check_disk",    json!({"path": "/home"})),
            PlanStep::tool_call("s3", "检查内存",        "check_memory",  json!({})),
        ],
    );

    runtime
        .run_plan(
            session_id.clone(),
            manual_plan,
            PlanConfig::new()
                .with_executor(step_executor)
                .recovery(Recovery::skip())
                .store(plan_store.clone()),
            |event| { print_event(&event); Ok(()) },
        )
        .await?;

    timer_e.mark("执行完成");
    timer_e.summary("场景 E: 手动计划");

    // ======================================================================
    // PlanStore 查询
    // ======================================================================
    println!("\n┌────────────────────────────────────────────────────────────┐");
    println!("│ PlanStore 查询结果                                         │");
    println!("└────────────────────────────────────────────────────────────");

    for plan_id in ["manual-check"] {
        match plan_store.load_plan(plan_id).await? {
            Some(data) => println!("  📦 {}: status={:?}", plan_id, data.plan.status),
            None => println!("  📦 {}: 未找到", plan_id),
        }
    }

    println!("\n=== Demo 结束 ===");
    Ok(())
}
