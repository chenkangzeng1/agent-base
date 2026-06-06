//! LlmPlanGenerator Demo — 使用 LLM 自动生成并执行计划
//!
//! 展示了：
//!   1. 从 .env 读取 LLM 配置
//!   2. 定义业务工具（磁盘检查、内存检查、进程检查、服务重启）
//!   3. 用 LlmPlanGenerator 让 LLM 根据目标自动生成执行计划
//!   4. 用 ToolCallingStepExecutor 逐个执行计划步骤
//!   5. 事件回调打印进度
//!   6. 对比：手动创建计划和自动生成计划
//!
//! 运行方式：
//!   cp .env.example .env   # 填写 OPENAI_API_KEY 等配置
//!   cargo run --example llm_plan_demo

use std::sync::Arc;

use agent_base::{
    AgentBuilder, AgentError, AgentResult, ExecutionPlan, InMemoryPlanStore,
    LlmPlanGenerator, OpenAiClient, PlanConfig, PlanStep, PlanStore, Recovery, RuntimeEvent,
    Tool, ToolContext, ToolControlFlow, ToolOutput, ApprovalRequest, ToolPolicy, RiskLevel,
    ApprovalHandler, ApprovalDecision,
};
use async_trait::async_trait;
use dotenvy::dotenv;
use serde_json::{json, Value};

// ============================================================================
// 业务工具定义
// ============================================================================

/// 检查磁盘使用情况
struct DiskCheckTool;

#[async_trait]
impl Tool for DiskCheckTool {
    fn name(&self) -> &'static str { "check_disk" }

    fn definition(&self) -> Value {
        json!({
            "type": "function",
            "function": {
                "name": "check_disk",
                "description": "检查服务器磁盘使用情况，返回已用/总空间和使用率",
                "parameters": {
                    "type": "object",
                    "properties": {
                        "path": {
                            "type": "string",
                            "description": "要检查的文件系统路径，如 '/'、'/home'"
                        }
                    },
                    "required": ["path"]
                }
            }
        })
    }

    async fn call(&self, args: &Value, _ctx: &ToolContext) -> AgentResult<ToolOutput> {
        let path = args["path"].as_str().unwrap_or("/");
        Ok(ToolOutput {
            summary: format!("{} 磁盘: 总计 100G, 已用 68G, 可用 32G (使用率 68%)", path),
            raw: Some(json!({"path": path, "total_gb": 100, "used_gb": 68, "available_gb": 32, "percent": 68})),
            control_flow: ToolControlFlow::Continue,
            truncation: None,
        })
    }
}

/// 检查内存使用情况
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
                "parameters": {
                    "type": "object",
                    "properties": {}
                }
            }
        })
    }

    async fn call(&self, _args: &Value, _ctx: &ToolContext) -> AgentResult<ToolOutput> {
        Ok(ToolOutput {
            summary: "内存: 总计 32G, 已用 18G, 可用 14G (使用率 56%), Swap: 总计 8G, 已用 0G".into(),
            raw: Some(json!({"total_gb": 32, "used_gb": 18, "percent": 56 })),
            control_flow: ToolControlFlow::Continue,
            truncation: None,
        })
    }
}

/// 检查指定进程是否运行
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
                        "name": {
                            "type": "string",
                            "description": "进程名称，如 'nginx', 'mysql', 'redis'"
                        }
                    },
                    "required": ["name"]
                }
            }
        })
    }

    async fn call(&self, args: &Value, _ctx: &ToolContext) -> AgentResult<ToolOutput> {
        let name = args["name"].as_str().unwrap_or("unknown");
        // 模拟: 除 sshd 外, nginx 是停的
        let (running, pid) = match name {
            "nginx"  => (false, 0),
            "sshd"   => (true,  1024),
            _        => (true,  2048),
        };
        let summary = if running {
            format!("进程 '{}' 正在运行 (PID: {})", name, pid)
        } else {
            format!("进程 '{}' 未运行", name)
        };
        Ok(ToolOutput {
            summary,
            raw: Some(json!({"process": name, "running": running, "pid": pid})),
            control_flow: ToolControlFlow::Continue,
            truncation: None,
        })
    }
}

/// 重启服务（敏感操作，需要审批）
struct RestartServiceTool;

#[async_trait]
impl Tool for RestartServiceTool {
    fn name(&self) -> &'static str { "restart_service" }

    fn definition(&self) -> Value {
        json!({
            "type": "function",
            "function": {
                "name": "restart_service",
                "description": "重启指定服务。此操作会导致短暂服务中断，需要审批",
                "parameters": {
                    "type": "object",
                    "properties": {
                        "service": {
                            "type": "string",
                            "description": "服务名称，如 'nginx', 'mysql'"
                        }
                    },
                    "required": ["service"]
                }
            }
        })
    }

    async fn call(&self, args: &Value, _ctx: &ToolContext) -> AgentResult<ToolOutput> {
        let service = args["service"].as_str().unwrap_or("unknown");
        Ok(ToolOutput {
            summary: format!("服务 '{}' 已成功重启，状态: active (running)", service),
            raw: Some(json!({"service": service, "status": "restarted", "success": true})),
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
    async fn evaluate_approval(
        &self,
        tool_name: &str,
        args: &Value,
    ) -> Option<ApprovalRequest> {
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
            // 读操作自动放行
            None
        }
    }

    fn before_call(&self, _tool_name: &str, _args: &Value, _ctx: &ToolContext) -> AgentResult<()> {
        Ok(())
    }

    fn after_call(
        &self,
        _tool_name: &str,
        _args: &Value,
        _result: &ToolOutput,
        _ctx: &ToolContext,
    ) -> AgentResult<()> {
        Ok(())
    }
}

/// 自动审批 handler — 开发环境简单放行
struct AutoApprove;

#[async_trait]
impl ApprovalHandler for AutoApprove {
    async fn approve(&self, request: ApprovalRequest) -> AgentResult<ApprovalDecision> {
        println!();
        println!("⚠️  审批请求: {}", request.title);
        println!("   risk: {:?}", request.risk_level);
        println!("   {}", request.message);
        println!("   → 自动放行 (开发模式)");
        Ok(ApprovalDecision::AllowOnce)
    }
}

// ============================================================================
// 主流程
// ============================================================================

#[tokio::main]
async fn main() -> AgentResult<()> {
    // 1. 加载 .env 配置
    dotenv().ok();

    let api_key = std::env::var("OPENAI_API_KEY")
        .map_err(|_| AgentError::internal("请设置 OPENAI_API_KEY 环境变量"))?;

    let model = std::env::var("OPENAI_MODEL")
        .unwrap_or_else(|_| "gpt-4o-mini".to_string());

    let base_url = std::env::var("OPENAI_BASE_URL")
        .unwrap_or_else(|_| "https://api.openai.com/v1".to_string());

    println!("=== LlmPlanGenerator Demo ===\n");
    println!("Model : {model}");
    println!("Base  : {base_url}\n");

    // 2. 创建 LLM 客户端
    let llm_client = Arc::new(OpenAiClient::new(api_key, model.clone(), Some(base_url)));

    // 3. 构建 Agent Runtime, 注册工具
    let runtime = AgentBuilder::new(llm_client.clone())
        .register_tool(DiskCheckTool)
        .register_tool(MemCheckTool)
        .register_tool(CheckProcessTool)
        .register_tool(RestartServiceTool)
        .tool_policy(Arc::new(ServerCheckPolicy))
        .approval_handler(Arc::new(AutoApprove))
        .build()?;

    let session_id = runtime.create_session().await;
    let plan_store = Arc::new(InMemoryPlanStore::new());

    // ======================================================================
    // 方式一: LlmPlanGenerator — LLM 根据目标自动生成计划并执行
    // ======================================================================
    println!("┌─────────────────────────────────────────────────────┐");
    println!("│ 方式一: LlmPlanGenerator 自动生成 + 执行            │");
    println!("└─────────────────────────────────────────────────────┘\n");

    let generator = Arc::new(LlmPlanGenerator::new(llm_client.clone())
        .with_max_steps(5)                    // 最多生成 5 个步骤
    );

    // create_step_executor 创建的 executor 通过 ToolRegistry 调用已注册的工具
    let step_executor = Arc::new(runtime.create_step_executor());

    println!("目标: 检查服务器健康状况, 如果 nginx 不在运行则重启\n");
    println!("--- LLM 正在生成计划... ---\n");

    let result = runtime
        .run_plan_with_generator(
            session_id.clone(),
            "检查服务器健康状况: 依次检查磁盘、内存、nginx 进程状态, 如果 nginx 不在运行则重启 nginx",
            generator,
            PlanConfig::new()
                .executor(step_executor.clone())
                .recovery(Recovery::abort())
                .store(plan_store.clone()),
            |event| {
                match event {
                    RuntimeEvent::PlanGenerating { plan_id, .. } => {
                        println!("🤖 LLM 正在生成计划 [{}] ...", plan_id);
                        Ok(())
                    }
                    RuntimeEvent::PlanGenerated { plan, .. } => {
                        println!("\n📋 计划已生成:");
                        println!("   id       : {}", plan.id);
                        println!("   objective: {}", plan.objective);
                        println!("   steps    : {}", plan.total_steps());
                        for step in plan.all_steps() {
                            let tool = step.payload.get("tool_name")
                                .and_then(|v| v.as_str())
                                .unwrap_or("(agentic)");
                            println!("     - {}: {} [tool={}]", step.id, step.description, tool);
                        }
                        println!();
                        Ok(())
                    }
                    RuntimeEvent::PlanStepStarted { step_id, step_description, .. } => {
                        println!("▶️  开始执行: {} - {}", step_id, step_description);
                        Ok(())
                    }
                    RuntimeEvent::PlanStepCompleted { step_id, success, result, .. } => {
                        let icon = if success { "✅" } else { "❌" };
                        let output = result.unwrap_or_default();
                        println!("{}  步骤完成: {} → {}", icon, step_id, output);
                        Ok(())
                    }
                    RuntimeEvent::PlanCompleted { success, .. } => {
                        if success { println!("\n🎉 所有步骤执行成功!\n"); }
                        else       { println!("\n⚠️  计划执行未完全成功\n"); }
                        Ok(())
                    }
                    RuntimeEvent::ToolCallStarted { tool_name, args_json, .. } => {
                        println!("  🔧 [tool] {} args={}", tool_name, args_json);
                        Ok(())
                    }
                    RuntimeEvent::ToolCallFinished { tool_name, summary, .. } => {
                        let short = if summary.len() > 120 {
                            format!("{}...", &summary[..120])
                        } else {
                            summary.clone()
                        };
                        println!("  📤 [tool] {} → {}", tool_name, short);
                        Ok(())
                    }
                    _ => Ok(())
                }
            },
        )
        .await?;

    println!("结果: {:?}\n", result);

    // ======================================================================
    // 方式二: 手动创建计划 → run_plan（对比）
    // ======================================================================
    println!("┌─────────────────────────────────────────────────────┐");
    println!("│ 方式二: 手动创建计划 + run_plan（对比）             │");
    println!("└─────────────────────────────────────────────────────┘\n");

    let manual_plan = ExecutionPlan::of_steps(
        "manual-check",
        "手动检查磁盘和内存",
        vec![
            PlanStep::tool_call("s1", "检查磁盘 /",  "check_disk",    json!({"path": "/"})),
            PlanStep::tool_call("s2", "检查磁盘 /home", "check_disk", json!({"path": "/home"})),
            PlanStep::tool_call("s3", "检查内存",      "check_memory", json!({})),
        ],
    );

    println!("目标: {}\n", manual_plan.objective);

    let result = runtime
        .run_plan(
            session_id.clone(),
            manual_plan,
            PlanConfig::new()
                .executor(step_executor)
                .recovery(Recovery::skip())    // 对比: 跳过失败步骤
                .store(plan_store.clone()),
            |event| {
                match event {
                    RuntimeEvent::PlanStepStarted { step_id, .. } => {
                        println!("▶️  {}", step_id);
                        Ok(())
                    }
                    RuntimeEvent::PlanStepCompleted { step_id, success, result, .. } => {
                        let icon = if success { "✅" } else { "❌" };
                        println!("{}   {} → {}", icon, step_id, result.unwrap_or_default());
                        Ok(())
                    }
                    _ => Ok(())
                }
            },
        )
        .await?;

    println!("\n结果: {:?}", result);

    // ======================================================================
    // 检查存储的计划
    // ======================================================================
    let stored = plan_store.load_plan("manual-check").await?;
    if let Some(data) = stored {
        println!("\n📦 plan_store 中 'manual-check' 的状态: {:?}", data.plan.status);
    }

    println!("\n=== Demo 完成 ===");
    Ok(())
}
