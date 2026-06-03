use std::sync::Arc;
use std::time::Instant;

use agent_base::{
    AgentBuilder, AgentResult, AllowAllApprovalHandler, ChatMessage, ExecutionPlan,
    InMemoryPlanStore, LlmClient, OpenAiClient, PlanGenerator, PlanOrchestrator, PlanStep,
    ReasoningConfig, RuntimeEvent, StepExecutor, StepResult, StreamChunk, ToolContext,
};
use async_trait::async_trait;
use futures_util::StreamExt;
use serde_json::{json, Value};

// ── helper: millisecond clock ──────────────────────────────────────────

fn now_ms(start: Instant) -> String {
    format!("{:>6}ms", start.elapsed().as_millis())
}

// ── step definition matching PlanStepResponse ──────────────────────────

#[derive(serde::Deserialize, Debug, Clone)]
struct PlanStepLine {
    id: String,
    description: String,
    command: String,
    host_id: String,
    #[serde(default)]
    depends_on: Vec<String>,
}

fn build_plan_prompt(objective: &str, context: &str) -> String {
    format!(
        r#"你是一个 Linux 运维专家，需要为以下目标生成执行计划。

目标：{}
上下文：{}

请生成详细的执行计划，要求：
1. 每个步骤只包含一个 SSH 命令
2. 考虑步骤间的依赖关系
3. 步骤应该是可幂等的（重复执行不会产生副作用）
4. 使用标准的 Linux 命令

请严格以 JSONL 格式输出，每行一个独立的 JSON 对象，不要包含 markdown 代码块或其他内容：

{{"objective": "任务目标"}}
{{"id": "step_1", "description": "步骤描述", "command": "具体命令", "host_id": "目标主机ID", "depends_on": []}}
{{"id": "step_2", "description": "步骤描述", "command": "具体命令", "host_id": "目标主机ID", "depends_on": ["step_1"]}}"#,
        objective, context
    )
}

// ── test 1: direct LLM stream → raw chunk timeline ─────────────────────

async fn test_direct_llm_stream(
    client: &OpenAiClient,
    label: &str,
    enable_thinking: bool,
) -> AgentResult<()> {
    println!("\n{}", "=".repeat(70));
    println!("TEST 1: {} | thinking={}", label, enable_thinking);
    println!("{}", "=".repeat(70));

    let prompt = build_plan_prompt("全面检查服务器健康状况", "目标主机: 121.41.191.236");
    let messages = vec![ChatMessage::user(&prompt)];
    let reasoning = ReasoningConfig {
        enabled: Some(enable_thinking),
        budget_tokens: if enable_thinking { Some(128) } else { None },
        effort: None,
    };

    let t0 = Instant::now();
    println!("{} sending LLM request (prompt_len={})", now_ms(t0), prompt.len());

    let mut stream = client
        .chat_stream(&messages, &[], Some(&reasoning), None)
        .await?;

    // Line buffer for JSONL parsing
    let mut line_buf = String::new();
    let mut step_count = 0usize;
    let mut text_chunks = 0usize;
    let mut thought_total = 0usize;
    let mut first_text = true;

    while let Some(chunk) = stream.next().await {
        match chunk? {
            StreamChunk::Thought(t) => {
                thought_total += t.len();
                if thought_total <= 200 {
                    print!(
                        "{} 💭 {} chars (total: {})",
                        now_ms(t0),
                        t.len(),
                        thought_total
                    );
                    if thought_total <= 80 {
                        println!(" | \"{}\"", &t[..t.len().min(60)]);
                    } else {
                        println!();
                    }
                }
            }
            StreamChunk::Text(text) => {
                if first_text {
                    println!(
                        "{} 📝 FIRST TEXT after {} thought chars",
                        now_ms(t0), thought_total,
                    );
                    first_text = false;
                }
                text_chunks += 1;

                line_buf.push_str(&text);

                while let Some(pos) = line_buf.find('\n') {
                    let line = line_buf[..pos].trim().to_string();
                    line_buf.drain(..=pos);

                    if line.is_empty() {
                        continue;
                    }

                    if let Ok(val) = serde_json::from_str::<Value>(&line) {
                        if let Some(obj) = val.get("objective").and_then(Value::as_str) {
                            println!("{} 🎯 objective: \"{}\"", now_ms(t0), obj);
                        } else if let Ok(step) = serde_json::from_str::<PlanStepLine>(&line) {
                            step_count += 1;
                            println!(
                                "{} ✅ step_{} [{}]: {} | cmd: {} | host: {}",
                                now_ms(t0),
                                step_count,
                                step.id,
                                step.description,
                                &step.command[..step.command.len().min(40)],
                                step.host_id,
                            );
                        }
                    }
                }
            }
            StreamChunk::Stop => {
                println!("{} 🛑 stop received", now_ms(t0));
            }
            StreamChunk::ToolCall(v) => {
                println!("{} 🔧 tool_call: {:?}", now_ms(t0), v);
            }
            StreamChunk::Usage(u) => {
                println!(
                    "{} 📊 usage: prompt={:?} completion={:?} total={:?}",
                    now_ms(t0),
                    u.prompt_tokens,
                    u.completion_tokens,
                    u.total_tokens,
                );
            }
        }
    }

    // Process any remaining text in buffer (last line without newline)
    if !line_buf.trim().is_empty() {
        let line = line_buf.trim().to_string();
        if let Ok(val) = serde_json::from_str::<Value>(&line) {
            if let Some(obj) = val.get("objective").and_then(Value::as_str) {
                println!("{} 🎯 objective (final): \"{}\"", now_ms(t0), obj);
            } else if let Ok(step) = serde_json::from_str::<PlanStepLine>(&line) {
                step_count += 1;
                println!(
                    "{} ✅ step_{} [{}]: {} (final)",
                    now_ms(t0), step_count, step.id, step.description,
                );
            }
        }
    }

    println!(
        "{} 📋 total: {} text chunks, {} thought chars, {} steps, {}ms elapsed",
        now_ms(t0),
        text_chunks,
        thought_total,
        step_count,
        t0.elapsed().as_millis(),
    );

    Ok(())
}

// ── test 2: through PlanOrchestrator trait dispatch ────────────────────

/// A PlanGenerator that uses OpenAiClient to call the LLM with JSONL prompt.
struct LlmPlanGenerator {
    client: Arc<OpenAiClient>,
    enable_thinking: bool,
}

fn parse_jsonl_response(text: &str) -> (Option<String>, Vec<PlanStepLine>) {
    let mut objective = None;
    let mut steps = Vec::new();
    for line in text.lines() {
        let line = line.trim();
        if line.is_empty() {
            continue;
        }
        if let Ok(val) = serde_json::from_str::<Value>(line) {
            if let Some(obj) = val.get("objective").and_then(Value::as_str) {
                if objective.is_none() {
                    objective = Some(obj.to_string());
                }
            } else if let Ok(step) = serde_json::from_str::<PlanStepLine>(line) {
                steps.push(step);
            }
        }
    }
    (objective, steps)
}

fn lines_to_plan(objective_str: String, steps: Vec<PlanStepLine>) -> ExecutionPlan {
    let plan_steps: Vec<PlanStep> = steps
        .into_iter()
        .map(|s| {
            PlanStep::new(
                s.id,
                s.description,
                json!({
                    "type": "ssh_command",
                    "command": s.command,
                    "host_id": s.host_id,
                }),
            )
            .with_dependencies(s.depends_on)
        })
        .collect();
    ExecutionPlan::with_single_phase("test-plan", objective_str, plan_steps)
}

#[async_trait]
impl PlanGenerator for LlmPlanGenerator {
    async fn generate_plan(
        &self,
        objective: &str,
        context: &str,
        _tools: &[Value],
    ) -> AgentResult<ExecutionPlan> {
        let prompt = build_plan_prompt(objective, context);
        let messages = vec![ChatMessage::user(&prompt)];
        let reasoning = ReasoningConfig {
            enabled: Some(self.enable_thinking),
            budget_tokens: if self.enable_thinking { Some(128) } else { None },
            effort: None,
        };

        let mut stream = self
            .client
            .chat_stream(&messages, &[], Some(&reasoning), None)
            .await?;

        let mut full_text = String::new();
        while let Some(chunk) = stream.next().await {
            match chunk? {
                StreamChunk::Text(t) => full_text.push_str(&t),
                StreamChunk::Stop => break,
                _ => {}
            }
        }

        let (obj_opt, steps) = parse_jsonl_response(&full_text);
        let objective_str = obj_opt.unwrap_or_else(|| objective.to_string());

        if steps.is_empty() {
            return Err(agent_base::AgentError::plan_generation(
                "no valid steps".to_string(),
            ));
        }

        Ok(lines_to_plan(objective_str, steps))
    }

    async fn generate_plan_streaming(
        &self,
        objective: &str,
        context: &str,
        _tools: &[Value],
        on_generating: Box<dyn Fn() + Send>,
        on_step_parsed: Box<dyn Fn(usize, String, String) + Send>,
        _on_thought: Box<dyn Fn(String) + Send>,
    ) -> AgentResult<ExecutionPlan> {
        let prompt = build_plan_prompt(objective, context);
        let messages = vec![ChatMessage::user(&prompt)];
        let reasoning = ReasoningConfig {
            enabled: Some(self.enable_thinking),
            budget_tokens: if self.enable_thinking { Some(128) } else { None },
            effort: None,
        };

        let mut stream = self
            .client
            .chat_stream(&messages, &[], Some(&reasoning), None)
            .await?;

        let mut line_buf = String::new();
        let mut first_chunk = true;
        let mut step_index = 0usize;
        let mut objective_opt: Option<String> = None;
        let mut all_steps: Vec<PlanStepLine> = Vec::new();

        while let Some(chunk) = stream.next().await {
            match chunk? {
                StreamChunk::Text(text) => {
                    if first_chunk {
                        first_chunk = false;
                        on_generating();
                    }
                    line_buf.push_str(&text);

                    while let Some(pos) = line_buf.find('\n') {
                        let line = line_buf[..pos].trim().to_string();
                        line_buf.drain(..=pos);

                        if line.is_empty() {
                            continue;
                        }

                        if let Ok(val) = serde_json::from_str::<Value>(&line) {
                            if let Some(obj) = val.get("objective").and_then(Value::as_str) {
                                if objective_opt.is_none() {
                                    objective_opt = Some(obj.to_string());
                                }
                            } else if let Ok(step) =
                                serde_json::from_str::<PlanStepLine>(&line)
                            {
                                on_step_parsed(
                                    step_index,
                                    step.id.clone(),
                                    step.description.clone(),
                                );
                                step_index += 1;
                                all_steps.push(step);
                            }
                        }
                    }
                }
                StreamChunk::Stop => break,
                _ => {}
            }
        }

        // Drain remaining buffer (last line without newline)
        if !line_buf.trim().is_empty() {
            let line = line_buf.trim().to_string();
            if let Ok(val) = serde_json::from_str::<Value>(&line) {
                if let Some(obj) = val.get("objective").and_then(Value::as_str) {
                    if objective_opt.is_none() {
                        objective_opt = Some(obj.to_string());
                    }
                }
            }
            if let Ok(step) = serde_json::from_str::<PlanStepLine>(&line) {
                on_step_parsed(step_index, step.id.clone(), step.description.clone());
                all_steps.push(step);
            }
        }

        let objective_str = objective_opt.unwrap_or_else(|| objective.to_string());

        if all_steps.is_empty() {
            return Err(agent_base::AgentError::plan_generation(
                "no valid steps".to_string(),
            ));
        }

        Ok(lines_to_plan(objective_str, all_steps))
    }
}

/// Stub step executor (never actually called in this test)
struct StubExecutor;
#[async_trait]
impl StepExecutor for StubExecutor {
    async fn execute_step(
        &self,
        step: &PlanStep,
        _plan_context: &Value,
        _ctx: &ToolContext,
    ) -> AgentResult<StepResult> {
        Ok(StepResult::success(format!("stub: {}", step.id), 0))
    }
}

async fn test_plan_orchestrator(
    client: Arc<OpenAiClient>,
    label: &str,
    enable_thinking: bool,
) -> AgentResult<()> {
    println!("\n{}", "=".repeat(70));
    println!(
        "TEST 2: PlanOrchestrator | {} | thinking={}",
        label, enable_thinking
    );
    println!("{}", "=".repeat(70));

    let t0 = Instant::now();

    let generator = Arc::new(LlmPlanGenerator {
        client: client.clone(),
        enable_thinking,
    });
    let executor = Arc::new(StubExecutor);
    let plan_store = Arc::new(InMemoryPlanStore::new());

    let runtime = AgentBuilder::new(client)
        .system_prompt(
            "你是一个资深的服务器运维工程师助手——「副驾驶」模式。\
             你在用户的终端面板中协同工作，用户能看到你执行的命令。\
             回复简洁直接，不要客套。",
        )
        .approval_handler(Arc::new(AllowAllApprovalHandler))
        .register_tool(PlanOrchestrator::new(
            generator.clone(),
            executor,
            plan_store.clone(),
        ))
        .build()?;

    let session_id = runtime.create_session().await;
    println!("{} session created", now_ms(t0));

    // Use run_turn_with_handler for real-time event printing
    let _outcome = runtime
        .run_turn_with_handler(
            session_id.clone(),
            "全面检查服务器健康状况\n\n[可用主机信息]\n- test-host (IP: 121.41.191.236)",
            |event| {
                match event {
                    RuntimeEvent::PlanGenerating { .. } => {
                        println!("{} 🔄 PlanGenerating", now_ms(t0));
                    }
                    RuntimeEvent::PlanStepParsed {
                        step_index,
                        step_id,
                        step_description,
                        ..
                    } => {
                        println!(
                            "{} ✅ PlanStepParsed [{}] {}: {}",
                            now_ms(t0), step_index, step_id, step_description,
                        );
                    }
                    RuntimeEvent::PlanGenerated { plan, .. } => {
                        println!(
                            "{} 📋 PlanGenerated: {} steps, objective=\"{}\"",
                            now_ms(t0),
                            plan.total_steps(),
                            plan.objective,
                        );
                    }
                    RuntimeEvent::ToolCallStarted { tool_name, .. } => {
                        println!("{} 🔧 ToolCallStarted: {}", now_ms(t0), tool_name);
                    }
                    RuntimeEvent::ToolCallFinished {
                        tool_name, ..
                    } => {
                        println!(
                            "{} ✓ ToolCallFinished: {}",
                            now_ms(t0), tool_name,
                        );
                    }
                    RuntimeEvent::RunFinished { .. } => {
                        println!("{} 🏁 RunFinished", now_ms(t0));
                    }
                    RuntimeEvent::ThoughtDelta { text, .. } => {
                        let short = &text[..text.len().min(60)];
                        println!("{} 💭 \"{}\"", now_ms(t0), short);
                    }
                    _ => {}
                }
                Ok(())
            },
        )
        .await?;

    println!(
        "\n{} 📋 total elapsed: {}ms",
        now_ms(t0),
        t0.elapsed().as_millis(),
    );

    Ok(())
}

// ── main ───────────────────────────────────────────────────────────────

#[tokio::main]
async fn main() -> AgentResult<()> {
    dotenvy::dotenv().ok();

    let api_key = std::env::var("OPENAI_API_KEY")
        .or_else(|_| std::env::var("DASHSCOPE_API_KEY"))
        .map_err(|_| agent_base::AgentError::internal("OPENAI_API_KEY 未设置"))?;

    let model =
        std::env::var("OPENAI_MODEL").unwrap_or_else(|_| "qwen-plus".to_string());

    let base_url = std::env::var("OPENAI_BASE_URL").unwrap_or_else(|_| {
        "https://dashscope.aliyuncs.com/compatible-mode/v1".to_string()
    });

    println!("╔════════════════════════════════════════════════════════════╗");
    println!("║  Plan Streaming Reproduction Test                         ║");
    println!("║  model: {:<51} ║", model);
    println!("╚════════════════════════════════════════════════════════════╝");

    let client = Arc::new(OpenAiClient::new(api_key, model, Some(base_url)));

    // ── Test 1: Direct LLM stream, no thinking ──
    test_direct_llm_stream(&client, "direct LLM (no thinking)", false).await?;

    // ── Test 2: Direct LLM stream, thinking enabled ──
    test_direct_llm_stream(&client, "direct LLM (thinking on)", true).await?;

    // ── Test 3: Through PlanOrchestrator, no thinking ──
    test_plan_orchestrator(client.clone(), "orchestrator (no thinking)", false).await?;

    // ── Test 4: Through PlanOrchestrator, thinking enabled ──
    test_plan_orchestrator(client, "orchestrator (thinking on)", true).await?;

    println!("\n═══════════════════════════════════════════════════════════");
    println!("  All tests complete.");
    println!("═══════════════════════════════════════════════════════════\n");

    Ok(())
}
