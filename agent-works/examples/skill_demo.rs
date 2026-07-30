use std::pin::Pin;
use std::sync::Arc;
use std::sync::Mutex;

use agent_base::{
    AgentResult, ChatMessage, LlmCapabilities, LlmClient, ReasoningConfig, ResponseFormat,
    RuntimeEvent, StreamChunk, Tool, ToolContext, ToolControlFlow, ToolOutput,
};
use agent_works::{
    AgentBuilder,
    skill::{FullDetailPrompter, LazySkillPrompter, Skill, SkillPrompter},
};
use async_trait::async_trait;
use futures_core::Stream;
use serde_json::{Value, json};

type ChunkStream = Pin<Box<dyn Stream<Item = AgentResult<StreamChunk>> + Send>>;

struct MockLlmClient {
    responses: Mutex<std::vec::IntoIter<Vec<StreamChunk>>>,
}

impl MockLlmClient {
    fn new(responses: Vec<Vec<StreamChunk>>) -> Self {
        Self {
            responses: Mutex::new(responses.into_iter()),
        }
    }
}

#[async_trait]
impl LlmClient for MockLlmClient {
    async fn chat(
        &self,
        _messages: &[ChatMessage],
        _tools: &[Value],
        _reasoning: Option<&ReasoningConfig>,
        _response_format: Option<&ResponseFormat>,
    ) -> AgentResult<Value> {
        unimplemented!()
    }

    async fn chat_stream(
        &self,
        _messages: &[ChatMessage],
        _tools: &[Value],
        _reasoning: Option<&ReasoningConfig>,
        _response_format: Option<&ResponseFormat>,
    ) -> AgentResult<ChunkStream> {
        let chunks: Vec<AgentResult<StreamChunk>> = self
            .responses
            .lock()
            .unwrap()
            .next()
            .unwrap_or_default()
            .into_iter()
            .map(Ok)
            .collect();
        Ok(Box::pin(futures_util::stream::iter(chunks)))
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

struct AddTool;

#[async_trait]
impl Tool for AddTool {
    fn name(&self) -> &'static str {
        "add"
    }

    fn definition(&self) -> Value {
        json!({
            "type": "function",
            "function": {
                "name": "add",
                "description": "Calculate the sum of two integers",
                "parameters": {
                    "type": "object",
                    "properties": {
                        "a": { "type": "integer", "description": "First addend" },
                        "b": { "type": "integer", "description": "Second addend" }
                    },
                    "required": ["a", "b"]
                }
            }
        })
    }

    async fn call(&self, args: &Value, _ctx: &ToolContext) -> AgentResult<ToolOutput> {
        let a = args["a"].as_i64().unwrap_or(0);
        let b = args["b"].as_i64().unwrap_or(0);
        Ok(ToolOutput {
            summary: format!("{a} + {b} = {}", a + b),
            raw: Some(json!({ "result": a + b })),
            control_flow: ToolControlFlow::Break,
            truncation: None,
        })
    }
}

struct SubtractTool;

#[async_trait]
impl Tool for SubtractTool {
    fn name(&self) -> &'static str {
        "subtract"
    }

    fn definition(&self) -> Value {
        json!({
            "type": "function",
            "function": {
                "name": "subtract",
                "description": "Calculate the difference of two integers (a - b)",
                "parameters": {
                    "type": "object",
                    "properties": {
                        "a": { "type": "integer", "description": "Minuend" },
                        "b": { "type": "integer", "description": "Subtrahend" }
                    },
                    "required": ["a", "b"]
                }
            }
        })
    }

    async fn call(&self, args: &Value, _ctx: &ToolContext) -> AgentResult<ToolOutput> {
        let a = args["a"].as_i64().unwrap_or(0);
        let b = args["b"].as_i64().unwrap_or(0);
        Ok(ToolOutput {
            summary: format!("{a} - {b} = {}", a - b),
            raw: Some(json!({ "result": a - b })),
            control_flow: ToolControlFlow::Break,
            truncation: None,
        })
    }
}

struct MathSkill;

impl Skill for MathSkill {
    fn name(&self) -> &'static str {
        "math"
    }

    fn brief_description(&self) -> String {
        "Math: supports addition and subtraction".to_string()
    }

    fn detailed_description(&self) -> String {
        "- **add**: Calculate the sum of two integers\n\
         - **subtract**: Calculate the difference of two integers"
            .to_string()
    }

    fn tools(&self) -> Vec<Arc<dyn Tool>> {
        vec![Arc::new(AddTool), Arc::new(SubtractTool)]
    }
}

#[tokio::main]
async fn main() -> AgentResult<()> {
    println!("=== agent-works Skill Demo ===\n");

    let llm = Arc::new(MockLlmClient::new(vec![
        vec![
            StreamChunk::ToolCall(json!({
                "delta": {
                    "tool_calls": [{
                        "id": "call_1",
                        "function": {
                            "name": "get_skill_detail",
                            "arguments": "{\"name\": \"math\"}"
                        }
                    }]
                }
            })),
            StreamChunk::Stop,
        ],
        vec![
            StreamChunk::ToolCall(json!({
                "delta": {
                    "tool_calls": [{
                        "id": "call_2",
                        "function": {
                            "name": "add",
                            "arguments": "{\"a\": 123, \"b\": 456}"
                        }
                    }]
                }
            })),
            StreamChunk::Stop,
        ],
        vec![
            StreamChunk::Text("123 + 456 = 579".to_string()),
            StreamChunk::Stop,
        ],
    ]));

    let runtime = AgentBuilder::new(llm)
        .system_prompt("You are a helpful assistant. Use skills when needed.")
        .register_skill(MathSkill)
        .build()
        .unwrap();

    println!("[1] Registered skill with 'register_skill()' on agent-works AgentBuilder");
    println!("    - Skill tools (add, subtract) auto-registered");
    println!("    - LazySkillPrompter injected into system prompt");
    println!("    - SkillDetailTool auto-registered as 'get_skill_detail'\n");

    let session_id = runtime.create_session().await;

    let (events, _outcome) = runtime
        .run_turn_collect(session_id, "help me calculate 123 + 456")
        .await?;

    for event in &events {
        match event {
            RuntimeEvent::ToolCallStarted {
                tool_name,
                args_json,
                ..
            } => {
                println!("[Tool Start] {tool_name} {args_json}");
            }
            RuntimeEvent::ToolCallFinished {
                tool_name, summary, ..
            } => {
                println!("[Tool Done] {tool_name}: {summary}");
            }
            RuntimeEvent::TextDelta { text, .. } => {
                print!("{text}");
            }
            _ => {}
        }
    }
    println!();

    println!("\n[2] Testing LazySkillPrompter and FullDetailPrompter");
    let skills: Vec<Arc<dyn Skill>> = vec![Arc::new(MathSkill)];
    let lazy = LazySkillPrompter::new();
    println!("LazySkillPrompter output:");
    println!("{}", lazy.build_prompt(&skills, "get_skill_detail"));

    let full = FullDetailPrompter;
    println!("\nFullDetailPrompter output:");
    println!("{}", full.build_prompt(&skills, "get_skill_detail"));

    println!("\n=== Demo Complete ===");
    Ok(())
}
