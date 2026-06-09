//! PlanExecTool 流程示例 —— create_plan → execute_plan 两步走
//!
//! 展示 LLM 通过两个工具完成复杂任务的完整流程：
//!   1. LLM 调用 `create_plan` 工具 → PlanOrchestrator 生成计划，存入 PlanStore
//!   2. LLM 调用 `execute_plan` 工具 → PlanExecTool 从 PlanStore 取出计划，通过 PlanRunner 执行
//!
//! 这是框架内置的标准工具对，适合需要「先规划、再执行」的场景。
//!
//! 运行:
//!   cargo run --example plan_exec_tool_demo

use std::collections::HashMap;
use std::pin::Pin;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};

use agent_base::{
    AgentBuilder, AgentError, AgentResult, ChatMessage, InMemoryPlanStore,
    LlmCapabilities, LlmClient, LlmPlanGenerator, PlanStep, PlanStore,
    Recovery, ResponseFormat, RunOutcome, RuntimeEvent, StepExecutor, StepResult, StreamChunk,
    Tool, ToolContext, ToolControlFlow, ToolOutput,
};
use async_trait::async_trait;
use serde_json::{json, Value};

// ============================================================================
// Mock 工具 —— 模拟运维操作
// ============================================================================

struct CheckServiceTool {
    fail_count: Arc<AtomicUsize>,
}

#[async_trait]
impl Tool for CheckServiceTool {
    fn name(&self) -> &'static str {
        "check_service"
    }

    fn definition(&self) -> Value {
        json!({
            "type": "function",
            "function": {
                "name": "check_service",
                "description": "检查服务状态",
                "parameters": {
                    "type": "object",
                    "properties": {
                        "service": { "type": "string" }
                    },
                    "required": ["service"]
                }
            }
        })
    }

    async fn call(&self, args: &Value, _ctx: &ToolContext) -> AgentResult<ToolOutput> {
        let service = args["service"].as_str().unwrap_or("unknown");
        let n = self.fail_count.fetch_add(1, Ordering::SeqCst) + 1;
        if n <= 1 {
            Ok(ToolOutput {
                summary: format!("ERROR: {} 连接超时", service),
                raw: Some(json!({"status": "error"})),
                control_flow: ToolControlFlow::Continue,
                truncation: None,
            })
        } else {
            Ok(ToolOutput {
                summary: format!("{} 运行正常", service),
                raw: Some(json!({"status": "healthy"})),
                control_flow: ToolControlFlow::Continue,
                truncation: None,
            })
        }
    }
}

struct RestartServiceTool;

#[async_trait]
impl Tool for RestartServiceTool {
    fn name(&self) -> &'static str {
        "restart_service"
    }

    fn definition(&self) -> Value {
        json!({
            "type": "function",
            "function": {
                "name": "restart_service",
                "description": "重启服务",
                "parameters": {
                    "type": "object",
                    "properties": {
                        "service": { "type": "string" }
                    },
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

// ============================================================================
// DemoStepExecutor —— 确定性执行器，根据 step.payload 调用工具
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
// MockLlmClient —— 模拟 LLM 的两阶段行为
//
// 阶段 1: LLM 看到用户任务，决定调用 create_plan
// 阶段 2: LLM 拿到计划结果，决定调用 execute_plan
// ============================================================================

struct MockLlmClient {
    /// 跟踪 LLM 被调用了几次
    call_count: Mutex<usize>,
}

impl MockLlmClient {
    fn text_chunk(text: &str) -> StreamChunk {
        StreamChunk::Text(text.to_string())
    }

    fn tool_call_chunk(id: &str, name: &str, args: &str) -> StreamChunk {
        StreamChunk::ToolCall(json!({
            "index": 0,
            "delta": {
                "tool_calls": [{
                    "index": 0,
                    "id": id,
                    "type": "function",
                    "function": { "name": name, "arguments": args }
                }]
            }
        }))
    }

    /// 从 Tool 消息中提取 plan_id
    fn extract_plan_id(messages: &[ChatMessage]) -> Option<String> {
        messages.iter().filter_map(|m| {
            if let ChatMessage::Tool { content, .. } = m {
                // 尝试从 JSON 提取
                if let Ok(v) = serde_json::from_str::<Value>(content) {
                    if let Some(pid) = v.get("plan_id").and_then(|p| p.as_str()) {
                        return Some(pid.to_string());
                    }
                }
                // 从文本提取 "plan_id: plan-xxx"
                content.split("plan_id:").nth(1)
                    .and_then(|s| s.trim().split_whitespace().next())
                    .map(|s| s.to_string())
            } else {
                None
            }
        }).next()
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
        unimplemented!()
    }

    async fn chat_stream(
        &self,
        messages: &[ChatMessage],
        tools: &[Value],
        _reasoning: Option<&agent_base::ReasoningConfig>,
        _response_format: Option<&ResponseFormat>,
    ) -> AgentResult<Pin<Box<dyn futures_core::Stream<Item = AgentResult<StreamChunk>> + Send>>>
    {
        let mut count = self.call_count.lock().unwrap();
        *count += 1;
        let call = *count;

        // 判断是否是计划生成调用（system prompt 包含 "task planner"）
        let is_plan_gen = messages.iter().any(|m| {
            matches!(m, ChatMessage::System { content } if content.contains("task planner"))
        });

        if is_plan_gen {
            // ── 计划生成：返回包含 tool_name 和 args 的计划 ──
            let plan_json = json!({
                "steps": [
                    {"id": "step-1", "description": "检查 nginx 服务状态", "tool_name": "check_service", "args": {"service": "nginx"}},
                    {"id": "step-2", "description": "重启 nginx 服务", "tool_name": "restart_service", "args": {"service": "nginx"}}
                ]
            });
            return Ok(Box::pin(futures_util::stream::iter(vec![
                Ok(Self::text_chunk(&plan_json.to_string())),
                Ok(StreamChunk::Stop),
            ])));
        }

        // ── 主 ReAct 循环 ──
        let tool_names: Vec<&str> = tools
            .iter()
            .filter_map(|t| {
                t.get("function")
                    .and_then(|f| f.get("name"))
                    .and_then(|n| n.as_str())
            })
            .collect();

        let has_create_plan = tool_names.iter().any(|&n| n == "create_plan");
        let has_execute_plan = tool_names.iter().any(|&n| n == "execute_plan");
        let plan_id = Self::extract_plan_id(messages);

        // 检查是否已经调用过 execute_plan
        let already_executed = messages.iter().any(|m| {
            matches!(m, ChatMessage::Assistant { tool_calls: Some(calls), .. }
                if calls.iter().any(|c| c.name == "execute_plan"))
        });

        let chunks = if call == 1 && has_create_plan {
            // 第 1 轮: 调用 create_plan
            vec![
                Self::text_chunk("这是复杂任务，先制定计划。\n"),
                Self::tool_call_chunk("call_create", "create_plan",
                    &json!({"objective": "检查 nginx 服务状态，如果不正常则重启", "context": "目标: prod-server-01"}).to_string()),
                StreamChunk::Stop,
            ]
        } else if has_execute_plan && plan_id.is_some() && !already_executed {
            // 有 plan_id 且还没执行过 → 调用 execute_plan
            let pid = plan_id.unwrap();
            vec![
                Self::text_chunk("计划已生成，开始执行。\n"),
                Self::tool_call_chunk("call_exec", "execute_plan",
                    &json!({"plan_id": pid}).to_string()),
                StreamChunk::Stop,
            ]
        } else {
            // 已经执行过或没有 plan_id → 返回最终结果
            vec![
                Self::text_chunk("任务完成。nginx 服务检查和修复流程已执行完毕。"),
                StreamChunk::Stop,
            ]
        };

        Ok(Box::pin(futures_util::stream::iter(chunks.into_iter().map(Ok))))
    }

    fn capabilities(&self) -> LlmCapabilities {
        LlmCapabilities {
            supports_streaming: true,
            supports_tools: true,
            supports_vision: false,
            supports_thinking: false,
            max_context_tokens: Some(8192),
            max_output_tokens: Some(4096),
        }
    }
}

// ============================================================================
// 事件处理
// ============================================================================

fn handle_event(event: &RuntimeEvent) {
    match event {
        RuntimeEvent::PlanStepParsed {
            step_id,
            step_description,
            ..
        } => {
            println!("   📝 解析步骤: [{step_id}] {step_description}");
        }
        RuntimeEvent::PlanStepStarted {
            step_id,
            step_description,
            ..
        } => {
            println!("   ▶  [{step_id}] {step_description}");
        }
        RuntimeEvent::PlanStepCompleted {
            step_id, success, result, ..
        } => {
            let icon = if *success { "✅" } else { "❌" };
            println!(
                "   {icon}  [{step_id}] {}",
                result.as_deref().unwrap_or("-")
            );
        }
        RuntimeEvent::StepRetry {
            step_id,
            retry_count,
            backoff_ms,
            ..
        } => {
            println!("   🔁 [{step_id}] 重试 #{retry_count} (退避 {backoff_ms}ms)");
        }
        RuntimeEvent::PlanCompleted {
            plan_id, success, ..
        } => {
            let icon = if *success { "🎉" } else { "💥" };
            println!(
                "   {icon} [{plan_id}] {}",
                if *success { "完成" } else { "失败" }
            );
        }
        _ => {}
    }
}

// ============================================================================
// 主流程
// ============================================================================

#[tokio::main]
async fn main() -> AgentResult<()> {
    println!("╔══════════════════════════════════════════════════════════════╗");
    println!("║  PlanExecTool 流程示例                                       ║");
    println!("║  create_plan → execute_plan 两步走                           ║");
    println!("╚══════════════════════════════════════════════════════════════╝");
    println!();
    println!("流程:");
    println!("  1. 用户告诉 LLM 一个复杂任务");
    println!("  2. LLM 调用 create_plan → PlanOrchestrator 生成计划，存入 PlanStore");
    println!("  3. LLM 调用 execute_plan → PlanExecTool 从 PlanStore 取计划，通过 PlanRunner 执行");
    println!("  4. 执行过程中自动重试（Level 0 恢复管线）");
    println!();

    // ── 共享组件 ──
    let plan_store = Arc::new(InMemoryPlanStore::new()) as Arc<dyn PlanStore>;
    let check_counter = Arc::new(AtomicUsize::new(0));

    // 注册实际的运维工具
    let tool_check: Arc<dyn Tool + Send + Sync> = Arc::new(CheckServiceTool {
        fail_count: check_counter.clone(),
    });
    let tool_restart: Arc<dyn Tool + Send + Sync> = Arc::new(RestartServiceTool);

    // 确定性执行器 —— PlanExecTool 用它来执行计划中的每一步
    let step_executor = Arc::new(DemoStepExecutor::new(vec![
        tool_check.clone(),
        tool_restart.clone(),
    ]));

    // LLM 计划生成器 —— PlanOrchestrator 用它来生成计划
    let llm_client: Arc<dyn LlmClient> = Arc::new(MockLlmClient {
        call_count: Mutex::new(0),
    });
    let plan_generator = Arc::new(LlmPlanGenerator::new(llm_client.clone()).with_max_steps(5));

    // ── 构造两个工具 ──
    // PlanOrchestrator: LLM 调用它来生成计划
    let orchestrator = agent_base::PlanOrchestrator::new(
        plan_generator,
        step_executor.clone(),
        plan_store.clone(),
    );

    // PlanExecTool: LLM 调用它来执行计划
    // 框架会在 build() 时自动注入 PlanRunner 引用
    let exec_tool = agent_base::PlanExecTool::new(
        step_executor.clone(),
        plan_store.clone(),
        Recovery::retry(2), // 失败自动重试 2 次
    );

    // ── 构建 Runtime ──
    let runtime = AgentBuilder::new(llm_client.clone())
        .register_tool_arc(tool_check)
        .register_tool_arc(tool_restart)
        .register_tool(orchestrator) // 注册 create_plan 工具
        .register_tool(exec_tool) // 注册 execute_plan 工具
        .build()?;

    // ── 注入用户消息，启动 ReAct 循环 ──
    let session_id = runtime.create_session().await;

    println!("════════════════════════════════════════════════════════════");
    println!("  用户: 检查 nginx 服务状态，如果不正常则重启");
    println!("═══════════════════════════════════════════════════════════\n");

    // 添加用户消息
    runtime
        .add_user_message(&session_id, "检查 nginx 服务状态，如果不正常则重启")
        .await?;

    // 运行 ReAct 循环 —— LLM 会自动调用 create_plan → execute_plan
    let outcome = runtime
        .run(session_id.clone(), |event| {
            handle_event(&event);
            Ok(())
        })
        .await?;

    println!();
    match &outcome {
        RunOutcome::Completed => println!("  ✅ 任务完成"),
        RunOutcome::Failed { error } => println!("  ❌ 失败: {error}"),
        _ => println!("  ⚠️  其他结果"),
    }

    // ── 查看 PlanStore 中的计划状态 ──
    println!();
    println!("════════════════════════════════════════════════════════════");
    println!("  PlanStore 中的计划");
    println!("═══════════════════════════════════════════════════════════\n");

    for plan_id in plan_store.list_plans().await? {
        if let Some(data) = plan_store.load_plan(&plan_id).await? {
            println!(
                "  📦 {}: status={:?}, steps={}",
                plan_id,
                data.plan.status,
                data.plan.total_steps()
            );
            for step in data.plan.all_steps() {
                println!("     - {}: {} [{:?}]", step.id, step.description, step.status);
            }
        }
    }

    println!();
    println!("════════════════════════════════════════════════════════════");
    println!("  流程总结");
    println!("═══════════════════════════════════════════════════════════\n");
    println!("  LLM 看到的工具:");
    println!("    1. check_service     — 检查服务状态");
    println!("    2. restart_service   — 重启服务");
    println!("    3. create_plan       — 生成执行计划（PlanOrchestrator）");
    println!("    4. execute_plan      — 执行计划（PlanExecTool）");
    println!();
    println!("  LLM 的决策过程:");
    println!("    → 这是复杂任务，先调 create_plan 生成计划");
    println!("    → 拿到 plan_id，再调 execute_plan 执行");
    println!("    → execute_plan 内部走 PlanRunner 的 4 级恢复管线");
    println!();
    println!("  关键设计:");
    println!("    PlanStore 是共享的 — create_plan 写入，execute_plan 读取");
    println!("    PlanExecTool 通过 OnceLock<Weak<PlanRunner>> 引用 PlanRunner");
    println!("    执行时自动注入 PlanRunner（build 时完成，无运行时开销）");

    Ok(())
}
