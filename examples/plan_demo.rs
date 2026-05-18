use std::sync::Arc;
use std::sync::Mutex;

use agent_base::{
    AgentBuilder, AgentEvent, AgentResult, ExecutionPlan, InMemoryPlanStore,
    LlmCapabilities, LlmClient, PlanExecutor, PlanStep, PlanStore, RecoveryAction, ResponseFormat,
    StepActionType, StepResult, StreamChunk, Tool, ToolContext, ToolControlFlow,
    ToolOutput, ChatMessage,
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
            truncated: false,
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
        _enable_thinking: Option<bool>,
        _response_format: Option<&ResponseFormat>,
    ) -> AgentResult<Value> {
        unimplemented!()
    }

    async fn chat_stream(
        &self,
        _messages: &[ChatMessage],
        _tools: &[Value],
        _enable_thinking: Option<bool>,
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

struct SimplePlanExecutor;

#[async_trait]
impl PlanExecutor for SimplePlanExecutor {
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
                StepActionType::ToolCall {
                    tool_name: "greet".to_string(),
                    args: json!({ "name": "User" }),
                },
            ),
            PlanStep::new(
                "step-2",
                "Confirm completion",
                StepActionType::ToolCall {
                    tool_name: "greet".to_string(),
                    args: json!({ "name": "Team" }),
                },
            ),
        ];

        Ok(plan)
    }

    async fn execute_step(
        &self,
        step: &PlanStep,
        _plan_context: &Value,
    ) -> AgentResult<StepResult> {
        println!("  Executing step: {} - {}", step.id, step.description);
        Ok(StepResult::success(format!("Step {} done", step.id), 100))
    }

    async fn should_continue(
        &self,
        _plan: &ExecutionPlan,
        _current_step: &PlanStep,
    ) -> AgentResult<bool> {
        Ok(true)
    }

    async fn handle_step_failure(
        &self,
        _step: &PlanStep,
        _error: &str,
        _retry_count: usize,
    ) -> AgentResult<RecoveryAction> {
        Ok(RecoveryAction::Abort)
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

    let mut runtime = AgentBuilder::new(llm.clone())
        .register_tool(GreetTool)
        .build();

    let plan_executor = Arc::new(SimplePlanExecutor);
    let plan_store = Arc::new(InMemoryPlanStore::new());

    let session_id = runtime.create_session();

    println!("Generating and executing plan...\n");

    let result = runtime
        .run_with_plan(
            session_id,
            "Greet the user and team",
            plan_executor,
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
