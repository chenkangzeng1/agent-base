use std::sync::Arc;
use std::sync::Mutex;

use agent_base::{
    AbortOnFailure, AgentBuilder, AgentEvent, AgentResult, AlwaysContinue, ExecutionPlan,
    InMemoryPlanStore, LlmCapabilities, LlmClient, PlanGenerator, PlanStep, PlanStore,
    ResponseFormat, StepExecutor, StepResult, StreamChunk, Tool, ToolContext, ToolControlFlow,
    ToolOutput, ChatMessage, RecoveryStrategy, StepContinuePolicy,
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
    ) -> AgentResult<Pin<Box<dyn futures_core::Stream<Item = AgentResult<StreamChunk>> + Send>>> {
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
        let mut plan = ExecutionPlan::new("demo-plan", objective);

        plan.steps = vec![
            PlanStep::new(
                "step-1",
                "Greet the user",
                json!({"type": "tool_call", "tool_name": "greet", "args": {"name": "User"}}),
            ),
            PlanStep::new(
                "step-2",
                "Confirm completion",
                json!({"type": "tool_call", "tool_name": "greet", "args": {"name": "Team"}}),
            ),
        ];

        Ok(plan)
    }
}

struct SimpleStepExecutor;

#[async_trait]
impl StepExecutor for SimpleStepExecutor {
    async fn execute_step(
        &self,
        step: &PlanStep,
        _plan_context: &Value,
    ) -> AgentResult<StepResult> {
        println!("  Executing step: {} - {}", step.id, step.description);
        Ok(StepResult::success(format!("Step {} done", step.id), 100))
    }
}

use std::pin::Pin;

#[tokio::main]
async fn main() -> AgentResult<()> {
    println!("=== Plan-and-Execute Demo ===\n");

    let llm = Arc::new(MockLlmClient::new(vec![
        vec![StreamChunk::Text("Greeting User...".to_string()), StreamChunk::Stop],
        vec![StreamChunk::Text("Greeting Team...".to_string()), StreamChunk::Stop],
    ]));

    let runtime = AgentBuilder::new(llm.clone())
        .register_tool(GreetTool)
        .build().unwrap();

    let generator = Arc::new(SimplePlanGenerator);
    let executor = Arc::new(SimpleStepExecutor);
    let plan_store = Arc::new(InMemoryPlanStore::new());

    let session_id = runtime.create_session().await;

    println!("Generating and executing plan (deterministic mode)...\n");

    let result = runtime
        .run_plan_deterministic(
            session_id,
            "Greet the user and team",
            generator,
            executor,
            Some(Arc::new(AlwaysContinue)),
            Some(Arc::new(AbortOnFailure)),
            Some(plan_store.clone()),
            |event| match event {
                AgentEvent::PlanGenerated { plan, .. } => {
                    println!("[PlanGenerated] id={}, objective={}", plan.id, plan.objective);
                    println!("  Steps: {}", plan.steps.len());
                    for step in &plan.steps {
                        println!("    - {}: {}", step.id, step.description);
                    }
                    Ok(())
                }
                AgentEvent::PlanStepStarted { step_id, step_description, .. } => {
                    println!("[PlanStepStarted] {} - {}", step_id, step_description);
                    Ok(())
                }
                AgentEvent::PlanStepCompleted { step_id, success, result, .. } => {
                    println!(
                        "[PlanStepCompleted] {} success={} result={:?}",
                        step_id, success, result
                    );
                    Ok(())
                }
                AgentEvent::PlanCompleted { plan_id, success, .. } => {
                    println!("[PlanCompleted] {} success={}", plan_id, success);
                    Ok(())
                }
                _ => Ok(()),
            },
        )
        .await?;

    println!("\nResult: {:?}", result);

    let stored = plan_store.load_plan("demo-plan").await?;
    if let Some(data) = stored {
        println!("\nStored plan status: {:?}", data.plan.status);
    }

    println!("\n=== Demo Complete ===");
    Ok(())
}
