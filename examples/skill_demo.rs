use std::sync::Arc;

use agent_base::{
    AgentBuilder, AgentEvent, AgentResult, OpenAiClient,
    Skill,
    Tool, ToolContext, ToolControlFlow, ToolOutput,
};
use async_trait::async_trait;
use dotenvy::dotenv;
use serde_json::{json, Value};

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
            summary: format!("{} + {} = {}", a, b, a + b),
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
            summary: format!("{} - {} = {}", a, b, a - b),
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
        r#"## Math Skill

### Available tools
- **add**: Calculate the sum of two integers。with args: a (First addend), b (Second addend)
- **subtract**: Calculate the difference of two integers。with args: a (Minuend), b (Subtrahend)

### Usage
- Use for integer arithmetic add or subtract tool
- Complex calculations can combine multiple tool calls
"#.trim().to_string()
    }

    fn tools(&self) -> Vec<Arc<dyn Tool>> {
        vec![
            Arc::new(AddTool),
            Arc::new(SubtractTool),
        ]
    }
}

struct UppercaseTool;

#[async_trait]
impl Tool for UppercaseTool {
    fn name(&self) -> &'static str {
        "uppercase"
    }

    fn definition(&self) -> Value {
        json!({
            "type": "function",
            "function": {
                "name": "uppercase",
                "description": "Convert text to uppercase",
                "parameters": {
                    "type": "object",
                    "properties": {
                        "text": { "type": "string", "description": "Text to convert" }
                    },
                    "required": ["text"]
                }
            }
        })
    }

    async fn call(&self, args: &Value, _ctx: &ToolContext) -> AgentResult<ToolOutput> {
        let text = args["text"].as_str().unwrap_or("");
        Ok(ToolOutput {
            summary: text.to_uppercase(),
            raw: Some(json!({ "result": text.to_uppercase() })),
            control_flow: ToolControlFlow::Break,
            truncation: None,
        })
    }
}

struct TextSkill;

impl Skill for TextSkill {
    fn name(&self) -> &'static str {
        "text"
    }

    fn brief_description(&self) -> String {
        "Text processing：Supports case conversion".to_string()
    }

    fn detailed_description(&self) -> String {
        r#"## Text processing Skill

### Available tools
- **uppercase**: Convert text to uppercase。with args: text (Text to convert)

### Usage
- Use when you need to convert text to uppercase uppercase tool
"#.trim().to_string()
    }

    fn tools(&self) -> Vec<Arc<dyn Tool>> {
        vec![
            Arc::new(UppercaseTool),
        ]
    }
}

#[tokio::main]
async fn main() {
    dotenv().ok();

    let api_key = std::env::var("OPENAI_API_KEY")
        .or_else(|_| std::env::var("DASHSCOPE_API_KEY"))
        .expect("Please set OPENAI_API_KEY or ANTHROPIC_API_KEY");

    let model = std::env::var("OPENAI_MODEL")
        .or_else(|_| std::env::var("DASHSCOPE_MODEL"))
        .unwrap_or_else(|_| "gpt-4o".to_string());

    let base_url = std::env::var("OPENAI_BASE_URL")
        .or_else(|_| std::env::var("DASHSCOPE_BASE_URL"))
        .ok();

    let client = Arc::new(OpenAiClient::new(api_key, model, base_url));

    let runtime = AgentBuilder::new(client)
        .system_prompt("You are a general-purpose assistant. Please use the tools provided in the Skill.")
        .register_skill(MathSkill)
        .register_skill(TextSkill)
        .build().unwrap();

    println!("=== Skills Demo ===");
    println!("Registered {}  Skill:", runtime.skills().len());
    for skill in runtime.skills() {
        println!("  - {}: {}", skill.name(), skill.brief_description());
    }
    println!();

    let session_id = runtime.create_session().await;

    let user_input = "help me calculate 123 + 456，then convert the result to uppercase";
    println!("User: {}", user_input);
    println!("---");

    let (events, _outcome) = runtime
        .run_turn_stream(session_id.clone(), user_input)
        .await
        .expect("Execution failed");

    for event in &events {
        match event {
            AgentEvent::TextDelta { text, .. } => print!("{}", text),
            AgentEvent::ToolCallStarted { tool_name, args_json, .. } => {
                println!("\nCalling tool: {} ({})", tool_name, args_json);
            }
            AgentEvent::ToolCallFinished { tool_name, summary, .. } => {
                println!("✅ {} → {}", tool_name, summary);
            }
            AgentEvent::Custom { payload, .. } => {
                if payload.get("type").and_then(Value::as_str) == Some("skill_detail_loaded") {
                    if let Some(skill) = payload.get("skill").and_then(Value::as_str) {
                        println!("📖 Loading Skill manual: {}", skill);
                    }
                }
            }
            _ => {}
        }
    }

    println!();
    println!("---");
    println!("done!");
}
