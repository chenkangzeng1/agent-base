use std::sync::Arc;
use std::sync::Mutex;

use agent_base::{
    AbortOnFailure, AgentBuilder, AgentResult, AlwaysContinue, ExecutionPlan,
    InMemoryPlanStore, LlmCapabilities, LlmClient, PlanConfig, PlanGenerator, PlanStep,
    PlanStore, Recovery, ResponseFormat, RuntimeEvent, StepExecutor, StepResult,
    StreamChunk, Tool, ToolContext, ToolControlFlow, ToolOutput, ChatMessage,
};
use async_trait::async_trait;
use serde_json::{json, Value};

struct GreetTool;

#[async_trait]
impl Tool for GreetTool {
    fn name(&self) -> &'static str {
        "greet"
    }

    fn definition(&self) -> Value {
        json!({
            "type": "function",
            "function": {
                "name": "greet",
                "description": "Generate a greeting message",
                "parameters": {
                    "type": "object",
                    "properties": {
                        "name": { "type": "string", "description": "Name to greet" }
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
            control_flow: ToolControlFlow::Break,
            truncation: None,
        })
    }
}

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
        unimplemented!()
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
        let chunks = if responses.is_empty() {
            vec![StreamChunk::Text("Done".to_string()), StreamChunk::Stop]
        } else {
            responses.remove(0)
        };
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

struct SimplePlanGenerator;

#[async_trait]
impl PlanGenerator for SimplePlanGenerator {
    async fn generate_plan(
        &self,
        objective: &str,
        _context: &str,
        _tools: &[Value],
    ) -> AgentResult<ExecutionPlan> {
        let plan = ExecutionPlan::of_steps(
            "demo-plan",
            objective,
            vec![
                PlanStep::tool_call("step-1", "Greet the user", "greet", json!({"name": "User"})),
                PlanStep::tool_call("step-2", "Confirm completion", "greet", json!({"name": "Team"})),
            ],
        );

        Ok(plan)
    }
}

struct SimpleStepExecutor;

#[async_trait]
impl StepExecutor for SimpleStepExecutor {
    async fn execute_step(
        &self,
        step: &PlanStep,
        _step_outputs: &Value,
        _ctx: &ToolContext,
    ) -> AgentResult<StepResult> {
        println!("  Executing step: {} - {}", step.id, step.description);
        Ok(StepResult::success(format!("Step {} done", step.id), 100))
    }
}

#[tokio::main]
async fn main() -> AgentResult<()> {
    println!("=== Plan-and-Execute Demo (New API) ===\n");

    let llm = Arc::new(MockLlmClient::new(vec![
        vec![StreamChunk::Text("Greeting User...".to_string()), StreamChunk::Stop],
        vec![StreamChunk::Text("Greeting Team...".to_string()), StreamChunk::Stop],
    ]));

    let runtime = AgentBuilder::new(llm.clone())
        .register_tool(GreetTool)
        .build()?;

    let generator = Arc::new(SimplePlanGenerator);
    let executor = Arc::new(SimpleStepExecutor);
    let plan_store = Arc::new(InMemoryPlanStore::new());

    let session_id = runtime.create_session().await;

    // ── New API: run_plan_with_generator ─────────────────────────────
    println!("1. Generating and executing plan with run_plan_with_generator...\n");

    let result = runtime
        .run_plan_with_generator(
            session_id.clone(),
            "Greet the user and team",
            generator,
            PlanConfig::new()
                .executor(executor.clone())
                .recovery(Recovery::abort())
                .store(plan_store.clone()),
            |event| match event {
                RuntimeEvent::PlanGenerated { plan, .. } => {
                    println!("[PlanGenerated] id={}, objective={}", plan.id, plan.objective);
                    println!("  Steps: {}", plan.total_steps());
                    for step in plan.all_steps() {
                        println!("    - {}: {}", step.id, step.description);
                    }
                    Ok(())
                }
                RuntimeEvent::PlanStepStarted { step_id, step_description, .. } => {
                    println!("[PlanStepStarted] {} - {}", step_id, step_description);
                    Ok(())
                }
                RuntimeEvent::PlanStepCompleted { step_id, success, result, .. } => {
                    println!(
                        "[PlanStepCompleted] {} success={} result={:?}",
                        step_id, success, result
                    );
                    Ok(())
                }
                RuntimeEvent::PlanCompleted { plan_id, success, .. } => {
                    println!("[PlanCompleted] {} success={}", plan_id, success);
                    Ok(())
                }
                _ => Ok(()),
            },
        )
        .await?;

    println!("\nResult: {:?}", result);

    // ── New API: run_plan with pre-built plan ────────────────────────
    println!("\n2. Executing pre-built plan with run_plan...\n");

    let plan = ExecutionPlan::of_steps(
        "manual-plan",
        "Greet everyone manually",
        vec![
            PlanStep::tool_call("s1", "Greet Alice", "greet", json!({"name": "Alice"})),
            PlanStep::tool_call("s2", "Greet Bob", "greet", json!({"name": "Bob"})),
        ],
    );

    let result = runtime
        .run_plan(
            session_id.clone(),
            plan,
            PlanConfig::new()
                .executor(executor.clone())
                .recovery(Recovery::skip()),
            |event| {
                if let RuntimeEvent::PlanStepCompleted { step_id, success, .. } = event {
                    println!("  [{}] {}", if success { "✓" } else { "✗" }, step_id);
                }
                Ok(())
            },
        )
        .await?;

    println!("\nResult: {:?}", result);

    // ── Recovery strategies ──────────────────────────────────────────
    println!("\n3. Recovery strategies demo...\n");

    println!("  Recovery::abort()  = AbortOnFailure");
    println!("  Recovery::skip()   = SkipOnFailure");
    println!("  Recovery::retry(3) = RetryOnFailure {{ max_retries: 3 }}");
    println!("  Recovery::custom(|step, err, count, plan, step_outputs| {{ ... }}) = CustomRecovery");

    // ── Check stored plan ────────────────────────────────────────────
    let stored = plan_store.load_plan("demo-plan").await?;
    if let Some(data) = stored {
        println!("\nStored plan status: {:?}", data.plan.status);
    }

    println!("\n=== Demo Complete ===");
    Ok(())
}
